# phase_alloc — рабочий LD_PRELOAD аллокатор (v4, MT-safe)

Статус: **работает на нашем синтетическом MT-стрессе и soak; НЕ готов для
произвольных больших приложений**. Открытый дефект: CPython 3.14 MT (2+ потоков) под LD_PRELOAD виснет.
Диагноз: ABBA-дедлок python-GIL <-> наш глобальный g_lock (воркер держит
GIL и ждёт g_lock, чей владелец ждёт GIL). GIL у python есть; импорты и
1-поток работают. План фикса: убрать глобальный lock из hot-path
malloc/free (per-thread регионы + редкая очередь чужих). До фикса прелоад
в произвольные/Steam-приложения НЕЛЬЗЯ; рабочий путь для своих движков -
galloc API (frame-арены+пулы+MT-куча) или frame-режим с pa_frame_begin/end.
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
