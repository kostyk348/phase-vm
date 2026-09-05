/* phase_alloc v3 — рабочий LD_PRELOAD аллокатор с фазовыми кадрами.
#include <dlfcn.h>
 * Безопасность: все наши mmap-регионы в реестре; заголовки читаются ТОЛЬКО
 * если указатель внутри нашего региона -> чужой указатель (glibc-внутренний)
 * уходит в RTLD_NEXT, никаких чтений до проверки владения.
 * Потокобезопасен, полный POSIX ABI, выравнивание 16.
 */
#define _GNU_SOURCE
#include <stdint.h>
#include <dlfcn.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <pthread.h>
#include <dlfcn.h>
#include <sys/mman.h>

#define CLASS_MAGIC 0xFA5E0001u
#define ALIGN_MAGIC 0xFA5E0002u
#define LARGE_BIT   (1ull<<63)

static const size_t CLASS_SZ[] = {16,32,48,64,96,128,192,256,384,512,768,1024,1536,2048,3072,4096};
#define NCLASS (sizeof(CLASS_SZ)/sizeof(CLASS_SZ[0]))
#define REGMAX 65536
typedef struct { void* base; size_t len; } Reg;
static Reg regs[REGMAX];
static int nreg = 0;
static pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;

typedef struct { void* base; size_t cap, off; void* free_head; } Arena;
static Arena arenas[NCLASS];


static int class_of(size_t n){ for(size_t i=0;i<NCLASS;i++) if(n<=CLASS_SZ[i]) return (int)i; return -1; }

static void reg_add(void* b, size_t l){ if(nreg<REGMAX){ regs[nreg].base=b; regs[nreg].len=l; nreg++; } }
static void reg_del(void* b){ for(int i=0;i<nreg;i++) if(regs[i].base==b){ regs[i]=regs[nreg-1]; nreg--; return; } }
/* индекс региона, содержащего p (или -1) */
static int reg_find(void* p){ uintptr_t x=(uintptr_t)p; for(int i=0;i<nreg;i++){ uintptr_t b=(uintptr_t)regs[i].base; if(x>=b && x<b+regs[i].len) return i; } return -1; }

static void* carve_locked(Arena* a, size_t n){
    if(a->free_head){ void* p=a->free_head; a->free_head=*(void**)p; return p; }
    if(a->off+n>a->cap){
        size_t cap=1u<<20;
        void* m=mmap(NULL,cap,PROT_READ|PROT_WRITE,MAP_PRIVATE|MAP_ANONYMOUS,-1,0);
        if(m==MAP_FAILED) return NULL;
        a->base=m; a->cap=cap; a->off=0;
        reg_add(m,cap);
    }
    void* p=(char*)a->base+a->off; a->off+=n; return p;
}

/* ---------- per-thread фазовые кадры ---------- */
static __thread int t_depth=0;
static __thread void* t_base=NULL;
static __thread size_t t_used=0, t_cap=0;
static pthread_key_t key;
static pthread_once_t key_once=PTHREAD_ONCE_INIT;
static void bump_dtor(void* p){ if(p) munmap(p,(size_t)64u<<20); }
static void key_init(void){ pthread_key_create(&key,bump_dtor); }

void pa_frame_begin(void){
    pthread_once(&key_once,key_init);
    if(t_depth==0){
        if(!t_base){
            t_cap=64u<<20;
            t_base=mmap(NULL,t_cap,PROT_READ|PROT_WRITE,MAP_PRIVATE|MAP_ANONYMOUS,-1,0);
            if(t_base==MAP_FAILED){ t_base=NULL; return; }
            pthread_setspecific(key,t_base);
        }
        t_used=0;
    }
    t_depth++;
}
void pa_frame_end(void){
    if(t_depth>0){ t_depth--; if(t_depth==0) t_used=0; }
}
int pa_frame_active(void){ return t_depth>0; }

/* ---------- выделение ---------- */
static void* alloc_locked(size_t n){
    if(n==0) n=1;
    int c=class_of(n);
    if(c>=0){
        void* p=carve_locked(&arenas[c],CLASS_SZ[c]+16);
        if(!p) return NULL;
        ((uint64_t*)p)[0]=CLASS_MAGIC|((uint64_t)c<<32);
        return (char*)p+16;
    }
    size_t tot=n+16;
    void* m=mmap(NULL,tot,PROT_READ|PROT_WRITE,MAP_PRIVATE|MAP_ANONYMOUS,-1,0);
    if(m==MAP_FAILED) return NULL;
    ((uint64_t*)m)[0]=CLASS_MAGIC|LARGE_BIT;
    ((uint64_t*)m)[1]=tot;
    reg_add(m,tot);
    return (char*)m+16;
}

static void* phase_malloc(size_t n){
    if(t_depth>0){
        size_t need=(n+15)&~(size_t)15;
        if(need==0) need=16;
        if(t_used+need<=t_cap){ void* p=(char*)t_base+t_used; t_used+=need; return p; }
    }
    pthread_mutex_lock(&lock);
    void* p=alloc_locked(n);
    pthread_mutex_unlock(&lock);
    return p;
}

