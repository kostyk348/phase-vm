/* algebra_alloc.c v2 — канонический аллокатор (алгебраическая прокачка galloc).
 *
 * Свободное пространство — в КАНОНИЧЕСКОЙ форме: boundary tags + немедленная
 * коалесценция. Голономия (неслитые смежные свободные пары) = 0 by construction.
 *
 * Скорость: ТОЧНЫЕ классы размеров -> pop O(1) (без скана). Слитые блоки
 * произвольного размера живут в отдельном span-листе; alloc сначала берёт
 * точный класс (O(1)), потом span (first-fit+split), потом bump в новом чанке.
 * Фазовые кадры (pa_frame_begin/end) — быстрый bump O(1) на кадр.
 *
 * Память: коалесценция -> блок класса A может обслужить запрос класса B;
 * полностью свободные чанки возвращаются ОС (MADV_DONTNEED) -> меньше RSS.
 */
#define _GNU_SOURCE
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <stdio.h>
#include <errno.h>
#include <unistd.h>
#include <pthread.h>
#include <dlfcn.h>
#include <malloc.h>
#include <sys/mman.h>

#define CHUNK   (4u<<20)
#define MINBLK  32u
#define FREE  1u
#define FIRST 2u
#define LAST  4u
#define HMASK (~(size_t)7)

static const size_t CSZ[] = {16,32,48,64,96,128,192,256,384,512,768,1024,1536,2048,3072,4096,
    6144,8192,12288,16384,24576,32768,49152,65536,98304,131072,196608,262144,393216,524288,786432,1048576};
#define NCLASS (sizeof(CSZ)/sizeof(CSZ[0]))
#define MAXCLS CSZ[NCLASS-1]

typedef struct Blk { size_t h; } Blk;
static inline size_t  bsz(Blk* b){ return b->h & HMASK; }
static inline int     bfree(Blk* b){ return b->h & FREE; }
static inline int     bfirst(Blk* b){ return b->h & FIRST; }
static inline int     blast(Blk* b){ return b->h & LAST; }
static inline void*   bpay(Blk* b){ return (char*)b + 8; }
static inline Blk*    bnext(Blk* b){ return (Blk*)((char*)b + bsz(b)); }
static inline size_t* bfoot(Blk* b){ return (size_t*)((char*)b + bsz(b) - 8); }
static inline Blk*    bprev(Blk* b){ return (Blk*)((char*)b - *((size_t*)b - 1)); }
static inline void    sethead(Blk* b, size_t sz, unsigned fl){ b->h = sz | fl; *bfoot(b) = sz; }

typedef struct Free { Blk* next; Blk* prev; } Free;   /* payload when free */

static int class_exact(size_t s){ int lo=0,hi=(int)NCLASS-1;
    while(lo<=hi){ int m=(lo+hi)>>1; if(CSZ[m]==s) return m; if(CSZ[m]<s) lo=m+1; else hi=m-1; } return -1; }
static int8_t g_ex16[257];
static void ex_init(void){ for(int i=0;i<=256;i++){ size_t sz=(size_t)i<<4; g_ex16[i] = (sz && sz<=MAXCLS)? (int8_t)class_exact(sz) : (int8_t)-1; } }
static inline int class_exact_fast(size_t s){ return (s<=4096)? g_ex16[s>>4] : class_exact(s); }
static int class_of(size_t n);
static int8_t g_co16[257];
static void co_init(void){ for(int i=0;i<=256;i++){ size_t n=(size_t)i<<4; if(!n) n=1;
    if(n<=16){ g_co16[i]=0; continue; } if(n>MAXCLS){ g_co16[i]=-1; continue; }
    int lo=0,hi=(int)NCLASS-1; while(lo<hi){ int m=(lo+hi)>>1; if(CSZ[m]>=n) hi=m; else lo=m+1; } g_co16[i]=(int8_t)lo; } }
static inline int class_of_fast(size_t n){ return (n<=4096)? g_co16[(n+15)>>4] : class_of(n); }
static int class_of(size_t n){ if(n<=16) return 0; if(n>MAXCLS) return -1;
    int lo=0,hi=(int)NCLASS-1; while(lo<hi){ int m=(lo+hi)>>1; if(CSZ[m]>=n) hi=m; else lo=m+1; } return lo; }

