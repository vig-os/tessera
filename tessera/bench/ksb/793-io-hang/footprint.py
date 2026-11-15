# #793 tmp-footprint sampler: polls statvfs(<dir>) every ~1 ms and keeps the PEAK bytes in use on that filesystem.
# Inside a Nix build sandbox /dev/shm is a tmpfs private to the build, so "in use" is this build's own tmp data.
# Rewrites <out> on every new peak (no signal handling needed: the caller just kills it) and appends one
# "<seconds> <max bytes in that second>" line per second to <out>.series.
import os
import sys
import time

d, out = sys.argv[1], sys.argv[2]
t0 = time.monotonic()
peak = 0
sec = 0
sec_max = 0
series = open(out + ".series", "w", buffering=1)
while True:
    s = os.statvfs(d)
    used = (s.f_blocks - s.f_bfree) * s.f_frsize
    if used > peak:
        peak = used
        with open(out + ".tmp", "w") as f:
            f.write("%d %.3f\n" % (peak, time.monotonic() - t0))
        os.replace(out + ".tmp", out)
    now = int(time.monotonic() - t0)
    if now != sec:
        series.write("%d %d\n" % (sec, sec_max))
        sec = now
        sec_max = 0
    sec_max = max(sec_max, used)
    time.sleep(0.001)
