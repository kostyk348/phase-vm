/* galloc.c — универсальный игровой аллокатор (реализация).
 * Механика MT-безопасной кучи из phase_alloc v4 (per-thread арены + private
 * листы + глобальный pending для чужих free) + фрейм-арены + пулы.
 */
#define _GNU_SOURCE
#include "galloc.h"
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <pthread.h>
#include <stdio.h>
#include <sys/mman.h>
#include <dlfcn.h>

#define CLS_MAGIC 0xA11C0001u
#define AL_MAGIC  0xA11C0002u
#define LARGE_BIT (1ull<<63)
#define FREE_BIT  (1ull<<62)

static const size_t CLS[] = {16,32,48,64,96,128,192,256,384,512,768,1024,1536,2048,3072,4096};
#define NCLS (sizeof(CLS)/sizeof(CLS[0]))

typedef struct { void* base; size_t cap, off; void* free_head; } Arena;
static Arena pend[NCLS];
static __thread Arena ta[NCLS];
static __thread int tid = -1;
static int gnext = 1;
static pthread_mutex_t L = PTHREAD_MUTEX_INITIALIZER;

#define RMAX 65536
typedef struct { void* base; size_t len; } Reg;
static Reg regs[RMAX];
static int nreg = 0;

static int cls_of(size_t n){ for(size_t i=0;i<NCLS;i++) if(n<=CLS[i]) return (int)i; return -1; }
static void reg_add(void* b,size_t l){ if(nreg<RMAX){ regs[nreg].base=b; regs[nreg].len=l; nreg++; } }
static void reg_del(void* b){ for(int i=0;i<nreg;i++) if(regs[i].base==b){ regs[i]=regs[nreg-1]; nreg--; return; } }
static int reg_find(void* p){ uintptr_t x=(uintptr_t)p; for(int i=0;i<nreg;i++){ uintptr_t b=(uintptr_t)regs[i].base; if(x>=b&&x<b+regs[i].len) return i; } return -1; }
static int myid(void){ if(tid<0){ pthread_mutex_lock(&L); if(tid<0) tid=gnext++; pthread_mutex_unlock(&L);} return tid; }

static void* pop_node(void** head){
    void* p=*head; if(p){ *head=*(void**)p; *((uint64_t*)((char*)p-16))&=~FREE_BIT; }
    return p;
}
static void* bump(Arena* a,size_t n){
    if(a->off+n>a->cap){
        pthread_mutex_lock(&L);
        if(a->off+n>a->cap){ size_t cap=1u<<20; void* m=mmap(NULL,cap,PROT_READ|PROT_WRITE,MAP_PRIVATE|MAP_ANONYMOUS,-1,0);
            if(m!=MAP_FAILED){ a->base=m; a->cap=cap; a->off=0; reg_add(m,cap);} }
        pthread_mutex_unlock(&L);
        if(a->off+n>a->cap) return NULL;
    }
    void* p=(char*)a->base+a->off; a->off+=n; return p;
}

/* ---------- общая куча ---------- */
void* ga_alloc(size_t n){
    if(n==0)n=1;
    int c=cls_of(n);
    if(c>=0){
        int id=myid(); Arena* a=&ta[c];
        if(a->free_head){ void* p=pop_node(&a->free_head); return p; }
        pthread_mutex_lock(&L);
        if(pend[c].free_head){ a->free_head=pend[c].free_head; pend[c].free_head=NULL; }
        pthread_mutex_unlock(&L);
        if(a->free_head){ void* p=pop_node(&a->free_head); return p; }
        void* p=bump(a,CLS[c]+16); if(!p) return NULL;
        ((uint64_t*)p)[0]=CLS_MAGIC|((uint64_t)c<<32); ((uint64_t*)p)[1]=(uint32_t)id;
        return (char*)p+16;
    }
    size_t tot=n+16;
    pthread_mutex_lock(&L);
    void* m=mmap(NULL,tot,PROT_READ|PROT_WRITE,MAP_PRIVATE|MAP_ANONYMOUS,-1,0);
    if(m!=MAP_FAILED){ ((uint64_t*)m)[0]=CLS_MAGIC|LARGE_BIT; ((uint64_t*)m)[1]=tot; reg_add(m,tot); }
    pthread_mutex_unlock(&L);
    return m==MAP_FAILED? NULL : (char*)m+16;
}

static void (*rf)(void*)=NULL; static int rft=0;
void ga_free(void* p){
    if(!p) return;
    pthread_mutex_lock(&L);
    if(reg_find(p)<0){ pthread_mutex_unlock(&L);
        if(!rft){ rft=1; rf=(void(*)(void*))dlsym(RTLD_NEXT,"free"); } if(rf) rf(p); return; }
    uint64_t h=*(uint64_t*)((char*)p-16);
    if((h&0xffffffffu)==CLS_MAGIC){
        if(h&LARGE_BIT){ void* b=(char*)p-16; size_t t=(size_t)*(uint64_t*)((char*)p-8); reg_del(b);
            pthread_mutex_unlock(&L); munmap(b,t); return; }
        int c=(int)((h>>32)&0xff); uint32_t own=(uint32_t)*(uint64_t*)((char*)p-8);
        if(c>=0&&c<(int)NCLS&&!(h&FREE_BIT)){
            *((uint64_t*)((char*)p-16))=CLS_MAGIC|((uint64_t)c<<32)|FREE_BIT;
            if(own==(uint32_t)myid()){ *(void**)p=ta[c].free_head; ta[c].free_head=p; }
            else { *(void**)p=pend[c].free_head; pend[c].free_head=p; }
        }
        pthread_mutex_unlock(&L); return;
    }
    uint64_t am=*(uint64_t*)((char*)p-32);
    if((am&0xffffffffu)==AL_MAGIC){ void* o=(void*)*(uint64_t*)((char*)p-24); size_t t=(size_t)*(uint64_t*)((char*)p-16);
        reg_del(o); pthread_mutex_unlock(&L); munmap(o,t); return; }
    pthread_mutex_unlock(&L);
}

