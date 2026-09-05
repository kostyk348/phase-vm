#include <stdint.h>
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
int main(void){
    void* p=NULL;
    if(posix_memalign(&p,64,100)) return 1;
    if(((uintptr_t)p&63)!=0){ printf("align fail\n"); return 1; }
    memset(p,1,100); free(p);
    void* q=aligned_alloc(4096,50);
    if(((uintptr_t)q&4095)!=0){ printf("aalloc fail\n"); return 1; }
    free(q);
    char* s=strdup("abc"); if(strcmp(s,"abc")){ return 1;} free(s);
    void* r=reallocarray(NULL,100,8);
    if(!r) return 1; free(r);
    printf("api ok\n");
    return 0;
}
