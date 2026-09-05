/* phase_alloc v4 — MT-safe LD_PRELOAD аллокатор.
 * Ключевая идея MT-безопасности: У КАЖДОГО ПОТОКА свои арены и свой
 * private free-лист (трогает только владелец). Чужой free() (чужой поток
 * освобождает чужой блок) кладёт блок в глобальный pending под mutex;
 * владелец при malloc забирает pending в свой лист под тем же mutex.
 * Shared free-лист между потоками отсутствует -> нет гонки списков.
 * Владелец блока пишется в заголовок (owner id). Заголовки class/large
 * в 16B перед ptr; aligned в 32B. Полный POSIX ABI, выравнивание 16.
 */
#define _GNU_SOURCE
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <pthread.h>
#include <dlfcn.h>
#include <sys/mman.h>

#define CLASS_MAGIC 0xFA5E0001u
#define ALIGN_MAGIC 0xFA5E0002u
#define LARGE_BIT   (1ull<<63)
#define FREE_BIT    (1ull<<62)

static const size_t CLASS_SZ[] = {16,32,48,64,96,128,192,256,384,512,768,1024,1536,2048,3072,4096};
#define NCLASS (sizeof(CLASS_SZ)/sizeof(CLASS_SZ[0]))

typedef struct { void* base; size_t cap, off; void* free_head; } Arena;
static Arena g_pending[NCLASS];   /* чужие освобождения (под g_lock) */
static __thread Arena t_a[NCLASS];/* приватные арены потока */
static __thread int t_id = -1;
static int g_next_id = 1;
static pthread_mutex_t g_lock = PTHREAD_MUTEX_INITIALIZER;

#define REGMAX 65536
typedef struct { void* base; size_t len; } Reg;
static Reg regs[REGMAX];
static int nreg = 0;

static int class_of(size_t n){ for(size_t i=0;i<NCLASS;i++) if(n<=CLASS_SZ[i]) return (int)i; return -1; }

static void reg_add(void* b, size_t l){ if(nreg<REGMAX){ regs[nreg].base=b; regs[nreg].len=l; nreg++; } }
static void reg_del(void* b){ for(int i=0;i<nreg;i++) if(regs[i].base==b){ regs[i]=regs[nreg-1]; nreg--; return; } }
static int reg_find(void* p){ uintptr_t x=(uintptr_t)p; for(int i=0;i<nreg;i++){ uintptr_t b=(uintptr_t)regs[i].base; if(x>=b && x<b+regs[i].len) return i; } return -1; }

static inline int my_id(void){ if(t_id<0){ pthread_mutex_lock(&g_lock); if(t_id<0) t_id=g_next_id++; pthread_mutex_unlock(&g_lock);} return t_id; }

/* приватный bump: mmap региона если нужно (под g_lock), выдать блок */
static void* bump(Arena* a, size_t n){
    if(a->off+n>a->cap){
        pthread_mutex_lock(&g_lock);
        if(a->off+n>a->cap){
            size_t cap=1u<<20;
            void* m=mmap(NULL,cap,PROT_READ|PROT_WRITE,MAP_PRIVATE|MAP_ANONYMOUS,-1,0);
            if(m!=MAP_FAILED){ a->base=m; a->cap=cap; a->off=0; reg_add(m,cap); }
        }
        pthread_mutex_unlock(&g_lock);
        if(a->off+n>a->cap) return NULL;
    }
    void* p=(char*)a->base+a->off; a->off+=n; return p;
}

/* ---------- per-thread фазовые кадры ---------- */
static __thread int fr_depth=0;
static __thread void* fr_base=NULL;
static __thread size_t fr_used=0, fr_cap=0;
static pthread_key_t key;
static pthread_once_t key_once=PTHREAD_ONCE_INIT;
static void bump_dtor(void* p){ if(p) munmap(p,(size_t)64u<<20); }
static void key_init(void){ pthread_key_create(&key,bump_dtor); }

void pa_frame_begin(void){
    pthread_once(&key_once,key_init);
    if(fr_depth==0){
        if(!fr_base){ fr_cap=64u<<20; fr_base=mmap(NULL,fr_cap,PROT_READ|PROT_WRITE,MAP_PRIVATE|MAP_ANONYMOUS,-1,0);
            if(fr_base==MAP_FAILED){ fr_base=NULL; return; } pthread_setspecific(key,fr_base); }
        fr_used=0;
    }
    fr_depth++;
}
void pa_frame_end(void){ if(fr_depth>0){ fr_depth--; if(fr_depth==0) fr_used=0; } }
int pa_frame_active(void){ return fr_depth>0; }

