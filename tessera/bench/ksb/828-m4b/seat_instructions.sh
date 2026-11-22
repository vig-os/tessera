#!/usr/bin/env bash
# #828 round 2, seat fallback for instruction counts only: ksb pods cannot count instructions (their seccomp
# profile, kcmp-seccomp.json, has defaultAction SCMP_ACT_ERRNO and no perf_event_open rule).
#
# Prebuilt round-1 binaries (no cargo build), inputs and outputs on tmpfs, one pinned core. Each run waits until
# the 1-minute load average is below the core count and logs uptime and /proc/pressure/{cpu,io} before and
# after. v1 and v2 are interleaved ROUNDS times per fixture. `perf stat` counts the WHOLE process, so the counts
# include loading the .npy columns, the same code in both builds: the v1/v2 ratio is a bound diluted towards 1,
# and the per-event difference v2 - v1 is the encode-path difference exactly.
#
#   BIN=/dev/shm/t828/bin DATA=/dev/shm/t828 OUT=/dev/shm/t828/seatperf CORE=29 ROUNDS=5 ./seat_instructions.sh
set -euo pipefail
: "${BIN:?}" "${DATA:?}" "${OUT:?}"
CORE=${CORE:-29}
ROUNDS=${ROUNDS:-5}
cores=$(nproc)
mkdir -p "$OUT"
chmod 700 "$OUT"
res="$OUT/results.jsonl"
state() { printf '{"uptime":"%s","psi_cpu":"%s","psi_io":"%s"}' "$(uptime)" \
  "$(tr '\n' ' ' < /proc/pressure/cpu)" "$(tr '\n' ' ' < /proc/pressure/io)"; }
wait_quiet() {
  while :; do
    l=$(cut -d' ' -f1 /proc/loadavg)
    awk -v l="$l" -v c="$cores" 'BEGIN { exit !(l < c) }' && return
    sleep 30
  done
}
for d in ge siemens; do
  for r in $(seq 1 "$ROUNDS"); do
    for p in v1 v2; do
      wait_quiet
      before=$(state)
      tsra="$OUT/$d-$p.tsra"
      rm -f "$tsra"
      perf stat -x, -e instructions:u -o "$OUT/perf.txt" -- \
        taskset -c "$CORE" "$BIN/table_profile_$p" write "$DATA/$d" "$tsra" 1 > "$OUT/w.json"
      after=$(state)
      instr=$(awk -F, '$3=="instructions:u"{print $1}' "$OUT/perf.txt")
      printf '{"fixture":"%s","side":"%s","round":%s,"instructions_u":%s,"write":%s,"before":%s,"after":%s}\n' \
        "$d" "$p" "$r" "$instr" "$(cat "$OUT/w.json")" "$before" "$after" >> "$res"
      echo "$d $p round $r: instr $instr"
    done
  done
done
rm -f "$OUT"/*.tsra
echo SEAT_DONE
