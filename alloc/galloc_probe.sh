#!/bin/bash
# Проверка: переносит ли программа подмену malloc (LD_PRELOAD)?
# usage: ./galloc_probe.sh <path-to-game-binary>
BIN="${1:?usage: galloc_probe.sh <binary>}"
DIR=$(dirname "$BIN"); BASE=$(basename "$BIN")
TMP=$(mktemp -d)
# пустая .so (без символов) — контроль
printf 'int x=0;\n' > "$TMP/empty.c"; cc -O2 -shared -fPIC -o "$TMP/empty.so" "$TMP/empty.c"
# минимальный форвард malloc/free в glibc
cat > "$TMP/min.c" <<'C'
#define _GNU_SOURCE
#include <dlfcn.h>
#include <stddef.h>
static void*(*rm)(size_t); static void(*rf)(void*); static void*(*rr)(void*,size_t); static void*(*rc)(size_t,size_t);
__attribute__((constructor)) static void i(void){rm=dlsym(RTLD_NEXT,"malloc");rf=dlsym(RTLD_NEXT,"free");rr=dlsym(RTLD_NEXT,"realloc");rc=dlsym(RTLD_NEXT,"calloc");}
void* malloc(size_t n){return rm?rm(n):0;} void free(void*p){if(rf)rf(p);} void* calloc(size_t n,size_t s){return rc?rc(n,s):0;} void* realloc(void*p,size_t n){return rr?rr(p,n):0;}
C
cc -O2 -shared -fPIC -o "$TMP/min.so" "$TMP/min.c" -ldl
probe(){ local so="$1"; ( cd "$DIR"; setsid env LD_PRELOAD="$so" "./$BASE" >/dev/null 2>&1 & p=$!; sleep 6; if kill -0 $p 2>/dev/null; then kill -9 -$p 2>/dev/null; echo RUNNING; else wait $p; echo "EXIT:$?"; fi ); }
echo "binary: $BIN"
echo -n "empty.so  (контроль): "; probe "$TMP/empty.so"
echo -n "min malloc.so        : "; probe "$TMP/min.so"
rm -rf "$TMP"
