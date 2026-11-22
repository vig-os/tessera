"""Render the #828 round-2 job's summary.json as the Markdown tables of the measurement report.

Usage: python3 -I report.py <summary.json>
"""

import json
import sys

s = json.load(open(sys.argv[1]))
lines: list[str] = []
emit = lines.append


def mmm(d: dict, fmt: str = "{:.1f}") -> str:
    if not d:
        return "—"
    return f"{fmt.format(d['median'])} ({fmt.format(d['min'])}–{fmt.format(d['max'])})"


def ratio(a: dict, b: dict) -> str:
    if not a or not b:
        return "—"
    return f"{a['median'] / b['median']:.2f}×"


emit("### A3 re-measure: v1 vs v2, interleaved, median (min–max)\n")
emit(
    "| fixture | profile | B/event | CPU ns/event | user instructions/event | wall M ev/s | "
    "decode CPU MiB/s | take-500 ms |"
)
emit("|---|---|---:|---:|---:|---:|---:|---:|")
for vendor, sides in s["a3"].items():
    for side, r in sides.items():
        w = r["write"]
        emit(
            f"| {vendor} | {side} | {w['bytes_per_event']:.4f} | {mmm(w['cpu_ns_per_event'])} | "
            f"{mmm(w['instructions_per_event'], '{:.0f}')} | {mmm(w['wall_mev_per_s'], '{:.2f}')} | "
            f"{mmm(r['decode_cpu_mib_per_s'], '{:.0f}')} | {r['take_ms']['uniform_500']:.1f} |"
        )
    v1, v2 = sides["v1"]["write"], sides["v2"]["write"]
    emit(
        f"| {vendor} | **v1/v2 cost ratio** (≥ 1 means v2 is at least as fast) | | "
        f"{ratio(v1['cpu_ns_per_event'], v2['cpu_ns_per_event'])} | "
        f"{ratio(v1['instructions_per_event'], v2['instructions_per_event'])} | "
        f"{ratio(v2['wall_mev_per_s'], v1['wall_mev_per_s'])} | | |"
    )

if "m4b" in s:
    for vendor, pts in s["m4b"].items():
        emit(
            f"\n### M4b sweep, {vendor} (v2 writer, median (min–max) of the interleaved rounds)\n"
        )
        emit(
            "| Pco level | values/page | chunk rows | B/event (A1) | CPU ns/event (A3) | instructions/event | "
            "wall M ev/s | decode CPU MiB/s (A5) | take-500 ms (A4) | take-1 ms |"
        )
        emit("|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|")
        for p in pts:
            pt = p["point"]
            emit(
                f"| {pt['T828_PCO_LEVEL']} | {pt['T828_PAGE']} | {pt['T828_CHUNK_ROWS']} | "
                f"{p['bytes_per_event']:.4f} | {mmm(p['cpu_ns_per_event'])} | "
                f"{mmm(p['instructions_per_event'], '{:.0f}')} | {mmm(p['wall_mev_per_s'], '{:.2f}')} | "
                f"{p['decode_cpu_mib_per_s']:.0f} | {p['take_500_ms']:.1f} | {p['take_1_ms']:.1f} |"
            )

sys.stdout.write("\n".join(lines) + "\n")
