/* phase_alloc v5 — MT-safe LD_PRELOAD аллокатор.
 * Hot-path БЕЗ глобального лока: свои блоки (регионы этого потока)
 * malloc/free/usable не берут lock. Глобальный lock — только для
 * чужих/больших/aligned/новых регионов.
 * Диагностика env: GALLOC_DIAG=1 (лог foreign-free), GALLOC_LEAK_FOREIGN=1
 * (чужие free НЕ форвардить в glibc, а терять — для движков со своим
 * аллокатором). Полный POSIX ABI, выравнивание 16, фазовые кадры.
 */
#define _GNU_SOURCE
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <stdio.h>
#include <errno.h>
#include <time.h>
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
static Arena g_pending[NCLASS];               /* чужие (под lock) */
static __thread Arena t_a[NCLASS];            /* приватные арены потока */
static __thread int t_id = -1;
static int g_next_id = 1;
static pthread_mutex_t g_lock;
static volatile void* lowner = 0;
static __thread int holding = 0;

static void glock(void){
    if(holding){ fprintf(stderr,"[galloc] SELF-RELOCK ra=%p\n",__builtin_return_address(0)); abort(); }
    const char* w = getenv("GALLOC_LOCKWARN_MS");
    if(w){
        long ms = atol(w);
        struct timespec ts; clock_gettime(CLOCK_REALTIME,&ts);
        ts.tv_sec += ms/1000; ts.tv_nsec += (ms%1000)*1000000L;
        if(ts.tv_nsec >= 1000000000L){ ts.tv_sec++; ts.tv_nsec -= 1000000000L; }
        int r = pthread_mutex_timedlock(&g_lock,&ts);
        if(r == ETIMEDOUT){ fprintf(stderr,"[galloc] LOCK WAIT >%ldms ra=%p\n", ms, __builtin_return_address(0)); pthread_mutex_lock(&g_lock); }
        else if(r == EOWNERDEAD) pthread_mutex_consistent(&g_lock);
    } else {
        int r = pthread_mutex_lock(&g_lock);
        if(r == EOWNERDEAD) pthread_mutex_consistent(&g_lock);
    }
    holding = 1; lowner = (void*)pthread_self();
}
static void gunlock(void){ holding = 0; lowner = 0; pthread_mutex_unlock(&g_lock); }

#define REGMAX 65536
typedef struct { void* base; size_t len; } Reg;
static Reg regs[REGMAX];
static int nreg = 0;

/* per-thread bump-регионы — проверка владения БЕЗ лока */
static __thread struct { void* base; size_t len; } treg[128];
static __thread int ntre = 0;
static int in_own(void* p){
    char* q = (char*)p;
    for(int i=0;i<ntre;i++){ char* b=(char*)treg[i].base; if(q>=b && q<b+treg[i].len) return 1; }
    return 0;
}

static int class_of(size_t n){ for(size_t i=0;i<NCLASS;i++) if(n<=CLASS_SZ[i]) return (int)i; return -1; }
static void reg_add(void* b, size_t l){ if(nreg<REGMAX){ regs[nreg].base=b; regs[nreg].len=l; nreg++; } }
static void reg_del(void* b){ for(int i=0;i<nreg;i++) if(regs[i].base==b){ regs[i]=regs[nreg-1]; nreg--; return; } }
static int reg_find(void* p){ uintptr_t x=(uintptr_t)p; for(int i=0;i<nreg;i++){ uintptr_t b=(uintptr_t)regs[i].base; if(x>=b && x<b+regs[i].len) return i; } return -1; }
static int my_id(void){ if(t_id<0){ glock(); if(t_id<0) t_id=g_next_id++; gunlock(); } return t_id; }

static void* pop_node(void** head){
    void* p = *head;
    if(p){ *head = *(void**)p; *((uint64_t*)((char*)p-16)) &= ~FREE_BIT; }
    return p;
}

/* новый bump-регион потока: mmap без лока; в per-thread + глобальный реестр */
static void* bump(Arena* a, size_t n){
    if(a->off+n > a->cap){
        size_t cap = 1u<<20;
        void* m = mmap(NULL, cap, PROT_READ|PROT_WRITE, MAP_PRIVATE|MAP_ANONYMOUS, -1, 0);
        if(m == MAP_FAILED) return NULL;
        a->base = m; a->cap = cap; a->off = 0;
        if(ntre < 128){ treg[ntre].base = m; treg[ntre].len = cap; ntre++; }
        glock(); reg_add(m, cap); gunlock();
    }
    void* p = (char*)a->base + a->off; a->off += n; return p;
}

/* ---------- фазовые кадры (per-thread) ---------- */
static __thread int fr_depth = 0;
static __thread void* fr_base = NULL;
static __thread size_t fr_used = 0, fr_cap = 0;
static pthread_key_t key;
static pthread_once_t key_once = PTHREAD_ONCE_INIT;
static void bump_dtor(void* p){ if(p) munmap(p, (size_t)64u<<20); }
static void key_init(void){ pthread_key_create(&key, bump_dtor); }

