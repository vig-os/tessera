"""#828 round 2 job driver: the A3 re-measure (v1 vs v2) and the M4b sweep, in one reserved ksb slice.

Protocol (owner note on #828, 2026-11-22): load-independent cost first (CPU seconds and user-space instructions
retired, per event, around the timed section only), wall time second; v1 and v2 interleaved, >= 5 rounds, median
and min-max; binaries in the nix store, inputs and outputs on the job's local scratch, one pinned core.

Env (set by the flake): V1, V2 (table_profile binaries), KSB_JOB_OUT. The compressed #822 columns are staged by
submit.sh into $KSB_JOB_OUT/fixture (a READY marker ends the copy): copying them through the nix snapshot timed out
in preparation (tessera-828-m4b-probe-1). The job verifies them, decompresses them to its local scratch and deletes
the staged copy before measuring.
Usage: run.py [probe|full] [--rounds N]
"""

import argparse
import hashlib
import json
import os
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

V1, V2 = os.environ["V1"], os.environ["V2"]
OUT = Path(os.environ.get("KSB_JOB_OUT", "out"))
FIXTURE = Path(os.environ.get("FIXTURE", OUT / "fixture"))
OUT.mkdir(parents=True, exist_ok=True)
WORK = Path(tempfile.mkdtemp(prefix="t828-", dir=os.environ.get("TMPDIR", "/tmp")))
RESULTS = OUT / "results.jsonl"
CPU = min(os.sched_getaffinity(0))
# Round-1 table bytes (ksb-ws, same heads): a build that disagrees is not the code under test.
EXPECT_BYTES = {
    ("ge", "v1"): 211421352,
    ("ge", "v2"): 174329624,
    ("siemens", "v1"): 412902352,
    ("siemens", "v2"): 382995576,
}
# M4b: Pco level x values per page x numeric chunk framing (rows per Pco chunk / Zstd frame).
POINTS = [
    {"T828_PCO_LEVEL": lvl, "T828_PAGE": page, "T828_CHUNK_ROWS": chunk}
    for lvl in (4, 6, 8)
    for page in (8192, 65536)
    for chunk in (65536, 262144)
]


def log(msg: str) -> None:
    sys.stdout.write(f"[{time.strftime('%H:%M:%S')}] {msg}\n")
    sys.stdout.flush()


def host_state() -> dict:
    def read(p: str) -> str:
        try:
            return Path(p).read_text().strip()
        except OSError as e:
            return f"unreadable: {e}"

    return {
        "loadavg": read("/proc/loadavg"),
        "psi_cpu": read("/proc/pressure/cpu"),
        "psi_io": read("/proc/pressure/io"),
    }


def perf_works() -> bool:
    r = subprocess.run(
        ["perf", "stat", "-x", ",", "-e", "instructions:u", "true"],
        capture_output=True,
        text=True,
    )
    ok = (
        r.returncode == 0
        and "not supported" not in r.stderr
        and "<not counted>" not in r.stderr
    )
    log(f"perf probe rc={r.returncode} ok={ok}: {r.stderr.strip()[:300]}")
    return ok


PERF = False


def run(tag: str, binary: str, args: list[str], env_extra: dict | None = None) -> dict:
    """One measured invocation, pinned to CPU, counters enabled for the harness's timed section only."""
    env = dict(os.environ)
    env.update({k: str(v) for k, v in (env_extra or {}).items()})
    cmd = ["taskset", "-c", str(CPU), binary, *args]
    perf_out = WORK / "perf.txt"
    if PERF:
        ctl, ack = WORK / "perf.ctl", WORK / "perf.ack"
        for f in (ctl, ack):
            if f.exists():
                f.unlink()
            os.mkfifo(f)
        env.update({"T828_PERF_CTL": str(ctl), "T828_PERF_ACK": str(ack)})
        cmd = [
            "perf",
            "stat",
            "-x",
            ",",
            "-e",
            "instructions:u,task-clock",
            "--delay=-1",
            "--control",
            f"fifo:{ctl},{ack}",
            "-o",
            str(perf_out),
            "--",
            *cmd,
        ]
    before = host_state()
    r = subprocess.run(cmd, env=env, capture_output=True, text=True)
    after = host_state()
    if r.returncode != 0:
        log(f"{tag}: rc={r.returncode}\n{r.stderr[-3000:]}")
        raise SystemExit(f"{tag} failed")
    rec = {
        "tag": tag,
        "env": env_extra or {},
        "r": json.loads(r.stdout.strip().splitlines()[-1]),
        "before": before,
        "after": after,
    }
    if PERF:
        counters = {}
        for line in perf_out.read_text().splitlines():
            f = line.split(",")
            if len(f) > 2 and f[0] and not line.startswith("#"):
                try:
                    counters[f[2]] = float(f[0])
                except ValueError:
                    counters[f[2]] = None
        rec["perf"] = counters
    with RESULTS.open("a") as fh:
        fh.write(json.dumps(rec) + "\n")
    return rec


