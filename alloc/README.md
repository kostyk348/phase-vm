# phase_alloc — рабочий LD_PRELOAD аллокатор (v4, MT-safe)

Статус: **v5 чистый — работает на реальных рантаймах** (CPython 3.14 MT,
C++ std::thread, git, + наш MT-стресс/soak/cross-free). Прежние «python-висы»
были артефактом повреждённого файла (наслоение правок: glock сам себя
вызывал), а не дизайна. hot-path без глобального лока. Открытый дефект: CPython 3.14 MT (2+ потоков) под LD_PRELOAD виснет.
СТАТУС v5: hot-path БЕЗ глобального лока (per-thread регионы in_own + быстрый
free/usable своих блоков; глобальный lock только для чужих/больших/новых
регионов). Проверено: api, MT-стресс 8x200k, soak 8x1M, cross-free C, и
РЕАЛЬНЫЙ C++ std::thread 6 потоков (vector/string churn) — всё зелёное.
Диагностика env: GALLOC_DIAG=1 (лог foreign-free), GALLOC_LEAK_FOREIGN=1
(чужие free не форвардить — для движков со своим аллокатором).
Для Steam: launch options LD_PRELOAD=$PWD/libphase_alloc.so %command%
(только native-Linux).
Проверено:
- MT стресс 8 потоков (200k x 8) 3/3, checksum == glibc;
- SOAK 8 потоков x 1M: exit=0, checksum 174278880 == glibc, peak RSS **2 544 kB против glibc 5 664 kB**;
- фикс: FREE_BIT снимается при pop (иначе блок терялся -> RSS 2 ГБ и зависание);
- кадры 6.4 ns/alloc против glibc 14.0 (~2.2×) (LD_PRELOAD=... ./bench frames).

Дизайн v4 (после root-cause гонки в общем free-листе):
- У каждого потока СВОИ арены и приватный free-лист (трогает только владелец).
- Чужой free() кладёт блок в глобальный pending (под mutex); владелец при
  malloc забирает pending в свой лист.
- Общего кросс-поточного списка нет -> нет гонки. Owner-id в заголовке.
- FREE_BIT в заголовке делает двойной free безвредным.
- Полный POSIX ABI, выравнивание 16, реестр mmap-регионов (чужие -> RTLD_NEXT),
  per-thread фазовые кадры pa_frame_begin/end (bump, bulk reset O(1)).

Замер: кадры 6.4 ns/alloc vs glibc 14.0 (~2.2×) (LD_PRELOAD=$PWD/libphase_alloc.so ./bench frames).

Запуск проверок:
  make && ./api && LD_PRELOAD=$PWD/libphase_alloc.so ./api
  ./stress && LD_PRELOAD=$PWD/libphase_alloc.so ./stress   # MT 8 потоков
