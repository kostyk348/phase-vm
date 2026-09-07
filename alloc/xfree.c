#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <pthread.h>
#include <stdint.h>
#define NT 4
#define PER 200000
static void* slots[NT][PER];
static void* worker(void* arg){
    long me=(long)arg;
    unsigned seed=(unsigned)(0x1000+me);
    /* фаза 1: аллоцируем свои */
    for(int i=0;i<PER;i++){ size_t n=(seed*31+i*7)%800+16; slots[me][i]=malloc(n); if(slots[me][i]) *(char*)slots[me][i]=(char)i; }
    /* фаза 2: перекрёстный free: освобождаем чужие блоки */
    long victim=(me+1)%NT;
    for(int i=0;i<PER;i++){ free(slots[victim][i]); }
    return NULL;
}
int main(void){
    pthread_t th[NT];
    for(long i=0;i<NT;i++) pthread_create(&th[i],NULL,worker,(void*)i);
    for(int i=0;i<NT;i++) pthread_join(th[i],NULL);
    printf("xfree ok\n");
    return 0;
}
