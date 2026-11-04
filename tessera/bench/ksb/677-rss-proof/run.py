#!/usr/bin/env python3
"""ADR-0064 §8.1, as amended by #760: the RSS proof of `StreamWriter::push_table` with the consumer reproducer (#715).

Two ksb jobs, each a subcommand that runs the reproducer, writes `records.json` to `--out` and then evaluates it:

- `job` (criteria 1-5): the sweep, with every session and output directory on a volume that `statfs` shows is NOT
  tmpfs (else the job fails before any row is written). Each W = 4 point of 64 blocks or more runs 3 times (A.2),
  every other point once. Per run it records the exit code, the 10 ms samples' peaks, the input row-group at the
  `RssAnon` peak, the CPUs it may run on, the cgroup's anon and processes just before and after it (criterion 3
  counts only the anon the run added) and the sealed manifest (kept as `<run>.manifest.json`, summarised in
  `records.json`); at 268 M rows it also prints the wall times and the session device's `%util` as context (A.3,
  A.4): timings on a shared disk are never checked.
- `parallel` (criterion 6): the W = 1 and W = 4 runs at 268 M rows with the session and output directories on
  `/dev/shm`. Its guard is the converse one (A.3): they MUST be tmpfs, else the job fails, so neither job can
  silently measure the other's setup. The speed-up only counts with the 4 CPUs of `--cpu 4 --exclusive-cpus`.

`records.json` is rewritten after every run, so a job that dies keeps the runs it finished. `run.py --out DIR
evaluate` (`--out` before the subcommand, default `$KSB_JOB_OUT`) re-checks one offline, the guards included. Every
criterion prints its numbers and bound; the exit code is 1 if any fails. A criterion whose runs are missing or failed
fails: it is never passed on no data.
"""

import argparse
import csv
import hashlib
import json
import os
import re
import shutil
import statistics
import subprocess
import sys
import time
import zipfile
from itertools import combinations
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
PLATEAU_FROM = {
    1: 16,
    4: 64,
}  # criterion 2: n ≥ 16 W blocks at W = 4, from 16 blocks at W = 1
TAU = 0.05
SPEEDUP = 1.5  # criterion 6


def s_rss(w):
    """§8.1: allocator retention and thread stacks the heap model doesn't see."""
    return 32 * MIB + 8 * MIB * (w + 1)


def blocks(rows):
    """The leaves push_table writes for `rows` (tessera_io::table::partition_blocks: 0 rows is one empty block)."""
    return max(1, -(-rows // BLOCK_ROWS))


def repeats(w, rows):
    """A.2: 3 runs of each W = 4 point at 64 blocks or more, 1 of every other point."""
    return REPEATS if w == 4 and blocks(rows) >= 64 else 1


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
    # Peak RssAnon over each tenth of the input, so where the peak grows is on the record (§8).
    tenths = [
        [r["rss_anon"] for r in rows if min(9, r["rows"] * 10 // max(total, 1)) == i]
        for i in range(10)
    ]
    after_seal = int(final.group(1)) > top["rss_anon"]
    return {
        "samples": len(rows),
        "rss_anon": max(top["rss_anon"], int(final.group(1))),
        "peak_row_group": "after seal" if after_seal else top["rows"] // ROWS_PER_GROUP,
        "tenths": [max(t, default=0) for t in tenths],
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

    # 2. Plateau, on the medians of a W = 4 point's 3 runs and on W = 1's single runs.
    for w in WORKERS:
        sizes = [rows for rows in ROWS if blocks(rows) >= PLATEAU_FROM[w]]
        for n1, n2 in combinations(sizes, 2):
            name = f"2 plateau W={w} n={blocks(n1)}→{blocks(n2)}"
            p = [v.need(name, w, rows, repeats(w, rows)) for rows in (n1, n2)]
            if None in p:
                continue
            m1, m2 = (statistics.median(r["rss_anon"] for r in runs) for runs in p)
            e = max(r["e_leaf"] for runs in p for r in runs)
            dn = blocks(n2) - blocks(n1)
            bound = (1 + TAU) * m1 + c_leaf(e) * dn
            med = "median of 3 " if len(p[0]) > 1 else ""
            v.verdict(
                name,
                m2 <= bound,
                f"{med}RssAnon {mib(m2)} ≤ 1.05 · {mib(m1)} + c_leaf {c_leaf(e)} B (e_leaf {e} B) · (n₂ − n₁) "
                f"{dn} = {mib(bound)}; runs {[mib(r['rss_anon']) for r in p[0]]} → {[mib(r['rss_anon']) for r in p[1]]}",
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

    # 5. Bytes and names: canonical manifests minus the §6 fields, and content_hash, W = 1 against each W = 4 run.
    one, four = (
        v.need("5 bytes and names", 1, MID),
        v.need("5 bytes and names", 4, MID, repeats(4, MID)),
    )
    if one is not None and four is not None:
        c1 = one[0]["canonical"]
        for i, r4 in enumerate(four):
            c4 = r4["canonical"]
            differ = sorted(k for k in c1.keys() | c4.keys() if c1.get(k) != c4.get(k))
            v.verdict(
                f"5 bytes and names rows={MID} W=4 run {i + 1}",
                not differ and one[0]["content_hash"] == r4["content_hash"],
                f"canonical manifests W=1 vs W=4 differ in {differ or 'nothing'} ({one[0]['blocks']} blocks, "
                f"keys {sorted(c1)}), content_hash {one[0]['content_hash']} vs {r4['content_hash']}",
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


def evaluate(out):
    records = json.loads(Path(out, "records.json").read_text())
    check, want_tmpfs = {
        "proof": (evaluate_proof, False),
        "parallel": (evaluate_parallel, True),
    }[records["job"]]
    return check(Verdicts(records["runs"], want_tmpfs))


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
    sub.add_parser("evaluate", help="check the criteria over --out/records.json")
    a = p.parse_args()
    if a.out is None:
        p.error("--out or $KSB_JOB_OUT is required")
    if a.cmd == "job":
        plan = [
            (w, rows, k)
            for w in a.workers
            for rows in a.rows
            for k in range(repeats(w, rows))
        ]
        sweep(a, "proof", plan, want_tmpfs=False)
    elif a.cmd == "parallel":
        sweep(a, "parallel", [(1, MID, 0), (4, MID, 0)], want_tmpfs=True)
    sys.exit(evaluate(a.out))


if __name__ == "__main__":
    main()