size_t ga_usable(void* p){
    if(!p) return 0;
    size_t r=0; pthread_mutex_lock(&L);
    if(reg_find(p)>=0){ uint64_t h=*(uint64_t*)((char*)p-16);
        if((h&0xffffffffu)==CLS_MAGIC){ if(h&LARGE_BIT) r=(size_t)*(uint64_t*)((char*)p-8)-16;
            else { int c=(int)((h>>32)&0xff); r=(c>=0&&c<(int)NCLS)?CLS[c]:0; } } }
    pthread_mutex_unlock(&L);
    return r;
}

void* ga_aligned(size_t align,size_t n){
    if(n==0)n=1; if(align<16)align=16; if(align&(align-1)) return NULL;
    size_t tot=n+align+64; pthread_mutex_lock(&L);
    void* m=mmap(NULL,tot,PROT_READ|PROT_WRITE,MAP_PRIVATE|MAP_ANONYMOUS,-1,0);
    if(m==MAP_FAILED){ pthread_mutex_unlock(&L); return NULL; }
    uintptr_t a=(((uintptr_t)m)+32+align-1)&~(uintptr_t)(align-1); void* p=(void*)a;
    ((uint64_t*)p)[-4]=AL_MAGIC; ((uint64_t*)p)[-3]=(uint64_t)m; ((uint64_t*)p)[-2]=(uint64_t)tot;
    reg_add(m,tot); pthread_mutex_unlock(&L);
    return p;
}
void* ga_realloc(void* p,size_t n){
    if(!p) return ga_alloc(n); if(!n){ ga_free(p); return NULL; }
    size_t old=ga_usable(p); void* q=ga_alloc(n); if(!q) return NULL;
    if(old) memcpy(q,p,old<n?old:n); ga_free(p); return q;
}

/* ---------- фрейм-арена ---------- */
static __thread int fd=0; static __thread void* fb=NULL; static __thread size_t fu=0,fc=0;
static pthread_key_t key; static pthread_once_t ok=PTHREAD_ONCE_INIT;
static void kd(void* p){ if(p) munmap(p,(size_t)64u<<20); }
static void ki(void){ pthread_key_create(&key,kd); }

void ga_frame_begin(void){
    pthread_once(&ok,ki);
    if(fd==0){ if(!fb){ fc=64u<<20; fb=mmap(NULL,fc,PROT_READ|PROT_WRITE,MAP_PRIVATE|MAP_ANONYMOUS,-1,0);
        if(fb==MAP_FAILED){ fb=NULL; return; } pthread_setspecific(key,fb);} fu=0; }
    fd++;
}
void ga_frame_end(void){ if(fd>0){ fd--; if(fd==0) fu=0; } }
size_t ga_frame_mark(void){ return fu; }
void ga_frame_rollback(size_t m){ fu=m; }
void* ga_frame_alloc(size_t n){ size_t need=(n+15)&~(size_t)15; if(need==0)need=16;
    if(fu+need<=fc){ void* p=(char*)fb+fu; fu+=need; return p; }
    /* переполнение фрейма: общая куча */
    return ga_alloc(n);
}

/* ---------- пулы ---------- */
struct ga_pool { size_t elem; int cap, used; void** free_list; void* base; pthread_mutex_t m; };

ga_pool* ga_pool_create(size_t elem,int cap){
    size_t es=(elem+15)&~(size_t)15;
    size_t tot=(size_t)cap*es;
    void* base=mmap(NULL,tot,PROT_READ|PROT_WRITE,MAP_PRIVATE|MAP_ANONYMOUS,-1,0);
    if(base==MAP_FAILED) return NULL;
    ga_pool* p=ga_alloc(sizeof(ga_pool));
    p->elem=es; p->cap=cap; p->used=0; p->base=base; pthread_mutex_init(&p->m,NULL);
    /* свободный список = все блоки */
    p->free_list=ga_alloc((size_t)cap*sizeof(void*));
    for(int i=0;i<cap;i++) p->free_list[i]=(char*)base+(size_t)i*es;
    return p;
}
void* ga_pool_alloc(ga_pool* p){
    pthread_mutex_lock(&p->m);
    void* o=NULL;
    if(p->used<p->cap) o=p->free_list[p->used++];
    pthread_mutex_unlock(&p->m);
    return o;
}
void ga_pool_free(ga_pool* p,void* o){
    pthread_mutex_lock(&p->m);
    if(p->used>0) p->free_list[--p->used]=o;
    pthread_mutex_unlock(&p->m);
}
void ga_pool_destroy(ga_pool* p){
    munmap(p->base,(size_t)p->cap*p->elem);
    ga_free(p->free_list);
    ga_free(p);
}

void ga_stats(void){
    fprintf(stderr,"galloc: nreg=%d (регионов 1MB+)\n",nreg);
}