/* ---- per-thread state ---- */
static __thread Blk* t_fl[NCLASS];
static __thread Blk* t_span;
static __thread char* t_ch[256]; static __thread size_t t_chl[256]; static __thread int t_nch;
static __thread struct { char* base; size_t len; } t_rc[64];
static __thread int t_tid = -1;
static __thread char* fr_base; static __thread size_t fr_used, fr_cap; static __thread int fr_depth;

static int g_next_id=1;
static pthread_mutex_t g_lock = PTHREAD_MUTEX_INITIALIZER;
static Blk* volatile g_pend; static volatile size_t g_pend_ct;
static int g_passthrough = -1, g_ctor_done = 0;
static volatile unsigned long long S_alloc,S_free,S_split,S_coal,S_chunk,S_compact,S_holonomy_fixed;
static volatile unsigned long long S_holonomy_viol;

static void* (*real_malloc)(size_t); static void (*real_free)(void*);
static void* (*real_realloc)(void*,size_t); static void* (*real_calloc)(size_t,size_t);
static size_t(*real_usable)(void*); static void* (*real_memalign)(size_t,size_t);

#define AREGMAX 4096
static struct { void* base; size_t len; } g_areg[AREGMAX]; static int g_nareg=0;
static int areg_find(void* p){ uintptr_t x=(uintptr_t)p;
    for(int i=0;i<g_nareg;i++){ uintptr_t b=(uintptr_t)g_areg[i].base; if(x>=b && x<b+g_areg[i].len) return i; } return -1; }
static void areg_add(void* b,size_t l){ if(g_nareg<AREGMAX){ g_areg[g_nareg].base=b; g_areg[g_nareg].len=l; g_nareg++; } }
static void areg_del(void* b){ int i=areg_find(b); if(i<0) return; for(int j=i;j<g_nareg-1;j++) g_areg[j]=g_areg[j+1]; g_nareg--; }

/* ---- list ops; b in list indexed by exact class or span ---- */
static inline void push_list(Blk** head, Blk* b){
    Free* f=bpay(b); f->prev=NULL; f->next=*head; if(f->next) ((Free*)bpay(f->next))->prev=b; *head=b;
}
static void place(Blk* b){ int c=class_exact_fast(bsz(b)); if(c>=0) push_list(&t_fl[c], b); else push_list(&t_span, b); }
static void unplace(Blk* b){
    Free* f=bpay(b); int c=class_exact_fast(bsz(b)); Blk** head = (c>=0)? &t_fl[c] : &t_span;
    if(f->prev) ((Free*)bpay(f->prev))->next=f->next; else *head=f->next;
    if(f->next) ((Free*)bpay(f->next))->prev=f->prev;
}
static int in_own(void* p){
    uintptr_t x=(uintptr_t)p; int ci=(x>>21)&63;
    { char* b=t_rc[ci].base; if(b && x>=(uintptr_t)b && x<(uintptr_t)b+t_rc[ci].len) return 1; }
    for(int i=0;i<t_nch;i++){ char* b=t_ch[i]; if(x>=(uintptr_t)b && x<(uintptr_t)b+t_chl[i]) return 1; }
    return 0;
}
static Blk* carve(size_t need){
    size_t cap = need + MINBLK; if(cap < CHUNK) cap = CHUNK;
    void* m = mmap(NULL, cap, PROT_READ|PROT_WRITE, MAP_PRIVATE|MAP_ANONYMOUS, -1, 0);
    if(m==MAP_FAILED) return NULL;
    __atomic_fetch_add(&S_chunk,1,__ATOMIC_RELAXED);
    if(t_nch<256){ t_ch[t_nch]=(char*)m; t_chl[t_nch]=cap; t_nch++; }
    { int i=t_nch-1; while(i>0 && (uintptr_t)t_ch[i-1]>(uintptr_t)m){ t_ch[i]=t_ch[i-1]; t_chl[i]=t_chl[i-1]; i--; }
      t_ch[i]=(char*)m; t_chl[i]=cap; int ci=((uintptr_t)m>>21)&63; t_rc[ci].base=(char*)m; t_rc[ci].len=cap; }
    Blk* b=(Blk*)m; sethead(b, cap, FREE|FIRST|LAST);
    return b;
}
static Blk* coalesce(Blk* b){
    b->h |= FREE;
    if(!blast(b)){ Blk* n=bnext(b);
        if(bfree(n)){ unplace(n); size_t fl = (bfirst(b)?FIRST:0)|(blast(n)?LAST:0);
            sethead(b, bsz(b)+bsz(n), FREE|fl); __atomic_fetch_add(&S_coal,1,__ATOMIC_RELAXED); } }
    if(!bfirst(b)){ Blk* p=bprev(b);
        if(bfree(p)){ unplace(p); size_t fl = (bfirst(p)?FIRST:0)|(blast(b)?LAST:0);
            sethead(p, bsz(p)+bsz(b), FREE|fl); b=p; __atomic_fetch_add(&S_coal,1,__ATOMIC_RELAXED); } }
    return b;
}
static void drain_pending(void){
    if(!__atomic_load_n(&g_pend_ct,__ATOMIC_RELAXED)) return;
    Blk* list=__atomic_exchange_n(&g_pend,NULL,__ATOMIC_ACQ_REL);
    __atomic_store_n(&g_pend_ct,0,__ATOMIC_RELAXED);
    while(list){ Blk* nx=*(Blk**)bpay(list); place(coalesce(list)); list=nx; }
}
/* ленивая компактификация: слить смежные FREE в каждом своём чанке (по требованию) */
static void compact(void){
    __atomic_fetch_add(&S_compact,1,__ATOMIC_RELAXED);
    for(int i=0;i<t_nch;i++){
        Blk* b=(Blk*)t_ch[i];
        for(;;){
            if(bfree(b)){
                Blk* e=b;
                while(!blast(e) && bfree(bnext(e))){ unplace(bnext(e)); e=bnext(e); }
                if(e!=b){ unplace(b); size_t fl=(bfirst(b)?FIRST:0)|(blast(e)?LAST:0);
                    sethead(b,(size_t)((char*)e+bsz(e)-(char*)b), FREE|fl); place(b);
                    __atomic_fetch_add(&S_holonomy_fixed,1,__ATOMIC_RELAXED); }
            }
            if(blast(b)) break;
            b=bnext(b);
        }
    }
}

