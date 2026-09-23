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


## Совместимость с играми (важно)

Некоторые игры тащат СВОИ аллокаторы (Paradox/Clausewitz, Chromium/CEF, Intel TBB)
и НЕ переносят подмену `malloc` вообще — даже минимальный форвард в glibc валит их
(`terminate called without an active exception`). Проверка перед использованием:

    ./galloc_probe.sh "/path/to/game/binary"

- `empty.so: RUNNING`, `min malloc.so: RUNNING` → аллокатор применим;
- `min malloc.so: EXIT:*` → игра не переносит подмену malloc, НЕ использовать.

Результаты:
- **Maestro's Cold War 2** (native Godot): работает, загрузка +6% к glibc,
  RAM ~15x меньше. `LD_PRELOAD=.../libphase_alloc.so %command%`
- **Hearts of Iron IV** (Paradox + CEF + TBB): **РАБОТАЕТ** (после фикса).
  Причина прежних падений: TBB вызывает malloc ДО нашего конструктора;
  если аллокатор зависит от ctor (mutex/таблица/dlsym) — init TBB падает.
  Фикс: статический мьютекс, ленивая таблица классов, никакой зависимости
  от конструктора в hot-path. Проверять `./galloc_probe.sh <binary>`.

## Журнал неудачных экспериментов (append-only, как дневник)

Проверено и отвергнуто замером (не удаляю — это часть истории):

| гипотеза | результат | откат |
|---|---|---|
| MADV_HUGEPAGE + регионы 2 МБ | malloc 72 cyc, free 82 (хуже: стоимость fault) | откачено |
| блоки без заголовка + глобальный кэш регионов | free 51 → 76…90 cyc (регресс) | откачено |
| top-block realloc (рост последнего блока без копии) | падение игры (fontconfig): смешение классов в регионе | откачено (восстановлено из git) |

Оставлено (измеренный оптимум): malloc 46 cyc, free 53 cyc, realloc 161 cyc;
CW2 ≈ 6.2 с против glibc 5.78 с; HOI4 запускается; режим DIARY для фазовых нагрузок.
