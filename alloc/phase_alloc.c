/* phase_alloc: LD_PRELOAD аллокатор с фазовыми кадрами (O(1) bulk-reset).
 * Режимы:
 *  - обычный: корректный malloc/free/calloc/realloc (классы размеров + mmap);
 *  - фазовый: pa_frame_begin()/pa_frame_end() — выделения внутри кадра идут
 *    из bump-арены (без заголовков, без free-обхода); конец кадра = bulk
 *    reset O(1). Объекты кадра НЕ должны переживать его конец (дисциплина
 *    PHASE: память = пик фазы). free() объектов кадра до конца — no-op.
 * Single-thread POC. */
#define _GNU_SOURCE
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

#define ALIGN 8
#define MAGIC 0xFA5E0001u

/* размерные классы */
static const size_t CLASS_SZ[] = {16,24,32,48,64,96,128,192,256,384,512,768,1024,1536,2048};
#define NCLASS (sizeof(CLASS_SZ)/sizeof(CLASS_SZ[0]))
static const size_t SMALL_MAX = 2048;

static int class_of(size_t n){ for(size_t i=0;i<NCLASS;i++) if(n<=CLASS_SZ[i]) return (int)i; return -1; }

typedef struct { void* base; size_t cap, off; void* free_head; } Arena;

static Arena arenas[NCLASS];

static void* carve(Arena* a, size_t n){
    if(a->free_head){ void* p=a->free_head; a->free_head=*(void**)p; return p; }
    if(a->off+n>a->cap){
        size_t cap = 1u<<20; /* 1 MiB per class-arena */
        void* m = mmap(NULL,cap,PROT_READ|PROT_WRITE,MAP_PRIVATE|MAP_ANONYMOUS,-1,0);
        if(m==MAP_FAILED) return NULL;
        a->base=m; a->cap=cap; a->off=0;
    }
    void* p=(char*)a->base+a->off; a->off+=n; return p;
}

static void* phase_malloc(size_t n){
    if(n==0) n=1;
    /* фазовый кадр активен? */
    extern int __phase_frame_depth;
    extern void* __phase_bump_base; extern size_t __phase_bump_used, __phase_bump_cap;
    if(__phase_frame_depth>0){
        size_t need=(n+ALIGN-1)&~(size_t)(ALIGN-1);
        if(__phase_bump_used+need<=__phase_bump_cap){
            void* p=(char*)__phase_bump_base+__phase_bump_used;
            __phase_bump_used+=need;
            return p;
        }
        /* переполнение bump: обычный путь (объект живёт до явного free) */
    }
    int c=class_of(n);
    if(c>=0){
        void* p=carve(&arenas[c], CLASS_SZ[c]+8);
        if(!p) return NULL;
        *(uint64_t*)p=(uint64_t)(MAGIC|((uint64_t)c<<8));
        return (char*)p+8;
    }
    /* крупный: mmap с заголовком */
    size_t tot=n+32;
    void* m=mmap(NULL,tot,PROT_READ|PROT_WRITE,MAP_PRIVATE|MAP_ANONYMOUS,-1,0);
    if(m==MAP_FAILED) return NULL;
    *(uint64_t*)m=(MAGIC|(1ull<<63)); /* бит mmap */
    *(size_t*)((char*)m+8)=tot;
    return (char*)m+32;
}

static void phase_free(void* p){
    if(!p) return;
    extern int __phase_frame_depth;
    extern void* __phase_bump_base; extern size_t __phase_bump_used;
    if(__phase_frame_depth>0){
        char* b=(char*)__phase_bump_base;
        if(p>=(void*)b && p<(void*)(b+__phase_bump_used)) return; /* объект кадра */
    }
    uint64_t h=*(uint64_t*)((char*)p-8);
    if((h&0xffffffffu)!=MAGIC) return;
    if(h&(1ull<<63)){
        size_t tot=*(size_t*)((char*)p-24);
        munmap((char*)p-32,tot);
        return;
    }
    int c=(int)((h>>8)&0xff);
    if(c<0||c>=(int)NCLASS) return;
    *(void**)p=arenas[c].free_head;
    arenas[c].free_head=p;
}

/* ---------- фазовые кадры ---------- */
int __phase_frame_depth=0;
void* __phase_bump_base=NULL; size_t __phase_bump_used=0, __phase_bump_cap=0;

void pa_frame_begin(void){
    if(__phase_frame_depth==0){
        if(!__phase_bump_base){
            __phase_bump_cap=64u<<20; /* 64 MiB резерв */
            __phase_bump_base=mmap(NULL,__phase_bump_cap,PROT_READ|PROT_WRITE,
                                   MAP_PRIVATE|MAP_ANONYMOUS,-1,0);
            if(__phase_bump_base==MAP_FAILED){ __phase_bump_base=NULL; return; }
        }
        __phase_bump_used=0;
    }
    __phase_frame_depth++;
}
void pa_frame_end(void){
    if(__phase_frame_depth>0){
        __phase_frame_depth--;
        if(__phase_frame_depth==0) __phase_bump_used=0; /* bulk reset O(1) */
    }
}
int pa_frame_active(void){ return __phase_frame_depth>0; }

/* ---------- публичный ABI ---------- */
void* malloc(size_t n){ return phase_malloc(n); }
void free(void* p){ phase_free(p); }
void* calloc(size_t n,size_t s){ size_t t; if(__builtin_mul_overflow(n,s,&t)) return NULL; void* p=phase_malloc(t); if(p) memset(p,0,t); return p; }
void* realloc(void* p,size_t n){ if(!p) return phase_malloc(n); if(!n){ free(p); return NULL; }
    uint64_t h=*(uint64_t*)((char*)p-8); size_t old=0;
    extern int __phase_frame_depth; extern void* __phase_bump_base; extern size_t __phase_bump_used;
    int frame=0;
    if(__phase_frame_depth>0){ char* b=(char*)__phase_bump_base; if(p>=(void*)b&&p<(void*)(b+__phase_bump_used)) frame=1; }
    if(!frame&&(h&0xffffffffu)==MAGIC){ if(h&(1ull<<63)) old=*(size_t*)((char*)p-24)-32; else { int c=(int)((h>>8)&0xff); old=CLASS_SZ[c]; } }
    else old=0;
    void* q=phase_malloc(n); if(!q) return NULL; if(old) memcpy(q,p,old<n?old:n); free(p); return q; }