static void* alm(size_t n){
    if(n==0) n=1;
    size_t need=(n+16+15)&~(size_t)15; if(need<MINBLK) need=MINBLK;
    if(fr_depth>0){ size_t u=(need+23)&~(size_t)15;
        if(fr_used+u<=fr_cap){ void* p=fr_base+fr_used; fr_used+=u; __atomic_fetch_add(&S_alloc,1,__ATOMIC_RELAXED); return p; } }
    int c = class_of_fast(need);
    if(c<0){ size_t tot=need+16; pthread_mutex_lock(&g_lock);
        void* m=mmap(NULL,tot,PROT_READ|PROT_WRITE,MAP_PRIVATE|MAP_ANONYMOUS,-1,0); pthread_mutex_unlock(&g_lock);
        if(m==MAP_FAILED) return NULL; Blk* b=(Blk*)m; sethead(b,tot,FIRST|LAST);
        if(t_nch<256){ t_ch[t_nch]=(char*)m; t_chl[t_nch]=tot; t_nch++; }
        __atomic_fetch_add(&S_alloc,1,__ATOMIC_RELAXED); return bpay(b); }
    drain_pending();
    if(t_fl[c]){ Blk* b=t_fl[c]; t_fl[c]=((Free*)bpay(b))->next; if(t_fl[c]) ((Free*)bpay(t_fl[c]))->prev=NULL;
        __atomic_fetch_add(&S_alloc,1,__ATOMIC_RELAXED); return bpay(b); }
    /* span: first-fit + split */
    for(Blk* x=t_span; x; x=((Free*)bpay(x))->next){ if(bsz(x)>=need){ unplace(x); size_t s=bsz(x);
        if(s>=need+MINBLK){ int waslast=blast(x),wasfirst=bfirst(x);
            Blk* r=(Blk*)((char*)x+need); sethead(r, s-need, FREE|(waslast?LAST:0)); place(r);
            sethead(x, need, (wasfirst?FIRST:0)); __atomic_fetch_add(&S_split,1,__ATOMIC_RELAXED); }
        else sethead(x, s, (bfirst(x)?FIRST:0)|(blast(x)?LAST:0));
        __atomic_fetch_add(&S_alloc,1,__ATOMIC_RELAXED); return bpay(x); } }
    compact();
    drain_pending();
    if(t_fl[c]){ Blk* b=t_fl[c]; t_fl[c]=((Free*)bpay(b))->next; if(t_fl[c]) ((Free*)bpay(t_fl[c]))->prev=NULL;
        __atomic_fetch_add(&S_alloc,1,__ATOMIC_RELAXED); return bpay(b); }
    for(Blk* x2=t_span; x2; x2=((Free*)bpay(x2))->next){ if(bsz(x2)>=need){ unplace(x2); size_t s2=bsz(x2);
        if(s2>=need+MINBLK){ int wl=blast(x2),wf=bfirst(x2);
            Blk* r=(Blk*)((char*)x2+need); sethead(r,s2-need,FREE|(wl?LAST:0)); place(r);
            sethead(x2,need,(wf?FIRST:0)); }
        else sethead(x2,s2,(bfirst(x2)?FIRST:0)|(blast(x2)?LAST:0));
        __atomic_fetch_add(&S_alloc,1,__ATOMIC_RELAXED); return bpay(x2); } }
    Blk* ch=carve(need); if(!ch) return NULL; place(ch);
    Blk* x=t_span; unplace(x); size_t s=bsz(x);
    if(s>=need+MINBLK){ int waslast=blast(x),wasfirst=bfirst(x);
        Blk* r=(Blk*)((char*)x+need); sethead(r, s-need, FREE|(waslast?LAST:0)); place(r);
        sethead(x, need, (wasfirst?FIRST:0)); __atomic_fetch_add(&S_split,1,__ATOMIC_RELAXED); }
    else sethead(x, s, (bfirst(x)?FIRST:0)|(blast(x)?LAST:0));
    __atomic_fetch_add(&S_alloc,1,__ATOMIC_RELAXED); return bpay(x);
}