def instr(rec: dict) -> float | None:
    v = (rec.get("perf") or {}).get("instructions:u")
    return v


def setup() -> None:
    deadline = time.time() + 3 * 3600
    while not (FIXTURE / "READY").exists():
        if time.time() > deadline:
            raise SystemExit(f"no fixture staged at {FIXTURE} (READY marker missing)")
        time.sleep(10)
    for vendor in ("ge", "siemens"):
        d = WORK / vendor
        d.mkdir()
        meta = json.loads((FIXTURE / vendor / "events.json").read_text())
        (d / "events.json").write_text(json.dumps(meta))
        for c in meta["columns"]:
            dst = d / c["file"]
            with dst.open("wb") as fh:
                subprocess.run(
                    [
                        "zstd",
                        "-q",
                        "-d",
                        "-c",
                        str(FIXTURE / vendor / (c["file"] + ".zst")),
                    ],
                    stdout=fh,
                    check=True,
                )
            b = dst.read_bytes()
            if (
                vendor == "ge"
            ):  # GE manifest hashes the .npy payload (after the 10 + header_len byte header)
                hl = int.from_bytes(b[8:10], "little")
                ok = hashlib.sha256(b[10 + hl :]).hexdigest() == c["sha256"]
            else:  # Siemens ships sha256 of the whole decompressed .npy
                want = {
                    ln.split()[1].split("/")[-1]: ln.split()[0]
                    for ln in (FIXTURE / vendor / "NPY_DECOMPRESSED.sha256")
                    .read_text()
                    .splitlines()
                }
                ok = hashlib.sha256(b).hexdigest() == want[c["file"]]
            if not ok:
                raise SystemExit(f"fixture {vendor}/{c['file']}: sha256 mismatch")
        log(f"fixture {vendor}: {len(meta['columns'])} columns verified")


def summarise(rows: list[dict], key) -> dict:
    vals = [key(r) for r in rows if key(r) is not None]
    if not vals:
        return {}
    return {
        "median": statistics.median(vals),
        "min": min(vals),
        "max": max(vals),
        "n": len(vals),
    }


def write_stats(recs: list[dict]) -> dict:
    ev = recs[0]["r"]["events"]
    return {
        "bytes_per_event": recs[0]["r"]["bytes_per_event"],
        "table_bytes": sorted({r["r"]["table_bytes"] for r in recs}),
        "cpu_ns_per_event": summarise(recs, lambda r: r["r"]["cpu_ns_per_event"]),
        "instructions_per_event": summarise(
            recs, lambda r: instr(r) / ev if instr(r) else None
        ),
        "wall_mev_per_s": summarise(recs, lambda r: r["r"]["mev_per_s"]),
    }


def a3(rounds: int) -> dict:
    out = {}
    for vendor in ("ge", "siemens"):
        recs = {"v1": [], "v2": []}
        dec = {"v1": [], "v2": []}
        for i in range(rounds):
            for side, binary in (("v1", V1), ("v2", V2)):
                f = WORK / f"a3-{vendor}-{side}.tsra"
                if f.exists():
                    f.unlink()
                rec = run(
                    f"a3-write-{vendor}-{side}-{i}",
                    binary,
                    ["write", str(WORK / vendor), str(f), "1"],
                )
                if rec["r"]["table_bytes"] != EXPECT_BYTES[(vendor, side)]:
                    raise SystemExit(
                        f"{vendor} {side}: {rec['r']['table_bytes']} bytes, round 1 wrote "
                        f"{EXPECT_BYTES[(vendor, side)]}: not the code under test"
                    )
                recs[side].append(rec)
                log(
                    f"a3 {vendor} {side} round {i}: cpu {rec['r']['cpu_ns_per_event']:.1f} ns/ev, "
                    f"instr {instr(rec)}"
                )
        for i in range(rounds):
            for side, binary in (("v1", V1), ("v2", V2)):
                dec[side].append(
                    run(
                        f"a3-decode-{vendor}-{side}-{i}",
                        binary,
                        ["decode", str(WORK / f"a3-{vendor}-{side}.tsra"), "3"],
                    )
                )
        takes = {
            side: run(
                f"a3-take-{vendor}-{side}",
                binary,
                ["take", str(WORK / f"a3-{vendor}-{side}.tsra"), "5"],
            )
            for side, binary in (("v1", V1), ("v2", V2))
        }
        out[vendor] = {
            side: {
                "write": write_stats(recs[side]),
                "decode_cpu_mib_per_s": summarise(
                    dec[side], lambda r: r["r"]["cpu_mib_per_s"]
                ),
                "decode_wall_mib_per_s": summarise(
                    dec[side], lambda r: r["r"]["mib_per_s"]
                ),
                "decode_instructions_per_event": summarise(
                    dec[side],
                    lambda r: instr(r) / r["r"]["events"] / 3 if instr(r) else None,
                ),
                "take_ms": {
                    k: v["median_ms"] for k, v in takes[side]["r"]["cases"].items()
                },
            }
            for side in ("v1", "v2")
        }
        for side in ("v1", "v2"):
            f = WORK / f"a3-{vendor}-{side}.tsra"
            if f.exists():
                f.unlink()
    return out


