# phase_alloc — рабочий LD_PRELOAD аллокатор (v4, MT-safe)

Статус: **MT-safe подтверждён**. API-тест под нами OK; стресс 8 потоков
(200k итераций x 8, аллокации 1..8192, posix_memalign/aligned/strdup/realloc/
usable) — 3/3 успех, checksum 34851280 == glibc.

Дизайн v4 (после root-cause гонки в общем free-листе):
- У каждого потока СВОИ арены и приватный free-лист (трогает только владелец).
- Чужой free() кладёт блок в глобальный pending (под mutex); владелец при
  malloc забирает pending в свой лист.
- Общего кросс-поточного списка нет -> нет гонки. Owner-id в заголовке.
- FREE_BIT в заголовке делает двойной free безвредным.
- Полный POSIX ABI, выравнивание 16, реестр mmap-регионов (чужие -> RTLD_NEXT),
  per-thread фазовые кадры pa_frame_begin/end (bump, bulk reset O(1)).

Замер: кадры 2.5 ns/alloc vs glibc 14.0 (LD_PRELOAD=$PWD/libphase_alloc.so ./bench frames).

Запуск проверок:
  make && ./api && LD_PRELOAD=$PWD/libphase_alloc.so ./api
  ./stress && LD_PRELOAD=$PWD/libphase_alloc.so ./stress   # MT 8 потоков
