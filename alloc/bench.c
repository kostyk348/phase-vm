#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <stdint.h>
extern void pa_frame_begin(void) __attribute__((weak));
extern void pa_frame_end(void) __attribute__((weak));
static double now(void){ struct timespec t; clock_gettime(CLOCK_MONOTONIC,&t); return t.tv_sec+t.tv_nsec*1e-9; }
int main(int argc,char**argv){
    int frames = argc>1 && !strcmp(argv[1],"frames");
    int use_frames = frames && pa_frame_begin;
    int T=50000, PER=200;
    volatile uint64_t sink=0;
    double t0=now();
    for(int tick=0;tick<T;tick++){
        if(use_frames) pa_frame_begin();
        void* p[PER];
        for(int i=0;i<PER;i++){ p[i]=malloc((size_t)(8+((i*7)%96))); if(p[i]) *(volatile uint8_t*)p[i]=(uint8_t)i; sink+= (uintptr_t)p[i]&1; }
        if(!use_frames) for(int i=0;i<PER;i++) free(p[i]);
        if(use_frames) pa_frame_end();
    }
    double dt=now()-t0;
    printf("mode=%s ticks=%d allocs/tick=%d total=%dM time=%.3fs  %.1f ns/alloc\n",
        use_frames?"frames":(frames?"glibc-frames-noop":"malloc/free"), T, PER,
        (T*PER)/1000000, dt, dt*1e9/(double)((long)T*PER));
    return 0;
}
