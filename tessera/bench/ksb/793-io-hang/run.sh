#!/usr/bin/env bash
# #793 reproducer matrix. Usage: run.sh --scratch <disk-backed dir>
# Every run is one test of one binary, capped by `timeout` (the cap bounds the MEASUREMENT; rc 124 = still running).
set -u
SCRATCH=; while [ $# -gt 0 ]; do case $1 in --scratch) SCRATCH=$2; shift 2;; *) echo "unknown $1"; exit 2;; esac; done
OUT=${KSB_JOB_OUT:-/tmp/out}; mkdir -p "$OUT"
libs=("$TESTS"/tessera_io-*); slms=("$TESTS"/session_leaf_memory-*); LIB=${libs[0]}; SLM=${slms[0]}
CH=stream::tests::content_hash_is_identical_across_workers_and_windows
declare -A BIN=([ch]=$LIB [slm]=$SLM) FILT=([ch]=$CH [slm]=session_leaf_memory)
SHM=/dev/shm/r793

echo "== env"; nproc; cat /proc/loadavg
for f in pids.max pids.current cpu.max memory.max; do echo "cgroup $f: $(cat /sys/fs/cgroup/$f 2>/dev/null)"; done
echo "ulimit -u: $(ulimit -u)"; echo "scratch: $(stat -f -c %T "$SCRATCH") shm: $(stat -f -c %T /dev/shm)"

# one <label> <test> <dir> <cap> [prefix...] : a single run, timed
one() { local label=$1 t=$2 dir=$3 cap=$4; shift 4; mkdir -p "$dir"; local s e rc
  s=$(date +%s.%N); TMPDIR=$dir timeout "$cap" "$@" "${BIN[$t]}" --exact "${FILT[$t]}" --test-threads=1 > "$dir.log" 2>&1; rc=$?
  e=$(date +%s.%N); printf '%-34s %-4s rc=%-3s %9.2fs\n' "$label" "$t" "$rc" "$(echo "$e - $s" | bc)"; }

# many <label> <test> <root> <n> <cap> [prefix...] : n concurrent runs (nextest-style process fan-out)
many() { local label=$1 t=$2 root=$3 n=$4 cap=$5; shift 5; local s e
  s=$(date +%s.%N)
  for i in $(seq 1 "$n"); do one "$label/p$i" "$t" "$root/p$i" "$cap" "$@" > "$root.p$i.res" & done; wait
  e=$(date +%s.%N)
  awk -v l="$label" -v t="$t" '{split($3,a,"="); rc[a[2]]++; sub("s","",$4); if($4>mx)mx=$4; if(mn==""||$4<mn)mn=$4}
    END{r="";for(k in rc)r=r k"x"rc[k]" "; printf "%-34s %-4s n=%d rc{%s} min %.1fs max %.1fs\n", l, t, NR, r, mn, mx}' "$root".p*.res
  printf '%-34s %-4s batch wall %.1fs\n' "$label" "$t" "$(echo "$e - $s" | bc)"; cat "$root".p*.res | grep -v "rc=0 " | head -5; }

echo "== fsync count (strace, tmpfs)"
for t in ch slm; do mkdir -p $SHM/strace-$t
  TMPDIR=$SHM/strace-$t strace -f -c -e trace=fsync,fdatasync,rename,openat -o "$OUT/strace-$t.txt" "${BIN[$t]}" --exact "${FILT[$t]}" --test-threads=1 > /dev/null 2>&1
  echo "$t: $(awk '$NF=="fsync"||$NF=="fdatasync"||$NF=="rename"{print $NF"="$4}' "$OUT/strace-$t.txt" | tr '\n' ' ')"; done

echo "== A: tmpfs, 1 process"
for t in ch slm; do one A-tmpfs "$t" $SHM/A-$t 600; done
echo "== B1: tmpfs, 32 processes, taskset -c 0-1 (CPU-starved)"
for t in ch slm; do many B1-tmpfs-cpu2-x32 "$t" $SHM/B1-$t 32 1800 taskset -c 0-1; done
echo "== B2: tmpfs, 32 processes, taskset -c 0-1, ulimit -u 256 (thread budget exhausted)"
for t in ch slm; do many B2-tmpfs-cpu2-x32-nproc256 "$t" $SHM/B2-$t 32 1800 bash -c 'ulimit -u 256; exec "$@"' _ taskset -c 0-1; done
grep -lE "Resource temporarily unavailable|EAGAIN|failed to spawn" $SHM/B2-*/*.log $SHM/B2-*.p*.log 2>/dev/null | wc -l | sed 's/^/B2 logs mentioning EAGAIN: /'
rm -rf $SHM

echo "== fsync latency on scratch (idle)"; python3 -I "$PROBE" "$SCRATCH/probe" 100
echo "== C: disk scratch, 1 process"
for t in slm ch; do one C-disk "$t" "$SCRATCH/C-$t" 1800; done
echo "== D: disk scratch + 2 buffered writers (CI's concurrent crane tarballs)"
load() { while :; do dd if=/dev/zero of="$SCRATCH/load$1" bs=1M count=2048 status=none; done; }
load 1 & L1=$!; load 2 & L2=$!; sleep 5
python3 -I "$PROBE" "$SCRATCH/probe" 50
for t in slm ch; do
  one D-disk-load "$t" "$SCRATCH/D-$t" 1800 &
  P=$!; sleep 60
  # where is it waiting? (kernel wait channel of every thread of the test process)
  pid=$(pgrep -f -n -- "--exact ${FILT[$t]}" || true)
  [ -n "$pid" ] && { echo "wchan of $t (pid $pid) at 60s:"; for tk in /proc/"$pid"/task/*; do cat "$tk"/wchan; echo; done | sort | uniq -c;
    timeout 60 gdb -p "$pid" -batch -ex 'thread apply all bt 8' > "$OUT/gdb-D-$t.txt" 2>&1; grep -cE "fsync|sync_all" "$OUT/gdb-D-$t.txt" | sed 's/^/gdb frames in fsync: /'; }
  wait $P
done
kill $L1 $L2; wait 2>/dev/null; rm -f "$SCRATCH"/load*
echo "== E: disk scratch, 32 processes (nextest fan-out)"
many E-disk-x32 slm "$SCRATCH/E-slm" 32 1800
rm -rf "$SCRATCH"/{C,D,E}-* "$SCRATCH/probe"
echo "== done"