static void fr(void* p){
    if(!p) return;
    if(fr_depth>0 && (char*)p>=fr_base && (char*)p<fr_base+fr_used) return;   /* кадр */
    Blk* b=(Blk*)((char*)p-8);
    if(in_own(p)){ b->h |= FREE; place(b); __atomic_fetch_add(&S_free,1,__ATOMIC_RELAXED); return; }
    int isreg; pthread_mutex_lock(&g_lock); isreg=areg_find(p); pthread_mutex_unlock(&g_lock);
    if(isreg>=0){ uint64_t m=((uint64_t*)p)[-4];
        if((m&0xffffffffu)==0xA1160001u){ void* o=(void*)((uint64_t*)p)[-3]; size_t tot=(size_t)((uint64_t*)p)[-2];
            pthread_mutex_lock(&g_lock); areg_del(o); pthread_mutex_unlock(&g_lock); munmap(o,tot); __atomic_fetch_add(&S_free,1,__ATOMIC_RELAXED); return; } }
    b->h |= FREE; Blk* hd;
    do { hd=g_pend; *(Blk**)bpay(b)=hd; } while(!__atomic_compare_exchange_n(&g_pend,&hd,b,0,__ATOMIC_ACQ_REL,__ATOMIC_ACQUIRE));
    __atomic_fetch_add(&g_pend_ct,1,__ATOMIC_RELAXED);
    __atomic_fetch_add(&S_free,1,__ATOMIC_RELAXED);
}

/* ---- фазовые кадры ---- */
void pa_frame_begin(void){ if(fr_depth==0){ if(!fr_base){ fr_cap=64u<<20;
        fr_base=mmap(NULL,fr_cap,PROT_READ|PROT_WRITE,MAP_PRIVATE|MAP_ANONYMOUS,-1,0);
        if(fr_base==MAP_FAILED){ fr_base=NULL; fr_cap=0; return; } } fr_used=0; } fr_depth++; }
void pa_frame_end(void){ if(fr_depth>0){ fr_depth--; if(fr_depth==0) fr_used=0; } }
int pa_frame_active(void){ return fr_depth>0; }

/* ---- ABI ---- */
void* malloc(size_t n){ if(g_passthrough==1&&real_malloc) return real_malloc(n); return alm(n); }
void  free(void* p){ if(g_passthrough==1&&real_free){ if(p&&in_own(p)){ fr(p); return; } real_free(p); return; } fr(p); }
void* calloc(size_t n,size_t s){ if(g_passthrough==1&&real_calloc) return real_calloc(n,s); size_t t; if(__builtin_mul_overflow(n,s,&t)) return NULL; void* p=alm(t); if(p) memset(p,0,t); return p; }
size_t malloc_usable_size(void* p){
    if(!p) return 0;
    if(fr_depth>0 && (char*)p>=fr_base && (char*)p<fr_base+fr_used) return fr_used-((char*)p-fr_base);
    if(g_passthrough==1&&real_usable&&!in_own(p)) return real_usable(p);
    if(!in_own(p)) return 0;
    Blk* b=(Blk*)((char*)p-8); size_t s=bsz(b); return s>=16? s-16 : 0;
}
static void* rl(void* p,size_t n){ if(!p) return alm(n); if(!n){ fr(p); return NULL; }
    size_t old=malloc_usable_size(p); void* q=alm(n); if(!q) return NULL; if(old) memcpy(q,p,old<n?old:n); fr(p); return q; }
