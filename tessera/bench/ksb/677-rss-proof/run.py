#!/usr/bin/env python3
"""ADR-0064 §8.1, as amended by #760, #770 and #797: the RSS proof of `StreamWriter::push_table` with the consumer reproducer (#715).

Two ksb jobs, each a subcommand that runs the reproducer, writes `records.json` to `--out` and then evaluates it:

- `job` (criteria 1-5): the sweep, with every session and output directory on a volume that `statfs` shows is NOT
  tmpfs (else the job fails before any row is written). Each W = 4 point of 64 blocks or more runs k = 3 times
  (A.2), each W = 1 point of 16 blocks or more k₁ times (B.5), every other point once. Per run it records the exit code, the 10 ms samples' peaks, the input row-group at the
  `RssAnon` peak, the CPUs it may run on, the cgroup's anon and processes just before and after it (criterion 3
  counts only the anon the run added) and the sealed manifest (kept as `<run>.manifest.json`, summarised in
  `records.json`); at 268 M rows it also prints the wall times and the session device's `%util` as context (A.3,
  A.4): timings on a shared disk are never checked.
- `parallel` (criterion 6): the W = 1 and W = 4 runs at 268 M rows with the session and output directories on
  `/dev/shm`. Its guard is the converse one (A.3): they MUST be tmpfs, else the job fails, so neither job can
  silently measure the other's setup. The speed-up only counts with the 4 CPUs of `--cpu 4 --exclusive-cpus`.

Criterion 2 is C.2's within-run slope (#797): per W, over the tenths of every run at n ≥ 16 W whose leaves through
the writer x_rt are at least 16 W, the tenth's peak `RssAnon` is fitted as α_r + b · x_rt, one intercept per run and
one common slope, with a standard error cluster-robust by run (CR1) and t at G − 1 degrees of freedom, G the runs
fitted. B.3's pass rule is unchanged: the slope at most `c_leaf` plus the one-sided 95 % margin `t₀.₉₅,G−1 · SE`
(slope), and the detection floor `2 t · SE` at most δ_z = 0.029 B/row (resolution) and, at W = 1, at most `c_leaf`
(B.5's re-check). An unresolved fit is inconclusive, and fails. The W = 4 floor is printed as a stated limit. B.2's
across-run OLS slope of each run's peak against its leaves is printed as CONTEXT, never graded: across runs the peak
tracks the shared disk's load (C.1). `test_run.py` pins the arithmetic to ADR-0064's B.4, B.5 and C.4.
`run.py log-tenths LOG...` runs criterion 2 on tenths rebuilt from run logs' own sample lines, for r1, which recorded
none (C.4); `--extract DIR` writes the small, provenance-stamped extracts `test_run.py` uses.

`job` can run as shards (`--workers`, `--rows`, `--reps`: the repeat indices of each point to run), so the repeats run
in parallel ksb jobs rather than serially in one of ~24 h. A shard's exit code checks only that its own runs
completed, guards included; the criteria are checked over all shards by `run.py evaluate DIR...`, which refuses
shards of different binaries (R₀ and R₁ calibrate one binary) or a repeat recorded twice.

`records.json` is rewritten after every run, so a job that dies keeps the runs it finished. `run.py --out DIR
evaluate` (`--out` before the subcommand, default `$KSB_JOB_OUT`), or `run.py evaluate DIR...` over a job's shards,
re-checks one offline, the guards included. Every criterion prints its numbers and bound; the exit code is 1 if any
fails. A criterion whose runs are missing or failed fails: it is never passed on no data.
"""

import argparse
import csv
import hashlib
import json
import math
import os
import re
import shutil
import subprocess
import sys
import time
import zipfile
from pathlib import Path

MIB = 1 << 20
GIB = 1 << 30
ROWS_PER_GROUP = 1 << 16  # tessera_io::table::ROWS_PER_GROUP
BLOCK_ROWS = 1 << 22  # tessera_io::table::BLOCK_ROWS
# r_mem of the consumer's `pairs` row: pkt 8 + ms 4 + ms validity 1 + prompt 1 + 7 × u8 + tof 2 + rsvd 1.
R_MEM = 24
S = 2 * ROWS_PER_GROUP * R_MEM + 4 * MIB  # §8: s = s_stage + s_fixed = 7 MiB
B_LEAF = 1024  # A.1: b_leaf, the per-leaf bookkeeping outside the entry's bytes
TMPFS_MAGIC = 0x01021994
CPUS = 4  # §8.1/A.3: ksb run --cpu 4 --exclusive-cpus
CANONICAL_DROP = ("producer", "timestamp", "id", "id_inputs", "manifest_hash")  # §6
TABLE = "pairs"  # the table push_table streams; the reproducer's other tables are single blocks
LEAF = re.compile(
    rf"^{TABLE}(/\d{{4,}})?$"
)  # §1: the bare name for one block, else <table>/{index:04}
WORKERS = (1, 4)
ROWS = (0, 4_194_304, 67_108_864, 268_435_456, 1_073_741_824, 2_400_000_000)
MID = 268_435_456  # criteria 5 and 6, and the A.3 context
REPEATS = 3  # A.2: k, for each W = 4 point of 64 blocks or more
K1 = 6  # B.5: k₁, for each W = 1 point of 16 blocks or more (the floor at r3's noise ≤ c_leaf)
PLATEAU_FROM = {
    1: 16,
    4: 64,
}  # criterion 2: n ≥ 16 W blocks at W = 4, from 16 blocks at W = 1
ALPHA = 0.05  # B.3: the slope test's one-sided level, per W
DELTA_Z = 121_635  # B.3: δ_z, the zone-statistics regression (0.029 B/row) in B/leaf, which criterion 2 must resolve
SPEEDUP = 1.5  # criterion 6
# B.3: r3's largest pairs entry, for runs graded from logs, which hold no manifest. r1's sealed manifests weren't kept;
# its W = 1 slope fails even if its entries were 2,367 B larger (c_leaf + 4,734 B), its W = 4 slope by 22,569 B.
E_LEAF_LOGS = 2966


