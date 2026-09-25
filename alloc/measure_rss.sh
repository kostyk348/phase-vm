#!/bin/bash
# peak RSS (VmHWM) процесса за T секунд. Не виснет: setsid + kill группы, без wait.
BIN="$1"; T="${2:-15}"; shift 2
D=$(dirname "$BIN"); B=$(basename "$BIN")
setsid env "$@" "$D/$B" >/dev/null 2>&1 & P=$!
sleep 0.3
PEAK=0
for i in $(seq 1 $((T*5))); do
  V=$(awk '/VmHWM/{print $2}' /proc/$P/status 2>/dev/null)
  if [ -n "$V" ] && [ "$V" -gt "$PEAK" ]; then PEAK=$V; fi
  if ! kill -0 $P 2>/dev/null; then echo "процесс завершился на ${i}*0.2s (peak=$PEAK)"; kill -9 -$P 2>/dev/null; echo "peakRSS_kB=$PEAK"; exit 0; fi
  sleep 0.2
done
kill -9 -$P 2>/dev/null
echo "peakRSS_kB=$PEAK"