void pa_frame_begin(void){
    pthread_once(&key_once, key_init);
    if(fr_depth == 0){
        if(!fr_base){ fr_cap = 64u<<20;
            fr_base = mmap(NULL, fr_cap, PROT_READ|PROT_WRITE, MAP_PRIVATE|MAP_ANONYMOUS, -1, 0);
            if(fr_base == MAP_FAILED){ fr_base = NULL; return; }
            pthread_setspecific(key, fr_base); }
        fr_used = 0;
    }
    fr_depth++;
}
void pa_frame_end(void){ if(fr_depth>0){ fr_depth--; if(fr_depth==0) fr_used=0; } }
int pa_frame_active(void){ return fr_depth>0; }

/* ---------- malloc ---------- */
static void* phase_malloc(size_t n){
    if(n == 0) n = 1;
    if(fr_depth > 0){
        size_t need = (n+15)&~(size_t)15; if(need == 0) need = 16;
        if(fr_used+need <= fr_cap){ void* p=(char*)fr_base+fr_used; fr_used+=need; return p; }
    }
    int id = my_id();
    int c = class_of(n);
    if(c >= 0){
        Arena* a = &t_a[c];
        if(a->free_head){ return pop_node(&a->free_head); }
        /* редко: забрать чужие (pending) */
        glock();
        if(g_pending[c].free_head){ a->free_head = g_pending[c].free_head; g_pending[c].free_head = NULL; }
        gunlock();
        if(a->free_head){ return pop_node(&a->free_head); }
        void* p = bump(a, CLASS_SZ[c]+16);
        if(!p) return NULL;
        ((uint64_t*)p)[0] = CLASS_MAGIC|((uint64_t)c<<32);
        ((uint64_t*)p)[1] = (uint32_t)id;
        return (char*)p+16;
    }
    /* крупный */
    size_t tot = n+16;
    glock();
    void* m = mmap(NULL, tot, PROT_READ|PROT_WRITE, MAP_PRIVATE|MAP_ANONYMOUS, -1, 0);
    if(m != MAP_FAILED){ ((uint64_t*)m)[0]=CLASS_MAGIC|LARGE_BIT; ((uint64_t*)m)[1]=tot; reg_add(m,tot); }
    gunlock();
    return m == MAP_FAILED ? NULL : (char*)m+16;
}

/* ---------- free ---------- */
static void (*real_free)(void*) = NULL;
static int real_free_tried = 0;

static void phase_free(void* p){
    if(!p) return;
    if(fr_depth>0 && (char*)p>=(char*)fr_base && (char*)p<(char*)fr_base+fr_used) return;
    int me = my_id();
    /* БЫСТРО: блок из региона этого потока — без лока */
    if(in_own(p)){
        uint64_t h = *(uint64_t*)((char*)p-16);
        if((h&0xffffffffu)==CLASS_MAGIC && !(h&LARGE_BIT) && !(h&FREE_BIT)){
            int c = (int)((h>>32)&0xff);
            *((uint64_t*)((char*)p-16)) = CLASS_MAGIC|((uint64_t)c<<32)|FREE_BIT;
            if(c>=0 && c<(int)NCLASS){ *(void**)p = t_a[c].free_head; t_a[c].free_head = p; }
            return;
        }
    }
    /* МЕДЛЕННО: чужой/большой/aligned */
    glock();
    if(reg_find(p) < 0){
        gunlock();
        static int diag=0, leak=0; static unsigned long long fn=0; fn++;
        if(!diag && getenv("GALLOC_DIAG")) diag=1;
        if(!leak && getenv("GALLOC_LEAK_FOREIGN")) leak=1;
        if(diag && (fn%2000)==0) fprintf(stderr,"[galloc] foreign-free n=%llu p=%p\n", fn, p);
        if(leak) return; /* чужой аллокатор движка: не форвардим */
        /* real_free уже резолвнут в конструкторе: НЕ вызываем dlsym в hot-path
           (dlopen loader-lock reentrancy -> дедлок на загрузке ассетов) */
        if(real_free) real_free(p);
        return;
    }
    uint64_t h = *(uint64_t*)((char*)p-16);
    if((h&0xffffffffu) == CLASS_MAGIC){
        if(h & LARGE_BIT){ void* base=(char*)p-16; size_t tot=(size_t)*(uint64_t*)((char*)p-8);
            reg_del(base); gunlock(); munmap(base,tot); return; }
        int c = (int)((h>>32)&0xff); uint32_t own = (uint32_t)*(uint64_t*)((char*)p-8);
        if(c>=0 && c<(int)NCLASS && !(h&FREE_BIT)){
            *((uint64_t*)((char*)p-16)) = CLASS_MAGIC|((uint64_t)c<<32)|FREE_BIT;
            if(own == (uint32_t)me){ *(void**)p = t_a[c].free_head; t_a[c].free_head = p; }
            else { *(void**)p = g_pending[c].free_head; g_pending[c].free_head = p; }
        }
        gunlock(); return;
    }
    uint64_t am = *(uint64_t*)((char*)p-32);
    if((am&0xffffffffu) == ALIGN_MAGIC){
        void* o = (void*)*(uint64_t*)((char*)p-24); size_t tot = (size_t)*(uint64_t*)((char*)p-16);
        reg_del(o); gunlock(); munmap(o,tot); return;
    }
    gunlock();
}