def s_rss(w):
    """§8.1: allocator retention and thread stacks the heap model doesn't see."""
    return 32 * MIB + 8 * MIB * (w + 1)


def blocks(rows):
    """The leaves push_table writes for `rows` (tessera_io::table::partition_blocks: 0 rows is one empty block)."""
    return max(1, -(-rows // BLOCK_ROWS))


def repeats(w, rows):
    """A.2 and B.5: k runs of each W = 4 point at 64 blocks or more, k₁ of each W = 1 point at 16 blocks or more
    (the runs criterion 2 fits, n ≥ 16 W), 1 of every other point."""
    if blocks(rows) >= PLATEAU_FROM[w]:
        return {1: K1, 4: REPEATS}[w]
    return 1


def mib(b):
    return f"{b / MIB:.1f} MiB"


def jcs(v):
    """RFC 8785 canonical JSON, as `tessera_core::canonical` writes it, for the values a manifest entry holds.
    Keys sort by UTF-16 code units; strings escape as JSON's minimal set, which Python's `ensure_ascii=False` matches.
    A non-integer number has no exact Python counterpart of ECMAScript's shortest form, so it is refused, not guessed.
    """
    if isinstance(v, dict):
        keys = sorted(v, key=lambda k: k.encode("utf-16-be"))
        return "{" + ",".join(f"{jcs(k)}:{jcs(v[k])}" for k in keys) + "}"
    if isinstance(v, list):
        return "[" + ",".join(jcs(x) for x in v) + "]"
    if isinstance(v, float):
        raise ValueError(f"non-integer number {v!r} in a manifest entry")
    return json.dumps(v, ensure_ascii=False)


def leaves(manifest):
    """The streamed table's leaf entries (name, kind, digest, spec), in manifest order."""
    return [b for b in manifest["blocks"] if LEAF.match(b["name"])]


def e_leaf(manifest):
    """A.1: the largest leaf entry's length in the sealed manifest's canonical JSON, in bytes."""
    return max(len(jcs(b).encode()) for b in leaves(manifest))


def summary(manifest):
    """What the criteria read off a sealed manifest, small enough for `ksb out` to fetch: the streamed table's leaf
    count and e_leaf, `content_hash`, and a digest per top-level key of the canonical manifest minus §6's fields."""
    return {
        "leaves": len(leaves(manifest)),
        "e_leaf": e_leaf(manifest),
        "blocks": len(manifest["blocks"]),
        "content_hash": manifest["content_hash"],
        "canonical": {
            k: hashlib.sha256(jcs(v).encode()).hexdigest()
            for k, v in manifest.items()
            if k not in CANONICAL_DROP
        },
    }


def c_leaf(e):
    """A.1: c_leaf = 2 · (e_leaf + b_leaf), the factor 2 being the allocator-retention allowance."""
    return 2 * (e + B_LEAF)


def t_quantile(p, df):
    """The p-quantile (p > 0.5) of Student's t with `df` degrees of freedom, by bisection on its CDF: t's upper tail
    is I_x(df/2, 1/2) / 2 at x = df / (df + t²), the regularized incomplete beta function."""

    def ibeta(a, b, x):
        # Lentz's continued fraction for I_x(a, b) (Numerical Recipes §6.4), on the side where it converges fast.
        if x > (a + 1) / (a + b + 2):
            return 1 - ibeta(b, a, 1 - x)
        front = (
            math.exp(
                math.lgamma(a + b)
                - math.lgamma(a)
                - math.lgamma(b)
                + a * math.log(x)
                + b * math.log1p(-x)
            )
            / a
        )
        tiny, c, d, f = 1e-300, 1.0, 0.0, 1.0
        for i in range(400):
            m = i // 2
            if i == 0:
                num = 1.0
            elif i % 2:
                num = -(a + m) * (a + b + m) * x / ((a + 2 * m) * (a + 2 * m + 1))
            else:
                num = m * (b - m) * x / ((a + 2 * m - 1) * (a + 2 * m))
            d = 1 + num * d
            d = 1 / (d if abs(d) > tiny else tiny)
            c = 1 + num / c
            c = c if abs(c) > tiny else tiny
            f *= c * d
            if abs(1 - c * d) < 1e-15:
                return front * (f - 1)
        raise ArithmeticError(f"incomplete beta I_{x}({a}, {b}) did not converge")

    lo, hi = 0.0, 1e3
    for _ in range(200):
        t = (lo + hi) / 2
        if 1 - ibeta(df / 2, 0.5, df / (df + t * t)) / 2 < p:
            lo = t
        else:
            hi = t
    return (lo + hi) / 2


def plateau_checks(w, fit, e, detail):
    """B.3's pass rule on a fitted slope, as (name, ok, detail) triples: the slope check `b̂ ≤ c_leaf + t · SE`, the
    resolution check `2 t · SE ≤ δ_z`, and at W = 1 B.5's re-check `2 t · SE ≤ c_leaf`. An unresolved fit is
    inconclusive, and fails. The same rule grades the within-run slope (C.2) and was the across-run one's (B.3)."""
    b, margin, c = fit["b"], fit["t"] * fit["se"], fit["c_leaf"]
    floor = fit["floor"]
    checks = [
        (
            f"2 plateau W={w}: slope",
            b <= fit["bound"],
            f"b̂ {b:,.0f} ≤ c_leaf {c:,} B (e_leaf {e} B) + t · SE {margin:,.0f} = {fit['bound']:,.0f} B/leaf; "
            f"{detail}",
        ),
        (
            f"2 plateau W={w}: resolution",
            floor <= DELTA_Z,
            f"detection floor 2 t · SE {floor:,.0f} ≤ δ_z {DELTA_Z:,} B/leaf (0.029 B/row); else inconclusive",
        ),
    ]
    if w == 1:
        checks.append(
            (
                f"2 plateau W={w}: re-check",
                floor <= c,
                f"detection floor 2 t · SE {floor:,.0f} ≤ c_leaf {c:,} B/leaf (B.5: else inconclusive, and k₁ is "
                "re-derived from this noise by an amendment)",
            )
        )
    return checks


def fitted(b, se, df, e):
    """The pass rule's numbers for a slope `b̂` with standard error `se` and `df` degrees of freedom (B.3)."""
    t = t_quantile(1 - ALPHA, df)
    c = c_leaf(e)
    return {
        "b": b,
        "se": se,
        "t": t,
        "df": df,
        "bound": c + t * se,
        "floor": 2
        * t
        * se,  # B.3: the growth beyond c_leaf detected with probability ≥ 95 %, the resolution
        "c_leaf": c,
    }


def slope_test(w, points, e):
    """B.2 and B.3: the OLS slope of peak RssAnon (bytes) against leaves over `points`, the (n, RssAnon) of every run
    at n ≥ 16 W, and B.3's checks on it, as (name, ok, detail) triples. `e` is e_leaf, the largest over the runs.
    Since #797 (C.1) this across-run slope is context, confounded by the shared disk; `within_run_test` grades."""
    big_n = len(points)
    if big_n < 3 or len({n for n, _ in points}) < 2:
        return {}, [
            (
                f"2 plateau W={w}",
                False,
                f"{big_n} runs at {sorted({n for n, _ in points})} leaves: "
                "an OLS slope with a standard error needs 3 runs or more at 2 sizes or more",
            )
        ]
    n_bar = sum(n for n, _ in points) / big_n
    y_bar = sum(y for _, y in points) / big_n
    snn = sum((n - n_bar) ** 2 for n, _ in points)
    b = sum((n - n_bar) * (y - y_bar) for n, y in points) / snn
    a = y_bar - b * n_bar
    sigma = math.sqrt(sum((y - a - b * n) ** 2 for n, y in points) / (big_n - 2))
    fit = fitted(b, sigma / math.sqrt(snn), big_n - 2, e) | {
        "N": big_n,
        "sigma": sigma,
        "n_bar": n_bar,
        "snn": snn,
    }
    sizes = ", ".join(
        f"{n}: {[round(y / MIB, 2) for m, y in points if m == n]}"
        for n in sorted({n for n, _ in points})
    )
    detail = (
        f"N {big_n}, b̂ {b:,.0f} B/leaf ({b / BLOCK_ROWS:.4f} B/row), SE {fit['se']:,.0f}, "
        f"t₀.₉₅,{big_n - 2} {fit['t']:.3f}, σ̂ {sigma / MIB:.2f} MiB, n̄ {n_bar:.2f}, Sₙₙ {snn:,.0f}; "
        f"MiB per leaves {{{sizes}}}"
    )
    return fit, plateau_checks(w, fit, e, detail)


def tenth_leaves(rows, t):
    """C.1: x_rt, the leaves whose input is complete at the end of tenth `t` of a run of `rows` input rows. A run's
    records hold each tenth's peak RssAnon (`peaks`), and the tenths are cut at input rows `(t + 1) · rows / 10`, so
    tenth t ends with `(t + 1) · rows // (10 · BLOCK_ROWS)` complete blocks; the last tenth runs to the seal, when
    every leaf is committed."""
    return blocks(rows) if t == 9 else (t + 1) * rows // (10 * BLOCK_ROWS)


def cluster_fit(clusters):
    """C.2's fit over `clusters`, one list of (x_rt, y_rt) per run with two or more: the fixed-effects slope b̂ (OLS
    on the deviations from each run's own means), its CR1 standard error clustered by run, N and Sₓₓ. The CR1 factor
    is G/(G − 1) · (N − 1)/(N − 2): the per-run intercepts are nested in the clusters and not counted as regressors."""
    dev = []
    for kept in clusters:
        mx = sum(x for x, _ in kept) / len(kept)
        my = sum(y for _, y in kept) / len(kept)
        dev.append([(x - mx, y - my) for x, y in kept])
    sxx = sum(x * x for d in dev for x, _ in d)
    big_n = sum(len(d) for d in dev)
    if sxx == 0:
        return 0.0, math.inf, big_n, 0
    b = sum(x * y for d in dev for x, y in d) / sxx
    g = len(dev)
    scores = [sum(x * (y - b * x) for x, y in d) for d in dev]
    factor = g / (g - 1) * (big_n - 1) / (big_n - 2)
    return b, math.sqrt(factor * sum(u * u for u in scores)) / sxx, big_n, sxx


def within_run_test(w, runs, e):
    """C.2 and C.3 (#797): criterion 2's fixed-effects slope. Over every tenth t of every run r with
    x_rt ≥ 16 W leaves (`tenth_leaves`), y_rt = that tenth's peak RssAnon is fitted as y_rt = α_r + b · x_rt + ε,
    one intercept per run (its I/O state) and one common slope b, by OLS on the within-run deviations. SE(b̂) is
    cluster-robust by run (CR1: arbitrary correlation and variance within a run, runs independent), with B.3's
    pass rule at t₀.₉₅,G−1, G the runs with at least two tenths in the fit (a run with one has no within-run
    variation and adds nothing). Returns the fit and the (name, ok, detail) checks; missing data fail."""
    clusters, empty = [], []
    for r in runs:
        tenths = r.get("tenths") or []
        if len(tenths) != 10:
            return {}, [
                (
                    f"2 plateau W={w}",
                    False,
                    f"W={w} rows={r['rows']} run {r['rep'] + 1} has {len(tenths)} tenths recorded, not 10",
                )
            ]
        kept = [
            (tenth_leaves(r["rows"], t), y)
            for t, y in enumerate(tenths)
            if tenth_leaves(r["rows"], t) >= PLATEAU_FROM[w]
        ]
        empty += [(r["rows"], r["rep"]) for x, y in kept if y <= 0]
        if len(kept) >= 2:
            clusters.append(kept)
    if empty:
        return {}, [
            (
                f"2 plateau W={w}",
                False,
                f"tenths with no sample in the fit, (rows, rep) {sorted(set(empty))}: never passed on no data",
            )
        ]
    g = len(clusters)
    if g < 2:
        return {}, [
            (
                f"2 plateau W={w}",
                False,
                f"{g} run(s) with two or more tenths at x ≥ {PLATEAU_FROM[w]} leaves: a cluster-robust standard "
                "error needs 2 runs or more",
            )
        ]
    b, se, big_n, sxx = cluster_fit(clusters)
    if sxx == 0:
        return {}, [(f"2 plateau W={w}", False, "no within-run variation of x_rt")]
    fit = fitted(b, se, g - 1, e) | {"N": big_n, "G": g, "sxx": sxx}
    detail = (
        f"within-run (fixed effects, C.2): G {g} runs, N {big_n} tenths, b̂ {b:,.0f} B/leaf "
        f"({b / BLOCK_ROWS:.4f} B/row), cluster-robust SE {se:,.0f}, t₀.₉₅,{g - 1} {fit['t']:.3f}, "
        f"Σx̃² {sxx:,.0f}"
    )
    return fit, plateau_checks(w, fit, e, detail)


def statfs_type(path):
    """`statfs(2)` f_type of `path`'s filesystem."""
    out = subprocess.run(
        ["stat", "-f", "-c", "%t", str(path)],
        check=True,
        capture_output=True,
        text=True,
    )
    return int(out.stdout, 16)


def busy_ms(path):
    """Busy milliseconds of the block device holding `path` (field 10 of /sys/block/<dev>/stat), or None when the
    filesystem has no block device behind it (tmpfs, overlay)."""
    dev = os.stat(path).st_dev
    stat = Path(f"/sys/dev/block/{os.major(dev)}:{os.minor(dev)}/stat")
    return int(stat.read_text().split()[9]) if stat.exists() else None


def cgroup_snapshot():
    """The job cgroup's `memory.stat` anon and each process in it with its RssAnon, both in bytes. Taken by this
    process (which stays in the cgroup) before and after each run: anon the run didn't add is not the run's."""
    stat = Path("/sys/fs/cgroup/memory.stat").read_text()
    procs = []
    for pid in Path("/sys/fs/cgroup/cgroup.procs").read_text().split():
        try:
            status = Path(f"/proc/{pid}/status").read_text()
        except FileNotFoundError:
            continue
        rss = re.search(r"^RssAnon:\s+(\d+) kB", status, re.M)
        name = re.search(r"^Name:\s+(.*)$", status, re.M).group(1)
        procs.append([int(pid), name, int(rss.group(1)) * 1024 if rss else 0])
    return {
        "anon": int(re.search(r"^anon (\d+)$", stat, re.M).group(1)),
        "procs": procs,
    }


def tenth_peaks(samples, total):
    """Peak RssAnon over each tenth of the input, from (generated rows, RssAnon) samples: tenth t holds the samples
    while the row count is below (t + 1) · total / 10, and the last runs on through the seal. 0 marks a tenth with no
    sample, which criterion 2 treats as missing data."""
    tenths = [0] * 10
    for rows, rss in samples:
        t = min(9, rows * 10 // max(total, 1))
        tenths[t] = max(tenths[t], rss)
    return tenths


SAMPLE_LINE = re.compile(r"^sample rows=(\d+) RssAnon=(\d+)\b", re.M)


def log_tenths(text, total):
    """`tenth_peaks` from a run log's own `sample rows=… RssAnon=…` lines, the reproducer's ~1 s sampler. r1
    (`tessera-715-sweep-1`, the pre-#759 writer) recorded no tenths, and the 10 ms CSVs of its longer runs (256 and
    573 leaves at both W, 64 at W = 4) are over `ksb out`'s 1 MiB; the 64-leaf W = 1 one (481 KB) isn't. Every run's
    log fetches, so ADR-0064 C.4's r1 fit is rebuilt from the logs (`run.py log-tenths`), one sampler for every run."""
    samples = [(int(r), int(y)) for r, y in SAMPLE_LINE.findall(text)]
    if not samples:
        raise SystemExit("FAIL log: no `sample rows= RssAnon=` lines")
    return tenth_peaks(samples, total)


def log_extract(text, total, source):
    """The lines of a run log that `log_tenths` needs, for a small committed fixture: per tenth, the first sample line
    at that tenth's peak, under a provenance header (`source`, the log's sha256 and line count). `log_tenths` gives the
    same tenths from the extract as from the whole log."""
    tenths = log_tenths(text, total)
    kept, seen = [], set()
    for line in text.splitlines():
        m = SAMPLE_LINE.match(line)
        if m:
            t = min(9, int(m[1]) * 10 // max(total, 1))
            if int(m[2]) == tenths[t] and t not in seen:
                seen.add(t)
                kept.append(line)
    digest = hashlib.sha256(text.encode()).hexdigest()
    head = [
        f"# extract of {source}: sha256 {digest}, {len(text.splitlines())} lines",
        "# per tenth of input, the first `sample` line at that tenth's peak RssAnon (run.py log_extract)",
    ]
    return "\n".join(head + kept) + "\n"


def peaks(samples, stdout, total):
    """Peaks of one run's 10 ms samples, and the input row-group being generated at the RssAnon peak.
    The reproducer's own `peak_RssAnon` adds its reading after the seal, which a short run's samples miss."""
    with open(samples, newline="") as f:
        rows = [{k: int(v) for k, v in r.items()} for r in csv.DictReader(f)]
    final = re.search(r"peak_RssAnon=(\d+)", stdout)
    if not rows or final is None:
        raise SystemExit(
            f"FAIL sampler: {samples} has no samples or the run printed no peak_RssAnon"
        )
    top = max(rows, key=lambda r: r["rss_anon"])
    after_seal = int(final.group(1)) > top["rss_anon"]
    return {
        "samples": len(rows),
        "rss_anon": max(top["rss_anon"], int(final.group(1))),
        "peak_row_group": "after seal" if after_seal else top["rows"] // ROWS_PER_GROUP,
        # Where the peak grows is on the record (§8), and criterion 2 fits it (C.2).
        "tenths": tenth_peaks([(r["rows"], r["rss_anon"]) for r in rows], total),
        **{
            k: max(r[k] for r in rows)
            for k in ("memory_current", "anon", "file", "shmem")
        },
    }


def run_once(a, w, rows, rep, out, want_tmpfs):
    """One reproducer run with a fresh session dir, guarded for its filesystem; its product and session are deleted
    afterwards, so the next run shares neither its disk nor its page cache (nor, on tmpfs, its RAM)."""
    tag = f"w{w}-r{rows}-k{rep}"
    session = Path(a.session_root) / tag
    session.mkdir(parents=True)
    Path(a.output_root).mkdir(parents=True, exist_ok=True)
    # §8.1/A.3 guard the session dir; the output dir is guarded the same way, since an output on the other
    # filesystem would also measure the other job's setup (18 GB of tmpfs RAM at 2.4 G rows, or a disk in criterion 6).
    for what, path in (("session", session), ("output", Path(a.output_root))):
        magic = statfs_type(path)
        if (magic == TMPFS_MAGIC) != want_tmpfs:
            sys.exit(
                f"FAIL guard: {what} dir {path} has statfs f_type {magic:#x}; this job needs it "
                f"{'on' if want_tmpfs else 'off'} tmpfs ({TMPFS_MAGIC:#x})"
            )
    product = Path(a.output_root) / f"{tag}.tsra"
    before = cgroup_snapshot()
    busy0, started = busy_ms(session), time.monotonic()
    run = subprocess.run(
        [
            a.binary,
            "--rows",
            str(rows),
            "--workers",
            str(w),
            "--session-dir",
            str(session),
            "--output",
            str(product),
            "--samples",
            str(out / f"{tag}.csv"),
        ],
        capture_output=True,
        text=True,
    )
    wall, busy1 = time.monotonic() - started, busy_ms(session)
    (out / f"{tag}.log").write_text(run.stdout + run.stderr)
    record = {
        "workers": w,
        "rows": rows,
        "rep": rep,
        "exit": run.returncode,
        "wall_s": wall,
        "util": None if busy0 is None else (busy1 - busy0) / 1000 / wall,
        "memory_max": Path("/sys/fs/cgroup/memory.max").read_text().strip(),
        "session_fs": hex(statfs_type(session)),
        "output_fs": hex(statfs_type(a.output_root)),
        "cpus": len(os.sched_getaffinity(0)),
        "binary": str(Path(a.binary).resolve()),
        "cgroup_before": before,
        "cgroup_after": cgroup_snapshot(),
    }
    if run.returncode == 0:
        record["product_bytes"] = product.stat().st_size
        with zipfile.ZipFile(product) as z:
            manifest = z.read("manifest.json")
        (out / f"{tag}.manifest.json").write_bytes(manifest)
        record |= summary(json.loads(manifest))
        record |= peaks(out / f"{tag}.csv", run.stdout, rows)
    product.unlink(missing_ok=True)
    shutil.rmtree(session)
    util = "n/a" if record["util"] is None else f"{record['util']:.0%}"
    print(
        f"run W={w} rows={rows} k={rep}: exit {run.returncode}, {wall:.1f} s (device util {util}), RssAnon "
        f"{mib(record.get('rss_anon', 0))} at row-group {record.get('peak_row_group')}; per tenth of input "
        f"{[round(t / MIB, 1) for t in record.get('tenths', [])]} MiB",
        flush=True,
    )
    return record


def sweep(a, job, plan, want_tmpfs):
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    runs = []
    for w, rows, rep in plan:
        runs.append(run_once(a, w, rows, rep, out, want_tmpfs))
        (out / "records.json").write_text(
            json.dumps({"job": job, "runs": runs}, indent=1)
        )


class Verdicts:
    def __init__(self, runs, want_tmpfs):
        self.runs = runs
        self.want_tmpfs = want_tmpfs
        self.failed = []

    def verdict(self, name, ok, detail):
        print(f"{'PASS' if ok else 'FAIL'} {name}: {detail}")
        if not ok and name not in self.failed:
            self.failed.append(name)

    def need(self, name, w, rows, k=1):
        """The point's k completed runs, or None after failing `name` for a missing or failed one."""
        runs = [r for r in self.runs if (r["workers"], r["rows"]) == (w, rows)]
        done = [r for r in runs if r["exit"] == 0]
        if len(done) != k:
            self.verdict(
                name,
                False,
                f"W={w} rows={rows} needs {k} completed run(s), has {len(done)} (exits {[r['exit'] for r in runs]})",
            )
            return None
        for r in done:
            on = [
                r[f]
                for f in ("session_fs", "output_fs")
                if (int(r[f], 16) == TMPFS_MAGIC) != self.want_tmpfs
            ]
            if on:
                self.verdict(
                    name,
                    False,
                    f"W={w} rows={rows} ran on statfs f_type {on}, the other job's setup",
                )
                return None
            got = r["leaves"]
            if got != blocks(rows):
                self.verdict(
                    name,
                    False,
                    f"W={w} rows={rows} sealed {got} '{TABLE}' leaves, not {blocks(rows)}",
                )
                return None
        return done

    def close(self, what):
        print(
            f"VERDICT: {'FAIL ' + ', '.join(self.failed) if self.failed else 'PASS, ' + what}"
        )
        return 1 if self.failed else 0


def evaluate_proof(v):
    """Criteria 1-5 over the `job` records, and the A.3 disk context."""
    # 1. Ceiling, on every run; the 0- and 1-block runs calibrate R₀ and R₁.
    for w in WORKERS:
        cal = [v.need(f"1 ceiling W={w}", w, rows) for rows in ROWS[:2]]
        if None in cal:
            continue
        r0, r1 = cal[0][0]["rss_anon"], cal[1][0]["rss_anon"]
        print(
            f"CAL 1 ceiling W={w}: R₀ {mib(r0)} (0 rows), R₁ {mib(r1)} (one block) calibrate the bound"
        )
        for rows in ROWS[2:]:
            for i, r in enumerate(
                v.need(f"1 ceiling W={w} rows={rows}", w, rows, repeats(w, rows)) or []
            ):
                n, e = blocks(rows), r["e_leaf"]
                bound = r0 + min(w, n) * (r1 - r0) + S + s_rss(w) + c_leaf(e) * n
                v.verdict(
                    f"1 ceiling W={w} rows={rows} (n={n}) run {i + 1}",
                    r["rss_anon"] <= bound,
                    f"RssAnon {mib(r['rss_anon'])} (row-group {r['peak_row_group']}) ≤ R₀ {mib(r0)} + min(W, n) "
                    f"{min(w, n)} · (R₁ − R₀) {mib(r1 - r0)} + s {mib(S)} + s_rss {mib(s_rss(w))} + c_leaf "
                    f"{c_leaf(e)} B (e_leaf {e} B) · n {n} = {mib(bound)}",
                )

    # 2. Plateau, the OLS slope over every run at n ≥ 16 W, each repeat its own point (B.2, B.3).
    for w in WORKERS:
        sizes = [rows for rows in ROWS if blocks(rows) >= PLATEAU_FROM[w]]
        p = [v.need(f"2 plateau W={w}", w, rows, repeats(w, rows)) for rows in sizes]
        if None in p:
            continue
        runs = [r for point in p for r in point]
        e = max(r["e_leaf"] for r in runs)
        fit, checks = within_run_test(w, runs, e)
        for check in checks:
            v.verdict(*check)
        if fit and w != 1:
            print(
                f"LIMIT 2 plateau W={w}: detection floor {fit['floor']:,.0f} B/leaf "
                f"({fit['floor'] / BLOCK_ROWS:.4f} B/row), a stated limit, not a check (B.4)"
            )
        # C.3: the across-run slope (B.2) is reported, never graded: its peaks track the shared disk's load.
        _, context = slope_test(w, [(r["leaves"], r["rss_anon"]) for r in runs], e)
        print(
            f"CONTEXT 2 across-run W={w} (disk-confounded, not graded): "
            + "; ".join(
                f"{name.split(': ')[-1]} {'within' if ok else 'outside'} B.3's rule"
                for name, ok, _ in context
            )
            + f"; {context[0][2]}"
        )

    big = ROWS[-1]
    # 3. Page cache is reclaimable; 4. the consumer's budget. Every run of the point.
    for w in WORKERS:
        runs = v.need(f"3 reclaimable W={w}", w, big, repeats(w, big))
        if runs is None:
            v.verdict(f"4 budget W={w}", False, f"no completed runs at rows={big}")
            continue
        for i, r in enumerate(runs):
            at = f"W={w} rows={big} run {i + 1}"
            limit = r["memory_max"]
            v.verdict(
                f"3 reclaimable {at}: completes under 4 GiB",
                limit != "max" and int(limit) <= 4 * GIB,
                f"exit 0 with memory.max {limit} ≤ {4 * GIB}",
            )
            v.verdict(
                f"3 reclaimable {at}: shmem",
                r["shmem"] < 16 * MIB,
                f"peak shmem {mib(r['shmem'])} < 16 MiB",
            )
            # The cgroup also holds this process and the job's own (ksb measured 14-65 MiB of anon before a run's
            # first row), so the run's anon is the peak minus the anon already there when it was spawned.
            base = r.get("cgroup_before")
            if base is None:
                v.verdict(
                    f"3 reclaimable {at}: anon ≈ RssAnon",
                    False,
                    "no cgroup anon baseline recorded",
                )
            else:
                gap = abs(r["anon"] - base["anon"] - r["rss_anon"])
                others = ", ".join(
                    f"{name} {mib(rss)}" for _, name, rss in base["procs"]
                )
                v.verdict(
                    f"3 reclaimable {at}: anon ≈ RssAnon",
                    gap <= s_rss(w),
                    f"|cgroup anon {mib(r['anon'])} − anon before the run {mib(base['anon'])} − RssAnon "
                    f"{mib(r['rss_anon'])}| = {mib(gap)} ≤ s_rss {mib(s_rss(w))} (in the cgroup before: {others}; "
                    f"after: anon {mib(r['cgroup_after']['anon'])}; memory.current peak {mib(r['memory_current'])}, "
                    f"file {mib(r['file'])}, device util {'n/a' if r['util'] is None else format(r['util'], '.0%')})",
                )
            v.verdict(
                f"4 budget {at}",
                r["rss_anon"] < 8 * GIB,
                f"RssAnon {mib(r['rss_anon'])} < 8 GiB",
            )

    # 5. Bytes and names: canonical manifests minus the §6 fields, and content_hash. Every run at 268 M rows, both W
    # and every repeat, against W = 1's first: equality is transitive, so that covers every W = 1 / W = 4 pair.
    one, four = (
        v.need("5 bytes and names", 1, MID, repeats(1, MID)),
        v.need("5 bytes and names", 4, MID, repeats(4, MID)),
    )
    if one is not None and four is not None:
        ref = one[0]
        c1 = ref["canonical"]
        for r in one[1:] + four:
            c = r["canonical"]
            differ = sorted(k for k in c1.keys() | c.keys() if c1.get(k) != c.get(k))
            v.verdict(
                f"5 bytes and names rows={MID} W={r['workers']} run {r['rep'] + 1} vs W=1 run {ref['rep'] + 1}",
                not differ and ref["content_hash"] == r["content_hash"],
                f"canonical manifests differ in {differ or 'nothing'} ({ref['blocks']} blocks, keys {sorted(c1)}), "
                f"content_hash {ref['content_hash']} vs {r['content_hash']}",
            )

    # A.3/A.4 context, never a verdict: the shared scratch disk's wall times and %util at 268 M rows.
    for r in v.runs:
        if r["rows"] == MID:
            util = "n/a" if r["util"] is None else f"{r['util']:.0%}"
            print(
                f"CONTEXT disk W={r['workers']} rows={MID} k={r['rep']}: wall {r['wall_s']:.1f} s, device util {util}"
            )
    return v.close("criteria 1-5")


def evaluate_parallel(v):
    """Criterion 6 over the `parallel` records (session and output on /dev/shm)."""
    one, four = v.need("6 parallelism", 1, MID), v.need("6 parallelism", 4, MID)
    if one is not None and four is not None:
        cpus = [r["cpus"] for r in one + four]
        v.verdict(
            f"6 parallelism rows={MID}: CPUs",
            cpus == [CPUS, CPUS],
            f"runs had {cpus} CPUs, need {CPUS} each",
        )
        t1, t4 = one[0]["wall_s"], four[0]["wall_s"]
        v.verdict(
            f"6 parallelism rows={MID} on tmpfs",
            t1 >= SPEEDUP * t4,
            f"wall W=1 {t1:.1f} s / W=4 {t4:.1f} s = {t1 / t4:.2f}× ≥ {SPEEDUP}× (session fs "
            f"{one[0]['session_fs']}/{four[0]['session_fs']}, memory.max {four[0]['memory_max']}, "
            f"product T {one[0]['product_bytes'] / GIB:.2f} GiB)",
        )
    return v.close("criterion 6")


def load(dirs):
    """The runs of one job's records, merged over its shards' dirs. Shards are one job split for wall time: they
    must be the same job kind, the same binary (R₀ and R₁ calibrate it, §8.1), and no point's repeat twice."""
    jobs, runs = set(), []
    for d in dirs:
        records = json.loads(Path(d, "records.json").read_text())
        jobs.add(records["job"])
        runs += records["runs"]
    if len(jobs) != 1:
        raise SystemExit(
            f"FAIL merge: the dirs hold records of jobs {sorted(jobs)}, not one"
        )
    binaries = {r.get("binary") for r in runs}
    if len(dirs) > 1 and (None in binaries or len(binaries) != 1):
        raise SystemExit(
            f"FAIL merge: shards ran binaries {sorted(map(str, binaries))}, not one"
        )
    keys = [(r["workers"], r["rows"], r["rep"]) for r in runs]
    dup = sorted({k for k in keys if keys.count(k) > 1})
    if dup:
        raise SystemExit(f"FAIL merge: (W, rows, rep) {dup} recorded more than once")
    return jobs.pop(), runs


def job_plan(workers=WORKERS, rows=ROWS, reps=None):
    """The `job` runs as (W, rows, rep), in run order; with the defaults, the full design of criteria 1-5, and
    anything less is a shard."""
    return [
        (w, r, k)
        for w in workers
        for r in rows
        for k in range(repeats(w, r))
        if reps is None or k in reps
    ]


def check_shard(out, plan):
    """A shard's own exit code: each planned run completed, on the right filesystem, with the right leaf count.
    The criteria need every shard's runs, so `evaluate DIR...` checks them; this is not a criterion."""
    _, runs = load([out])
    v = Verdicts(runs, want_tmpfs=False)
    for w, rows in sorted({(w, rows) for w, rows, _ in plan}):
        v.need(
            f"shard W={w} rows={rows}",
            w,
            rows,
            sum(1 for p in plan if p[:2] == (w, rows)),
        )
    return v.close(
        f"shard of {len(plan)} runs (criteria: `evaluate` over every shard's records)"
    )


LOG_NAME = re.compile(r"^w(\d+)-r(\d+)(?:-k(\d+))?\.log$")


def evaluate_logs(paths):
    """Criterion 2's within-run fit per W over run logs (`log_tenths`), the runs at n ≥ 16 W, e_leaf at r3's 2,966 B
    since a log holds no manifest. Exits 1 if any W's checks fail. Only criterion 2: a log holds nothing else."""
    runs = {}
    for path in paths:
        m = LOG_NAME.match(Path(path).name)
        if m is None:
            raise SystemExit(f"FAIL log: {path} isn't named w<W>-r<rows>[-k<rep>].log")
        w, rows, rep = int(m[1]), int(m[2]), int(m[3] or 0)
        if blocks(rows) >= PLATEAU_FROM[w]:
            tenths = log_tenths(Path(path).read_text(), rows)
            runs.setdefault(w, []).append({"rows": rows, "rep": rep, "tenths": tenths})
    v = Verdicts([], want_tmpfs=False)
    for w in sorted(runs):
        for check in within_run_test(w, runs[w], E_LEAF_LOGS)[1]:
            v.verdict(*check)
    return v.close("criterion 2 on the logs")


def evaluate(*dirs):
    job, runs = load(dirs)
    check, want_tmpfs = {
        "proof": (evaluate_proof, False),
        "parallel": (evaluate_parallel, True),
    }[job]
    return check(Verdicts(runs, want_tmpfs))


def main():
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument(
        "--out",
        default=os.environ.get("KSB_JOB_OUT"),
        help="records dir (default $KSB_JOB_OUT)",
    )
    sub = p.add_subparsers(dest="cmd", required=True)
    for cmd, what in (
        ("job", "criteria 1-5, staging on disk"),
        ("parallel", "criterion 6, staging on /dev/shm"),
    ):
        s = sub.add_parser(cmd, help=f"run {what}, then evaluate it")
        s.add_argument(
            "--binary", required=True, help="the patched consumer reproducer"
        )
        s.add_argument(
            "--session-root",
            required=True,
            help="dir for the session dirs (its fs is guarded)",
        )
        s.add_argument(
            "--output-root",
            required=True,
            help="dir for the sealed products (deleted per run)",
        )
    job = sub.choices["job"]
    job.add_argument("--workers", type=int, nargs="+", default=WORKERS)
    job.add_argument("--rows", type=int, nargs="+", default=ROWS)
    job.add_argument(
        "--reps",
        type=int,
        nargs="+",
        help="a shard: only these repeat indices of each point (index i runs where i < the point's k); the "
        "job then checks its own runs only, and `evaluate DIR...` checks the criteria over all shards",
    )
    ev = sub.add_parser(
        "evaluate",
        help="check the criteria over the records.json of --out, or of each DIR",
    )
    ev.add_argument("dirs", nargs="*", help="records dirs of one job's shards, merged")
    lt = sub.add_parser(
        "log-tenths",
        help="criterion 2's within-run fit (C.2) on run logs' own sample lines, for runs that recorded no tenths "
        "(r1, ADR-0064 C.4); each LOG is named w<W>-r<rows>.log as `job` writes it",
    )
    lt.add_argument("logs", nargs="+")
    lt.add_argument(
        "--extract",
        metavar="DIR",
        help="also write each log's extract (log_extract) to DIR, under its own name",
    )
    lt.add_argument(
        "--source",
        default="tessera-715-sweep-1",
        help="the job the logs came from, for the extracts' provenance",
    )
    a = p.parse_args()
    if a.cmd == "log-tenths":
        if a.extract:
            for path in a.logs:
                rows = int(LOG_NAME.match(Path(path).name)[2])
                Path(a.extract, Path(path).name).write_text(
                    log_extract(
                        Path(path).read_text(), rows, f"{a.source} {Path(path).name}"
                    )
                )
        sys.exit(evaluate_logs(a.logs))
    if a.out is None and not getattr(a, "dirs", None):
        p.error("--out or $KSB_JOB_OUT is required")
    if a.cmd == "job":
        plan = job_plan(a.workers, a.rows, a.reps)
        sweep(a, "proof", plan, want_tmpfs=False)
        if plan != job_plan():
            sys.exit(check_shard(a.out, plan))
    elif a.cmd == "parallel":
        sweep(a, "parallel", [(1, MID, 0), (4, MID, 0)], want_tmpfs=True)
    sys.exit(evaluate(*(getattr(a, "dirs", None) or [a.out])))


if __name__ == "__main__":
    main()