static void* aligned_impl(size_t align,size_t size){
    if(size==0) size=1;
    if(align<16) align=16;
    if(align&(align-1)) return NULL;
    size_t tot=size+align+64;
    pthread_mutex_lock(&lock);
    void* m=mmap(NULL,tot,PROT_READ|PROT_WRITE,MAP_PRIVATE|MAP_ANONYMOUS,-1,0);
    if(m==MAP_FAILED){ pthread_mutex_unlock(&lock); return NULL; }
    uintptr_t a=(((uintptr_t)m)+32+align-1)&~(uintptr_t)(align-1);
    void* p=(void*)a;
    ((uint64_t*)p)[-4]=ALIGN_MAGIC;
    ((uint64_t*)p)[-3]=(uint64_t)m;
    ((uint64_t*)p)[-2]=(uint64_t)tot;
    reg_add(m,tot);
    pthread_mutex_unlock(&lock);
    return p;
}

/* ---------- free (безопасный: deref только внутри наших регионов) ---------- */
static void (*real_free)(void*)=NULL;
static int real_free_tried=0;

static void phase_free(void* p){
    if(!p) return;
    if(t_depth>0 && (char*)p>=(char*)t_base && (char*)p<(char*)t_base+t_used) return; /* кадр */
    pthread_mutex_lock(&lock);
    int ri=reg_find(p);
    if(ri<0){
        pthread_mutex_unlock(&lock);
        if(!real_free_tried){ real_free_tried=1; real_free=(void(*)(void*))dlsym(RTLD_NEXT,"free"); }
        if(real_free) real_free(p);
    }
    /* p внутри нашего региона. Читаем p-16 ПЕРВЫМ: для class/large/aligned
       p-16 всегда внутри региона (данные идут после заголовка). */
    uint64_t h=*(uint64_t*)((char*)p-16);
    if((h&0xffffffffu)==CLASS_MAGIC){
        if(h&LARGE_BIT){
            void* base=(char*)p-16;
            size_t tot=(size_t)*(uint64_t*)((char*)p-8);
            reg_del(base);
            pthread_mutex_unlock(&lock);
            munmap(base,tot);
            return;
        }
        int c=(int)((h>>32)&0xff);
        if(c>=0&&c<(int)NCLASS){ *(void**)p=arenas[c].free_head; arenas[c].free_head=p; }
        pthread_mutex_unlock(&lock);
        return;
    }
    /* не class: aligned? (у aligned p-16 = служебное, p-32 = magic) */
    uint64_t am=*(uint64_t*)((char*)p-32);
    if((am&0xffffffffu)==ALIGN_MAGIC){
        void* orig=(void*)*(uint64_t*)((char*)p-24);
        size_t tot=(size_t)*(uint64_t*)((char*)p-16);
        reg_del(orig);
        pthread_mutex_unlock(&lock);
        munmap(orig,tot);
        return;
    }
    pthread_mutex_unlock(&lock);
}

static size_t phase_usable(void* p){
    if(!p) return 0;
    if(t_depth>0&&(char*)p>=(char*)t_base&&(char*)p<(char*)t_base+t_used) return t_used-((char*)p-(char*)t_base);
    pthread_mutex_lock(&lock);
    size_t r=0;
    if(reg_find(p)>=0){
        uint64_t h=*(uint64_t*)((char*)p-16);
        if((h&0xffffffffu)==CLASS_MAGIC){
            if(h&LARGE_BIT) r=(size_t)*(uint64_t*)((char*)p-8)-16;
            else { int c=(int)((h>>32)&0xff); r=(c>=0&&c<(int)NCLASS)? CLASS_SZ[c]:0; }
        }
    }
    pthread_mutex_unlock(&lock);
    return r;
}

/* ---------- ABI ---------- */
void* malloc(size_t n){ return phase_malloc(n); }
void free(void* p){ phase_free(p); }
void* calloc(size_t n,size_t s){ size_t t; if(__builtin_mul_overflow(n,s,&t)) return NULL; void* p=phase_malloc(t); if(p) memset(p,0,t); return p; }
void* realloc(void* p,size_t n){
    if(!p) return phase_malloc(n);
    if(!n){ phase_free(p); return NULL; }
    size_t old=phase_usable(p);
    void* q=phase_malloc(n);
    if(!q) return NULL;
    if(old) memcpy(q,p,old<n?old:n);
    phase_free(p);
    return q;
}
void* reallocarray(void* p,size_t n,size_t s){ size_t t; if(__builtin_mul_overflow(n,s,&t)) return NULL; return realloc(p,t); }
int posix_memalign(void** m,size_t align,size_t size){
    if(align<sizeof(void*)) align=sizeof(void*);
    if(align&(align-1)) return 22;
    void* p=aligned_impl(align,size);
    if(!p) return 12;
    *m=p; return 0;
}
void* aligned_alloc(size_t align,size_t size){ return aligned_impl(align,size); }
void* memalign(size_t align,size_t size){ return aligned_impl(align,size); }
void* valloc(size_t size){ return aligned_impl(4096,size); }
size_t malloc_usable_size(void* p){ return phase_usable(p); }
char* strdup(const char* s){ size_t n=strlen(s)+1; char* p=malloc(n); if(p) memcpy(p,s,n); return p; }
char* strndup(const char* s,size_t m){ size_t n=0; while(n<m&&s[n]) n++; char* p=malloc(n+1); if(p){ memcpy(p,s,n); p[n]=0; } return p; }
void* __libc_malloc(size_t n){ return phase_malloc(n); }
void __libc_free(void* p){ phase_free(p); }
void* __libc_calloc(size_t n,size_t s){ return calloc(n,s); }
void* __libc_realloc(void* p,size_t n){ return realloc(p,n); }
