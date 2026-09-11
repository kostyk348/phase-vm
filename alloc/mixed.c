#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <pthread.h>
#include <stdint.h>
#include <time.h>
#define NT 8
#define ITER 300000
static double now(void){ struct timespec t; clock_gettime(CLOCK_MONOTONIC,&t); return t.tv_sec+t.tv_nsec*1e-9; }
static void* worker(void* a){
    unsigned s=(unsigned)(uintptr_t)a;
    void* live[64]; memset(live,0,sizeof live);
    for(int i=0;i<ITER;i++){
        s=s*1103515245+12345; unsigned r=s>>8;
        size_t n;
        switch(r%4){ case 0: n=16+r%256; break; case 1: n=512+r%8192; break; case 2: n=(r%262144)+4096; break; default: n=(r%1048576)+65536; }
        int slot=r&63;
        if(live[slot]) free(live[slot]);
        void* p=malloc(n); if(p) memset(p,1,(n<256?n:256));
        live[slot]=p;
    }
    for(int i=0;i<64;i++) if(live[i]) free(live[i]);
    return NULL;
}
int main(void){
    pthread_t t[NT]; double t0=now();
    for(long i=0;i<NT;i++) pthread_create(&t[i],NULL,worker,(void*)i);
    for(int i=0;i<NT;i++) pthread_join(t[i],NULL);
    printf("mixed ok %.2fs (%.0f ns/op)\n", now()-t0, (now()-t0)*1e9/((double)NT*ITER));
    return 0;
}