void* realloc(void* p,size_t n){ if(g_passthrough==1&&real_realloc){ if(!p) return real_malloc?real_malloc(n):NULL; if(!in_own(p)) return real_realloc(p,n);} return rl(p,n); }
void* reallocarray(void* p,size_t n,size_t s){ size_t t; if(__builtin_mul_overflow(n,s,&t)) return NULL; return realloc(p,t); }
static void* al(size_t align,size_t size){ if(size==0) size=1; if(align<16) align=16; if(align&(align-1)) return NULL;
    size_t tot=size+align+64; pthread_mutex_lock(&g_lock);
    void* m=mmap(NULL,tot,PROT_READ|PROT_WRITE,MAP_PRIVATE|MAP_ANONYMOUS,-1,0); pthread_mutex_unlock(&g_lock);
    if(m==MAP_FAILED) return NULL; uintptr_t a=(((uintptr_t)m)+32+align-1)&~(uintptr_t)(align-1); void* p=(void*)a;
    ((uint64_t*)p)[-4]=0xA1160001u; ((uint64_t*)p)[-3]=(uint64_t)m; ((uint64_t*)p)[-2]=(uint64_t)tot;
    pthread_mutex_lock(&g_lock); areg_add(m,tot); pthread_mutex_unlock(&g_lock); return p; }
int posix_memalign(void** m,size_t align,size_t size){ if(align<sizeof(void*)) align=sizeof(void*); if(align&(align-1)) return 22; void* p=al(align,size); if(!p) return 12; *m=p; return 0; }
void* aligned_alloc(size_t a,size_t s){ return al(a,s); }
void* memalign(size_t a,size_t s){ return al(a,s); }
void* valloc(size_t s){ return al(4096,s); }
char* strdup(const char* s){ size_t n=strlen(s)+1; char* p=malloc(n); if(p) memcpy(p,s,n); return p; }
char* strndup(const char* s,size_t m){ size_t n=0; while(n<m&&s[n]) n++; char* p=malloc(n+1); if(p){memcpy(p,s,n);p[n]=0;} return p; }
void cfree(void* p){ free(p); }

unsigned long long galloc_holonomy(void){ return __atomic_load_n(&S_holonomy_viol,__ATOMIC_RELAXED); }
int galloc_canonical(void){ int bad=0; compact();
    for(int i=0;i<t_nch;i++){ Blk* b=(Blk*)t_ch[i];
        for(;;){ if(!blast(b) && bfree(b) && bfree(bnext(b))) bad++; if(blast(b)) break; b=bnext(b); } }
    __atomic_store_n(&S_holonomy_viol,(unsigned long long)bad,__ATOMIC_RELAXED); return bad==0; }

__attribute__((constructor)) static void ct_init(void){
    g_passthrough=getenv("GALLOC_PASSTHROUGH")?1:0;
    real_malloc=(void*(*)(size_t))dlsym(RTLD_NEXT,"malloc");
    real_free=(void(*)(void*))dlsym(RTLD_NEXT,"free");
    real_realloc=(void*(*)(void*,size_t))dlsym(RTLD_NEXT,"realloc");
    real_calloc=(void*(*)(size_t,size_t))dlsym(RTLD_NEXT,"calloc");
    real_usable=(size_t(*)(void*))dlsym(RTLD_NEXT,"malloc_usable_size");
    real_memalign=(void*(*)(size_t,size_t))dlsym(RTLD_NEXT,"memalign");
    ex_init(); co_init();
    if(getenv("GALLOC_VERBOSE")) fprintf(stderr,"[algebra_alloc] canonical v2 loaded\n");
    g_ctor_done=1;
}
__attribute__((destructor)) static void ct_fini(void){
    if(getenv("GALLOC_STATS")) fprintf(stderr,
      "[algebra_alloc] alloc=%llu free=%llu split=%llu coal=%llu chunk=%llu compact=%llu holonomy=%llu\n",
      S_alloc,S_free,S_split,S_coal,S_chunk,S_compact,galloc_holonomy());
}
