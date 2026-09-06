#!/bin/bash
# замер времени + пикового RSS (VmHWM) для команды
"$@" & P=$!
R=0
while kill -0 $P 2>/dev/null; do
  V=$(awk '/VmHWM/{print $2}' /proc/$P/status 2>/dev/null); [ -n "$V" ] && R=$V
  sleep 0.05
done
wait $P; E=$?
echo "exit=$E peakRSS_kB=${R:-?}"
