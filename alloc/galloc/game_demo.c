/* game_demo.c — типичный игровой кадр на galloc.
 * Per frame: пул-сущности (spawn/update/despawn), фрейм-времена (частицы),
 * редкие долгоживущие (общая куча), иногда aligned. 4 потока-воркера,
 * у каждого свой кадр/пул (TLS). Детерминизм по сумме.
 */
#define _GNU_SOURCE
#include "galloc.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <pthread.h>
#include <stdint.h>
#include <time.h>

static double now(void){ struct timespec t; clock_gettime(CLOCK_MONOTONIC,&t); return t.tv_sec+t.tv_nsec*1e-9; }

#define BULLETS 4096
#define FRAMES  3000
typedef struct { uint64_t x,y,vx,vy; uint8_t life; } Bullet;

static void* worker(void* arg){
    ga_pool* pool = ga_pool_create(sizeof(Bullet),BULLETS);
    Bullet* live[BULLETS];
    int nlive=0;
    unsigned long long sum=0;
    for(int f=0;f<FRAMES;f++){
        ga_frame_begin();
        /* частицы-времена кадра */
        for(int i=0;i<300;i++){ size_t m=ga_frame_mark(); char* tmp=ga_frame_alloc(64); tmp[0]=(char)f; ga_frame_rollback(m); }
        /* спавн пуль */
        int spawn=(f%3==0)?4:0;
        for(int i=0;i<spawn;i++){ Bullet* b=ga_pool_alloc(pool); if(b){ b->x=0;b->y=0;b->vx=f;b->vy=1;b->life=100; live[nlive++]=b; } }
        /* апдейт + деспавн старых */
        for(int i=0;i<nlive;i++){ live[i]->x+=live[i]->vx; live[i]->y+=live[i]->vy; live[i]->life--;
            if(live[i]->life==0 || live[i]->y>100000){ ga_pool_free(pool,live[i]); live[i]=live[nlive-1]; nlive--; i--; } }
        sum += (unsigned long long)(uintptr_t)arg + nlive;
        /* редкое долгоживущее через общую кучу */
        if(f%97==0){ void* p=ga_alloc(4096); memset(p,0,4096); sum+=(uintptr_t)p&1; ga_free(p); }
        if(f%311==0){ void* p=ga_aligned(4096,100); sum+=((uintptr_t)p&4095)==0; ga_free(p); }
        ga_frame_end();
    }
    /* остаток пуль — пул живёт, освобождать не обязательно (destroy) */
    ga_pool_destroy(pool);
    return (void*)(uintptr_t)sum;
}

int main(void){
    enum{NT=4};
    pthread_t th[NT];
    double t0=now();
    for(int i=0;i<NT;i++) pthread_create(&th[i],NULL,worker,(void*)(uintptr_t)(100+i));
    unsigned long long total=0;
    for(int i=0;i<NT;i++){ void* r; pthread_join(th[i],&r); total+=(uintptr_t)r; }
    double dt=now()-t0;
    printf("game_demo: %d воркера x %d кадров, пул %d сущностей, частицы во frame-арене\n",NT,FRAMES,BULLETS);
    printf("время: %.3f s (%.0f ns/кадр/воркер), checksum=%llu\n",dt, dt*1e9/(double)(NT*FRAMES),(unsigned long long)total);
    ga_stats();
    return 0;
}
