# phase_alloc — рабочий LD_PRELOAD аллокатор (v4, MT-safe)

Статус: **ГОТОВ (добит)**. MT-safe + память ограничена.
Проверено:
- MT стресс 8 потоков (200k x 8) 3/3, checksum == glibc;
- SOAK 8 потоков x 1M: exit=0, checksum 174278880 == glibc, peak RSS **2 544 kB против glibc 5 664 kB**;
- фикс: FREE_BIT снимается при pop (иначе блок терялся -> RSS 2 ГБ и зависание);
- кадры 2.5 ns/alloc против glibc 14 (LD_PRELOAD=... ./bench frames).

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
