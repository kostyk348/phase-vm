# phase_alloc — LD_PRELOAD аллокатор (WIP)

Статус: **НЕ продакшен.** Однопоточная корректность подтверждена (api + стресс
3/3 детерминирован, кадры 2.5 ns/alloc против glibc 14). Многопоточный стресс
(8 потоков) всё ещё падает — известный открытый дефект гонки, root-cause не
завершён. Не прелоадить в реальные игры/приложения.

Что сделано (v3):
- полный POSIX ABI: malloc/free/calloc/realloc/reallocarray/posix_memalign/
  aligned_alloc/memalign/valloc/malloc_usable_size/strdup/strndup/__libc_*;
- выравнивание 16; заголовки: class/large 16B перед ptr, aligned 32B;
- потокобезопасность: глобальный mutex + per-thread фазовые кадры
  (pa_frame_begin/end, bump, bulk reset O(1));
- реестр mmap-регионов: заголовки читаются только внутри своих регионов,
  чужие указатели -> RTLD_NEXT free (фикс segfault на glibc-внутренних);
- фикс magic: класс в битах 32..39 (не внутри 32-битной магии).

Известные проблемы:
- MT стресс падает в carve_locked (порча free-листа класса) — гонка не найдена;
- однопоточный double-free-детектор давал ложные срабатывания — убран.

Быстрый старт: make && ./api && LD_PRELOAD=$PWD/libphase_alloc.so ./api
Замер кадров: LD_PRELOAD=$PWD/libphase_alloc.so ./bench frames
