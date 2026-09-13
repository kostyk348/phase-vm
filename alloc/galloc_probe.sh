#!/bin/bash
# Проверка применимости аллокатора к программе.
# usage: ./galloc_probe.sh <binary> [allocator.so]
# Тестирует: пустая .so (контроль), наш аллокатор (если задан/рядом), min-forward (контроль).
BIN="${1:?usage: galloc_probe.sh <binary> [allocator.so]}"
OURS="${2:-$(cd "$(dirname "$0")" && pwd)/libphase_alloc.so}"
DIR=$(dirname "$BIN"); BASE=$(basename "$BIN"); TMP=$(mktemp -d)
printf 'int x=0;\n' > "$TMP/empty.c"; cc -O2 -shared -fPIC -o "$TMP/empty.so" "$TMP/empty.c"
probe(){ local so="$1"; ( cd "$DIR"; setsid env LD_PRELOAD="$so" "./$BASE" >/dev/null 2>&1 & p=$!; sleep 6; if kill -0 $p 2>/dev/null; then kill -9 -$p 2>/dev/null; echo RUNNING; else wait $p; echo "EXIT:$?"; fi ); }
echo "binary: $BIN"
echo -n "empty.so (контроль)   : "; probe "$TMP/empty.so"
[ -f "$OURS" ] && { echo -n "phase_alloc.so        : "; probe "$OURS"; }
echo "(примечание: 'min-forward' preload даёт ложные отказы — не используем как критерий)"
rm -rf "$TMP"