static size_t phase_usable(void* p){
    if(!p) return 0;
    if(fr_depth>0 && (char*)p>=(char*)fr_base && (char*)p<(char*)fr_base+fr_used) return fr_used-((char*)p-(char*)fr_base);
    if(in_own(p)){
        uint64_t h = *(uint64_t*)((char*)p-16);
        if((h&0xffffffffu)==CLASS_MAGIC && !(h&LARGE_BIT)){
            int c = (int)((h>>32)&0xff);
            return c>=0 && c<(int)NCLASS ? CLASS_SZ[c] : 0;
        }
    }
    size_t r = 0;
    glock();
    if(reg_find(p) >= 0){ uint64_t h=*(uint64_t*)((char*)p-16);
        if((h&0xffffffffu)==CLASS_MAGIC){
            if(h&LARGE_BIT) r = (size_t)*(uint64_t*)((char*)p-8) - 16;
            else { int c=(int)((h>>32)&0xff); r = (c>=0 && c<(int)NCLASS)? CLASS_SZ[c]:0; } } }
    gunlock();
    return r;
}

static void* aligned_impl(size_t align, size_t size){
    if(size == 0) size = 1;
    if(align < 16) align = 16;
    if(align & (align-1)) return NULL;
    size_t tot = size + align + 64;
    glock();
    void* m = mmap(NULL, tot, PROT_READ|PROT_WRITE, MAP_PRIVATE|MAP_ANONYMOUS, -1, 0);
    if(m == MAP_FAILED){ gunlock(); return NULL; }
    uintptr_t a = (((uintptr_t)m)+32+align-1) & ~(uintptr_t)(align-1);
    void* p = (void*)a;
    ((uint64_t*)p)[-4] = ALIGN_MAGIC;
    ((uint64_t*)p)[-3] = (uint64_t)m;
    ((uint64_t*)p)[-2] = (uint64_t)tot;
    reg_add(m, tot);
    gunlock();
    return p;
}

/* ---------- ABI ---------- */
void* malloc(size_t n){ return phase_malloc(n); }
void free(void* p){ phase_free(p); }
void* calloc(size_t n,size_t s){ size_t t; if(__builtin_mul_overflow(n,s,&t)) return NULL; void* p=phase_malloc(t); if(p) memset(p,0,t); return p; }
void* realloc(void* p,size_t n){
    if(!p) return phase_malloc(n);
    if(!n){ phase_free(p); return NULL; }
    size_t old = phase_usable(p);
    void* q = phase_malloc(n);
    if(!q) return NULL;
    if(old) memcpy(q, p, old<n?old:n);
    phase_free(p);
    return q;
}
void* reallocarray(void* p,size_t n,size_t s){ size_t t; if(__builtin_mul_overflow(n,s,&t)) return NULL; return realloc(p,t); }
int posix_memalign(void** m,size_t align,size_t size){
    if(align < sizeof(void*)) align = sizeof(void*);
    if(align & (align-1)) return 22;
    void* p = aligned_impl(align,size); if(!p) return 12; *m = p; return 0;
}
void* aligned_alloc(size_t align,size_t size){ return aligned_impl(align,size); }
void* memalign(size_t align,size_t size){ return aligned_impl(align,size); }
void* valloc(size_t size){ return aligned_impl(4096,size); }
size_t malloc_usable_size(void* p){ return phase_usable(p); }
char* strdup(const char* s){ size_t n=strlen(s)+1; char* p=malloc(n); if(p) memcpy(p,s,n); return p; }
char* strndup(const char* s,size_t m){ size_t n=0; while(n<m && s[n]) n++; char* p=malloc(n+1); if(p){ memcpy(p,s,n); p[n]=0; } return p; }
void cfree(void* p){ phase_free(p); }
void* __libc_malloc(size_t n){ return phase_malloc(n); }
void __libc_free(void* p){ phase_free(p); }
void* __libc_calloc(size_t n,size_t s){ return calloc(n,s); }
void* __libc_realloc(void* p,size_t n){ return realloc(p,n); }
void* __libc_memalign(size_t a,size_t n){ return aligned_impl(a,n); }
void* __libc_valloc(size_t n){ return aligned_impl(4096,n); }

static void fork_child(void){ pthread_mutex_init(&g_lock, NULL); holding=0; lowner=NULL; }
__attribute__((constructor)) static void atfork_init(void){
    if(getenv("GALLOC_VERBOSE")) fprintf(stderr,"[galloc] loaded pid=%d\n",(int)getpid());
    /* резолвим настоящий free заранее — чтобы free() никогда не звал dlsym */
    if(!real_free_tried){ real_free_tried=1; real_free=(void(*)(void*))dlsym(RTLD_NEXT,"free"); }
    pthread_mutexattr_t a; pthread_mutexattr_init(&a);
    pthread_mutexattr_setrobust(&a, PTHREAD_MUTEX_ROBUST);
    pthread_mutex_init(&g_lock, &a);
    pthread_atfork(NULL, NULL, fork_child);
}
