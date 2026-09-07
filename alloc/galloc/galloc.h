/* galloc.h — универсальный игровой аллокатор.
 * 1) FRAME-арены: ga_frame_begin/end (O(1) сброс кадра), nested mark/rollback;
 * 2) ПУЛЫ объектов: ga_pool_create/alloc/free — O(1), повторное использование;
 * 3) ОБЩАЯ куча: ga_alloc/ga_free/ga_aligned — MT-safe (per-thread + reuse),
 *    выравнивание 16;
 * 4) Статистика: ga_stats().
 * Все арены/пулы/куча не зависят от malloc (mmap напрямую) — детерминированно
 * и без глобального аллокатора в горячих путях.
 */
#ifndef GALLOC_H
#define GALLOC_H
#include <stddef.h>

/* --- фрейм-арена (per-thread) --- */
void  ga_frame_begin(void);
void  ga_frame_end(void);
size_t ga_frame_mark(void);          /* байтовый маркер */
void  ga_frame_rollback(size_t m);   /* O(1) откат к маркеру */
void* ga_frame_alloc(size_t n);      /* bump; 16-выровнено */

/* --- пул объектов (фиксированный размер, O(1)) --- */
typedef struct ga_pool ga_pool;
ga_pool* ga_pool_create(size_t elem, int cap);
void* ga_pool_alloc(ga_pool* p);     /* 16-выровнено */
void  ga_pool_free(ga_pool* p, void* o);
void  ga_pool_destroy(ga_pool* p);

/* --- общая MT-куча --- */
void* ga_alloc(size_t n);
void  ga_free(void* p);
void* ga_aligned(size_t align, size_t n);
void* ga_realloc(void* p, size_t n);
size_t ga_usable(void* p);

void ga_stats(void); /* вывести статистику в stderr */
#endif
