#!/bin/bash
# Обёртка для Steam: preload только в реальный процесс игры.
# Steam launch options: /home/lain/phase-vm/alloc/run_game.sh %command%
export LD_PRELOAD=/home/lain/phase-vm/alloc/libphase_alloc.so
export GALLOC_VERBOSE=1        # баннер "[galloc] loaded pid=..." в stderr
export GALLOC_DIAG=1           # лог чужих free
# export GALLOC_LEAK_FOREIGN=1 # если движок со своим аллокатором (раскомментируй)
echo "[run_game] exec: $*" >&2
exec "$@"