def m4b(rounds: int) -> dict:
    out = {}
    for vendor in ("ge", "siemens"):
        recs = {i: [] for i in range(len(POINTS))}
        extra = {}
        for rnd in range(rounds):
            for i, pt in enumerate(POINTS):
                f = WORK / f"m4b-{vendor}-{i}.tsra"
                if f.exists():
                    f.unlink()
                rec = run(
                    f"m4b-write-{vendor}-{i}-{rnd}",
                    V2,
                    ["write", str(WORK / vendor), str(f), "1"],
                    pt,
                )
                recs[i].append(rec)
                log(
                    f"m4b {vendor} {pt} round {rnd}: {rec['r']['bytes_per_event']:.4f} B/ev, "
                    f"cpu {rec['r']['cpu_ns_per_event']:.1f} ns/ev"
                )
                if rnd == rounds - 1:
                    d = run(f"m4b-decode-{vendor}-{i}", V2, ["decode", str(f), "3"], pt)
                    t = run(f"m4b-take-{vendor}-{i}", V2, ["take", str(f), "5"], pt)
                    extra[i] = {
                        "decode_cpu_mib_per_s": d["r"]["cpu_mib_per_s"],
                        "decode_wall_mib_per_s": d["r"]["mib_per_s"],
                        "take_500_ms": t["r"]["cases"]["uniform_500"]["median_ms"],
                        "take_1_ms": t["r"]["cases"]["uniform_1"]["median_ms"],
                    }
                f.unlink()
        out[vendor] = [
            {"point": POINTS[i], **write_stats(recs[i]), **extra[i]}
            for i in range(len(POINTS))
        ]
    return out


def main() -> None:
    global PERF
    ap = argparse.ArgumentParser()
    ap.add_argument("mode", nargs="?", default="full", choices=["probe", "full"])
    ap.add_argument("--rounds", type=int, default=5)
    a = ap.parse_args()
    info = {
        "nproc_affinity": sorted(os.sched_getaffinity(0)),
        "pinned_cpu": CPU,
        "ksb_cpus": os.environ.get("KSB_CPUS"),
        "cpu_model": next(
            (
                ln.split(":", 1)[1].strip()
                for ln in Path("/proc/cpuinfo").read_text().splitlines()
                if ln.startswith("model name")
            ),
            "?",
        ),
        "clk_tck": str(os.sysconf("SC_CLK_TCK")),
        "host_start": host_state(),
        "v1": V1,
        "v2": V2,
    }
    PERF = perf_works()
    info["perf_instructions"] = PERF
    log(json.dumps(info))
    (OUT / "info.json").write_text(json.dumps(info, indent=2))
    if info["clk_tck"] != "100":
        raise SystemExit(
            f"CLK_TCK {info['clk_tck']} != 100: the harness's CPU seconds would be wrong"
        )
    setup()
    drop_staged_fixture()
    if a.mode == "probe":
        rec = run(
            "probe-write-ge-v2",
            V2,
            ["write", str(WORK / "ge"), str(WORK / "probe.tsra"), "1"],
        )
        log(json.dumps(rec))
        return
    summary = {"info": info, "a3": a3(a.rounds)}
    (OUT / "summary.json").write_text(json.dumps(summary, indent=2))
    summary["m4b"] = m4b(a.rounds)
    summary["host_end"] = host_state()
    (OUT / "summary.json").write_text(json.dumps(summary, indent=2))
    log("done")


def drop_staged_fixture() -> None:
    """The private fixture never stays in the job's results, whatever the outcome."""
    if FIXTURE == OUT / "fixture" and FIXTURE.exists():
        import shutil

        shutil.rmtree(FIXTURE)
        log("staged fixture removed from the job output")


if __name__ == "__main__":
    try:
        main()
    finally:
        drop_staged_fixture()
