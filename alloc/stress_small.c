#define _GNU_SOURCE
#include <malloc.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <pthread.h>
#include <stdint.h>
#include <stdint.h>
#define NT 8
#define ITER 20000
static volatile uint64_t total=0;
static void* worker(void* arg){
    unsigned seed=(unsigned)(uintptr_t)arg;
    uint64_t acc=0;
    for(int i=0;i<ITER;i++){
        size_t n=(rand_r(&seed)%8192)+1;
        int kind=i%7;
        if(kind==0){
            void* p=malloc(n); if(p){ memset(p,(int)(i&0xff),n); acc+=(uint64_t)((unsigned char*)p)[n-1]; free(p);} 
        } else if(kind==1){
            void* p=calloc(n,1); if(p){ acc+=((unsigned char*)p)[0]; free(p);} 
        } else if(kind==2){
            void* p=malloc(16); if(p){ p=realloc(p,n); if(p){ memset(p,1,n); acc+=(uint64_t)((unsigned char*)p)[0]; free(p);} } 
        } else if(kind==3){
            size_t al=(seed&1)?64:4096; void* p=NULL; if(!posix_memalign(&p,al,n)){ memset(p,2,n); acc+= (uint64_t)(((unsigned char*)p)[0]); free(p);} 
        } else if(kind==4){
            char* s=strdup("phase-preload-test"); if(s){ acc+=strlen(s); free(s);} 
        } else if(kind==5){
            void* p=aligned_alloc(32,n); if(p){ memset(p,3,n); acc+=(uint64_t)(((unsigned char*)p)[0]); free(p);} 
        } else {
            void* p=malloc(n); if(p){ size_t u=malloc_usable_size(p); acc+=u>=n?1:0; free(p);} 
        }
    }
    __atomic_fetch_add(&total,acc,__ATOMIC_RELAXED);
    return NULL;
}
int main(void){
    pthread_t th[NT];
    for(int i=0;i<NT;i++) pthread_create(&th[i],NULL,worker,(void*)(uintptr_t)(1000+i));
    for(int i=0;i<NT;i++) pthread_join(th[i],NULL);
    printf("stress ok, checksum=%llu\n",(unsigned long long)total);
    return 0;
}