/* ---------- malloc/free ---------- */
static void* phase_malloc(size_t n){
    if(n==0) n=1;
    /* фазовый кадр */
    if(fr_depth>0){
        size_t need=(n+15)&~(size_t)15; if(need==0) need=16;
        if(fr_used+need<=fr_cap){ void* p=(char*)fr_base+fr_used; fr_used+=need; return p; }
    }
    int id=my_id();
    int c=class_of(n);
    if(c>=0){
        Arena* a=&t_a[c];
        /* private лист */
        if(a->free_head){ void* p=a->free_head; a->free_head=*(void**)p; return p; }
        /* забрать чужие pending */
        pthread_mutex_lock(&g_lock);
        if(g_pending[c].free_head){ a->free_head=g_pending[c].free_head; g_pending[c].free_head=NULL; }
        pthread_mutex_unlock(&g_lock);
        if(a->free_head){ void* p=a->free_head; a->free_head=*(void**)p; return p; }
        void* p=bump(a,CLASS_SZ[c]+16);
        if(!p) return NULL;
        ((uint64_t*)p)[0]=CLASS_MAGIC|((uint64_t)c<<32);
        ((uint64_t*)p)[1]=(uint32_t)id;
        return (char*)p+16;
    }
    /* крупный */
    size_t tot=n+16;
    pthread_mutex_lock(&g_lock);
    void* m=mmap(NULL,tot,PROT_READ|PROT_WRITE,MAP_PRIVATE|MAP_ANONYMOUS,-1,0);
    if(m!=MAP_FAILED){ ((uint64_t*)m)[0]=CLASS_MAGIC|LARGE_BIT; ((uint64_t*)m)[1]=tot; reg_add(m,tot); }
    pthread_mutex_unlock(&g_lock);
    if(m==MAP_FAILED) return NULL;
    return (char*)m+16;
}

static void (*real_free)(void*)=NULL;
static int real_free_tried=0;

static void phase_free(void* p){
    if(!p) return;
    if(fr_depth>0 && (char*)p>=(char*)fr_base && (char*)p<(char*)fr_base+fr_used) return;
    pthread_mutex_lock(&g_lock);
    if(reg_find(p)<0){
        pthread_mutex_unlock(&g_lock);
        if(!real_free_tried){ real_free_tried=1; real_free=(void(*)(void*))dlsym(RTLD_NEXT,"free"); }
        if(real_free) real_free(p);
        return;
    }
    uint64_t h=*(uint64_t*)((char*)p-16);
    if((h&0xffffffffu)==CLASS_MAGIC){
        if(h&LARGE_BIT){ void* base=(char*)p-16; size_t tot=(size_t)*(uint64_t*)((char*)p-8);
            reg_del(base); pthread_mutex_unlock(&g_lock); munmap(base,tot); return; }
        int c=(int)((h>>32)&0xff);
        uint32_t owner=(uint32_t)*(uint64_t*)((char*)p-8);
        if(c>=0&&c<(int)NCLASS){
            if(!(h&FREE_BIT)){
                *(uint64_t*)((char*)p-16)=CLASS_MAGIC|((uint64_t)c<<32)|FREE_BIT;
                if(owner==(uint32_t)my_id()){
                    /* свой блок: кладём в свой лист (мы держим g_lock — ок) */
                    Arena* a=&t_a[c];
                    /* чужой мог положить сюда pending? нет: pending отдельный */
                    *(void**)p=a->free_head; a->free_head=p;
                } else {
                    *(void**)p=g_pending[c].free_head; g_pending[c].free_head=p;
                }
            }
        }
        pthread_mutex_unlock(&g_lock);
        return;
    }
    uint64_t am=*(uint64_t*)((char*)p-32);
    if((am&0xffffffffu)==ALIGN_MAGIC){
        void* orig=(void*)*(uint64_t*)((char*)p-24);
        size_t tot=(size_t)*(uint64_t*)((char*)p-16);
        reg_del(orig); pthread_mutex_unlock(&g_lock); munmap(orig,tot); return;
    }
    pthread_mutex_unlock(&g_lock);
}

static size_t phase_usable(void* p){
    if(!p) return 0;
    if(fr_depth>0&&(char*)p>=(char*)fr_base&&(char*)p<(char*)fr_base+fr_used) return fr_used-((char*)p-(char*)fr_base);
    size_t r=0;
    pthread_mutex_lock(&g_lock);
    if(reg_find(p)>=0){
        uint64_t h=*(uint64_t*)((char*)p-16);
        if((h&0xffffffffu)==CLASS_MAGIC){
            if(h&LARGE_BIT) r=(size_t)*(uint64_t*)((char*)p-8)-16;
            else { int c=(int)((h>>32)&0xff); r=(c>=0&&c<(int)NCLASS)? CLASS_SZ[c]:0; }
        }
    }
    pthread_mutex_unlock(&g_lock);
    return r;
}

static void* aligned_impl(size_t align,size_t size){
    if(size==0) size=1;
    if(align<16) align=16;
    if(align&(align-1)) return NULL;
    size_t tot=size+align+64;
    pthread_mutex_lock(&g_lock);
    void* m=mmap(NULL,tot,PROT_READ|PROT_WRITE,MAP_PRIVATE|MAP_ANONYMOUS,-1,0);
    if(m==MAP_FAILED){ pthread_mutex_unlock(&g_lock); return NULL; }
    uintptr_t a=(((uintptr_t)m)+32+align-1)&~(uintptr_t)(align-1);
    void* p=(void*)a;
    ((uint64_t*)p)[-4]=ALIGN_MAGIC;
    ((uint64_t*)p)[-3]=(uint64_t)m;
    ((uint64_t*)p)[-2]=(uint64_t)tot;
    reg_add(m,tot);
    pthread_mutex_unlock(&g_lock);
    return p;
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
    void* p=aligned_impl(align,size); if(!p) return 12; *m=p; return 0;
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
