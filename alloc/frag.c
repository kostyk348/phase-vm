/* фрагментация: освободить смежные пары 64+64 и держать ЖИВЫМИ много 128-блоков.
   Коалесцирующий аллокатор берёт их из слитых пар; без коалесценции — режет новую память. */
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#define N 60000
int main(void){
    void** p = malloc((size_t)N*sizeof(void*));
    for(int i=0;i<N;i++){ p[i]=malloc(64); *(char*)p[i]=1; }
    for(int i=0;i<N;i+=2){ free(p[i]); free(p[i+1]); }   /* смежные пары -> слитные 128 */
    void** q = malloc((size_t)(N/2)*sizeof(void*));
    int ok=0;
    for(int i=0;i<N/2;i++){ q[i]=malloc(128); if(!q[i]) break; *(char*)q[i]=2; ok++; }
    printf("live 128-blocks served = %d / %d\n", ok, N/2);
    return 0;
}
