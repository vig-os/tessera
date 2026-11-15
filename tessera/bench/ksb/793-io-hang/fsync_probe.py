# #793 fsync-latency probe: usage fsync_probe.py <dir> <n>. Writes, fsyncs and removes n 4 KiB files in <dir>,
# fsyncing the directory after each (the fragment + directory half of a WriteSession commit), and prints the
# median, p99, max and total latency. run.sh runs it on the job's disk-backed scratch, idle and under load.
import os
import sys
import time

d = sys.argv[1]
n = int(sys.argv[2])
os.makedirs(d, exist_ok=True)
lat = []
for i in range(n):
    p = os.path.join(d, f"f{i}")
    t = time.perf_counter()
    with open(p, "wb") as f:
        f.write(b"x" * 4096)
        f.flush()
        os.fsync(f.fileno())
    fd = os.open(d, os.O_RDONLY)
    os.fsync(fd)
    os.close(fd)
    lat.append(time.perf_counter() - t)
    os.unlink(p)
lat.sort()
print(
    d,
    "n",
    n,
    "median %.4f p99 %.4f max %.4f total %.2f"
    % (lat[n // 2], lat[int(n * 0.99)], lat[-1], sum(lat)),
)
