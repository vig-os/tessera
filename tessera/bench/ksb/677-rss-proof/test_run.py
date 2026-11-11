"""Self-test of run.py's criterion 2 (ADR-0064 §8.1 as amended by #770, B.2-B.5, and #797, C.1-C.4) on recorded ksb
data.

The r3 numbers are `tessera-715-r3-sweep-2`'s records.json (PR #767 at 0b66160, the post-#759 writer), exact bytes of
peak RssAnon. The r1 numbers are `tessera-715-sweep-1`'s job log (tessera a3df683, the pre-#759 writer), at the log's
0.1 MiB precision: its records.json is over `ksb out`'s 1 MiB. ADR-0064 B.4 tabulates both; these tests pin run.py to
that table, so the evaluator and the ADR can't drift apart. Since #797 that across-run fit is context, and criterion 2
is the within-run fit (C.2); its fail-before (C.4) is the pre-#759 writer on r1's tenths, rebuilt from its logs'
sample lines (the extracts in fixtures/r1-logs, with each source log's sha256), and on
`tessera-715-r3-failbefore-2`'s per-tenth records, exact bytes.
"""

import hashlib
import json
import random
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).parent))
import run  # noqa: E402

MIB = 1 << 20
E_LEAF = 2966  # r3's largest pairs leaf entry, so c_leaf = 7,980 B (B.3)
R3 = {
    4: [
        (64, 189_734_912),
        (64, 198_742_016),
        (64, 199_442_432),
        (256, 198_324_224),
        (256, 215_244_800),
        (256, 212_709_376),
        (573, 200_847_360),
        (573, 205_373_440),
        (573, 216_244_224),
    ],
    1: [(16, 62_783_488), (64, 65_560_576), (256, 62_894_080), (573, 67_293_184)],
}
R1_W1 = [(16, 59.1 * MIB), (64, 59.5 * MIB), (256, 64.1 * MIB), (573, 71.7 * MIB)]
# tessera-715-r3-failbefore-2, the pre-#759 writer at W = 1: each run's peak RssAnon per tenth of its input (C.4).
R3_FAILBEFORE = {
    268_435_456: [60936192, 61616128, 61587456, 62013440, 61657088]
    + [62242816, 62394368, 62427136, 63373312, 62779392],
    2_400_000_000: [62492672, 64000000, 65454080, 65544192, 66584576]
    + [67993600, 68558848, 69795840, 70447104, 71876608],
}


def verdicts(w, points, e=E_LEAF):
    fit, checks = run.slope_test(w, points, e)
    return fit, {name.split(": ")[-1]: ok for name, ok, _ in checks}


def grown(points, per_leaf):
    """A growth of `per_leaf` bytes per leaf, added to every run (B.4)."""
    return [(n, y + per_leaf * n) for n, y in points]


@pytest.mark.parametrize(
    "df,want",
    [
        (1, 6.314),
        (2, 2.920),
        (7, 1.895),
        (10, 1.812),
        (14, 1.761),
        (22, 1.717),
        (100, 1.660),
    ],
)
def test_t_quantile_matches_the_table(df, want):
    assert run.t_quantile(0.95, df) == pytest.approx(want, abs=5e-4)


def test_c_leaf_is_b3s():
    assert run.c_leaf(E_LEAF) == 7980


@pytest.mark.parametrize(
    "w,b,se,bound,floor",
    [(4, 19_961, 13_290, 33_160, 50_359), (1, 5_787, 4_558, 21_290, 26_619)],
)
def test_r3_passes_slope_and_resolution_with_b4s_numbers(w, b, se, bound, floor):
    fit, ok = verdicts(w, R3[w])
    assert (
        round(fit["b"]),
        round(fit["se"]),
        round(fit["bound"]),
        round(fit["floor"]),
    ) == (b, se, bound, floor)
    assert ok["slope"] and ok["resolution"]


@pytest.mark.parametrize("w", [4, 1])
def test_r3_plus_half_zone_statistics_fails_the_slope(w):
    """δ_z = 0.029 B/row, the regression criterion 2 exists for, fails at both W; the margin is r3's own."""
    assert run.DELTA_Z == round(0.029 * run.BLOCK_ROWS)
    fit, ok = verdicts(w, grown(R3[w], run.DELTA_Z))
    assert fit["b"] == pytest.approx(verdicts(w, R3[w])[0]["b"] + run.DELTA_Z)
    assert not ok["slope"]


def test_r3_w1_floor_fails_the_recheck_at_k_1():
    """B.5's re-check: r3 ran W = 1 once per size, and its floor (26,619 B/leaf) is above c_leaf: inconclusive, a
    fail. This is why W = 1 runs k₁ times."""
    _, ok = verdicts(1, R3[1])
    assert not ok["re-check"]


def at_r3_noise(k):
    """r3's W = 1 design repeated k times per size with r3's σ̂ held fixed (B.5's planning model): r3's four
    residuals, each repeated k times and scaled so that the residual sum of squares over N − 2 = 4k − 2 is r3's."""
    fit, _ = verdicts(1, R3[1])
    a = sum(y for _, y in R3[1]) / 4 - fit["b"] * fit["n_bar"]
    scale = ((4 * k - 2) / (2 * k)) ** 0.5
    return [
        (n, 60 * MIB + scale * (y - a - fit["b"] * n))
        for _ in range(k)
        for n, y in R3[1]
    ]


@pytest.mark.parametrize(
    "k,floor",
    [
        (1, 26_619),
        (2, 12_526),
        (3, 9_539),
        (4, 8_028),
        (5, 7_070),
        (6, 6_391),
        (7, 5_877),
    ],
)
def test_w1_floor_at_r3_noise_is_b5s_table(k, floor):
    """B.5's table: the W = 1 detection floor at r3's σ̂ for k runs per size. The re-check (floor ≤ c_leaf = 7,980)
    fails up to k = 4 and passes from k = 5; run.py's k₁ is B.5's 6."""
    fit, ok = verdicts(1, at_r3_noise(k))
    assert fit["N"] == 4 * k
    assert fit["sigma"] == pytest.approx(1_995_436, abs=1)
    assert round(fit["floor"]) == floor
    assert ok["re-check"] is (floor <= run.c_leaf(E_LEAF))
    assert ok["slope"] and ok["resolution"]


def test_k1_is_b5s():
    assert run.K1 == 6
    assert [run.repeats(1, rows) for rows in run.ROWS] == [1, 1, 6, 6, 6, 6]
    assert [run.repeats(4, rows) for rows in run.ROWS] == [1, 1, 1, 3, 3, 3]


@pytest.mark.parametrize("w,threshold", [(4, 13_198), (1, 15_503)])
def test_r3_plus_growth_fails_just_above_b4s_threshold(w, threshold):
    """B.4: added to r3, a growth δ fails when δ > 13,198 B/leaf at W = 4 and 15,503 at W = 1 (the bound less b̂), and
    passes just below; so the slope check is exactly b̂ ≤ c_leaf + t · SE, not a looser one."""
    assert verdicts(w, grown(R3[w], threshold - 1))[1]["slope"]
    assert not verdicts(w, grown(R3[w], threshold + 1))[1]["slope"]


def test_b4s_live_heap_row_passes_at_k_3():
    """B.4's stated deviation: the pre-#759 live heap per leaf, 12.8 KB, added to r3, passes at both W."""
    assert verdicts(4, grown(R3[4], 12_800))[1]["slope"]
    assert verdicts(1, grown(R3[1], 12_800))[1]["slope"]


def test_pre_759_writer_fails_on_its_own_data():
    """The fail-before (B.4): r1's W = 1 sweep, the pre-#759 writer, at 24,265 B/leaf against a bound of 10,652."""
    fit, ok = verdicts(1, R1_W1)
    assert (round(fit["b"]), round(fit["se"]), round(fit["bound"])) == (
        24_265,
        915,
        10_652,
    )
    assert not ok["slope"]


def test_noise_too_large_to_resolve_delta_z_is_inconclusive():
    noisy = [(n, y + (40 * MIB if i % 2 else 0)) for i, (n, y) in enumerate(R3[4])]
    _, ok = verdicts(4, noisy)
    assert not ok["resolution"]


def test_too_few_runs_fail():
    _, checks = run.slope_test(4, R3[4][:2], E_LEAF)
    assert [ok for _, ok, _ in checks] == [False]
    _, checks = run.slope_test(4, [(64, y) for _, y in R3[4]], E_LEAF)
    assert [ok for _, ok, _ in checks] == [False]


def tenth_runs(series):
    """Run records holding only what the within-run fit reads: rows, rep and the ten tenths' peaks."""
    return [
        {"rows": rows, "rep": rep, "tenths": tenths}
        for rep, (rows, tenths) in enumerate(series)
    ]


def within(w, series, e=E_LEAF):
    fit, checks = run.within_run_test(w, tenth_runs(series), e)
    return fit, {name.split(": ")[-1]: ok for name, ok, _ in checks}


def series(w, rows, level, per_leaf=0, jitter=0):
    """A run's ten tenths: `level`, plus `per_leaf` bytes per leaf through the writer, plus a deterministic ±`jitter`
    that doesn't depend on the leaves."""
    return (
        rows,
        [
            level + per_leaf * run.tenth_leaves(rows, t) + jitter * (-1) ** t
            for t in range(10)
        ],
    )


@pytest.mark.parametrize(
    "rows,want",
    [
        (67_108_864, [1, 3, 4, 6, 8, 9, 11, 12, 14, 16]),
        (2_400_000_000, [57, 114, 171, 228, 286, 343, 400, 457, 514, 573]),
    ],
)
def test_tenth_leaves_are_the_leaves_complete_at_each_tenths_end(rows, want):
    """C.2's x_rt: ⌊(t + 1) · rows / (10 · BLOCK_ROWS)⌋, and n in the last tenth (the seal commits every leaf)."""
    assert [run.tenth_leaves(rows, t) for t in range(10)] == want


SIZES = {
    4: (1_073_741_824, 2_400_000_000),
    1: (268_435_456, 1_073_741_824, 2_400_000_000),
}


def design(w, per_leaf=0, jitter=MIB // 4, level=lambda i: 60 * MIB):
    """The job design's fitted runs at W (k per size), each at its own level `level(i)`."""
    k = {4: run.REPEATS, 1: run.K1}[w]
    return [
        series(w, rows, level(i), per_leaf, jitter)
        for i, rows in enumerate(r for r in SIZES[w] for _ in range(k))
    ]


@pytest.mark.parametrize("w", [4, 1])
def test_flat_runs_at_different_levels_pass_within_and_fail_across(w):
    """C.1: per-run offsets that rise with n (the disk's load across runs) and no growth within a run. The within-run
    slope passes; the across-run slope (B.2, now context) on the same runs' peaks reads the offsets as growth."""
    runs = design(w, level=lambda i: 180 * MIB + i * 3 * MIB)
    fit, ok = within(w, runs)
    assert abs(fit["b"]) < run.c_leaf(E_LEAF) / 10 and all(ok.values())
    peaks = [(run.blocks(rows), max(t)) for rows, t in runs]
    assert not verdicts(w, peaks)[1]["slope"]


@pytest.mark.parametrize("w", [4, 1])
def test_growth_within_runs_fails(w):
    """A per-leaf growth of c_leaf beyond c_leaf (B.5's target), the same in every run, fails the within-run slope at
    both W, whatever the runs' levels."""
    level = lambda i: (60 + 7 * (i % 3)) * MIB  # noqa: E731
    flat, _ = within(w, design(w, level=level))
    fit, ok = within(w, design(w, per_leaf=2 * run.c_leaf(E_LEAF), level=level))
    # The fit is linear in y, so a growth common to every run moves b̂ by exactly that growth (as B.4 argues).
    assert fit["b"] == pytest.approx(flat["b"] + 2 * run.c_leaf(E_LEAF))
    assert fit["se"] == pytest.approx(flat["se"])
    assert not ok["slope"]


def test_g_and_n_are_c2s_counts_and_t_has_g_minus_1_df():
    """C.2: at W = 4 a 64-leaf run has one tenth at x ≥ 64 and is left out (G = 6: 256 leaves with 8 tenths, 573 with
    9); at W = 1 a 16-leaf run is left out (G = 18)."""
    runs4 = [series(4, 268_435_456, 60 * MIB)] * 3 + design(4)
    fit, _ = within(4, runs4)
    assert (fit["G"], fit["N"], fit["df"]) == (6, 3 * 8 + 3 * 9, 5)
    assert fit["t"] == pytest.approx(2.015, abs=5e-4)
    runs1 = [series(1, 67_108_864, 60 * MIB)] * run.K1 + design(1)
    fit, _ = within(1, runs1)
    assert (fit["G"], fit["N"], fit["df"]) == (18, 6 * 8 + 12 * 10, 17)


def test_the_standard_error_is_cr1_clustered_by_run():
    """Two runs of three tenths, worked by hand: the within-run deviations and their residuals, the per-run score
    Σ x̃ e, and CR1's factor G/(G − 1) · (N − 1)/(N − 2)."""
    pts = [
        [(64, 10.0), (128, 30.0), (192, 20.0)],
        [(64, 0.0), (128, 10.0), (192, 50.0)],
    ]
    # x̃ = (−64, 0, 64) in both; ỹ = (−10, 10, 0) and (−20, −10, 30); Sₓₓ = 4 · 64²; b̂ = (640 + 3200) / 16384.
    b = 3840 / 16384
    scores = [
        -64 * (-10 + 64 * b) + 64 * (0 - 64 * b),
        -64 * (-20 + 64 * b) + 64 * (30 - 64 * b),
    ]
    se = (2 / 1 * 5 / 4 * sum(u * u for u in scores)) ** 0.5 / 16384
    got_b, got_se, big_n, sxx = run.cluster_fit(pts)
    assert (big_n, sxx) == (6, 16384)
    assert got_b == pytest.approx(b) and got_se == pytest.approx(se)


def test_noise_within_runs_too_large_to_resolve_delta_z_is_inconclusive():
    """Resolution at W = 4: tenths with 40 MiB of independent noise put the floor above δ_z; the fit is inconclusive
    and fails, whatever b̂ is."""
    rng = random.Random(4)
    runs = [
        (rows, [300 * MIB + rng.gauss(0, 40 * MIB) for _ in range(10)])
        for rows, _ in design(4)
    ]
    fit, ok = within(4, runs)
    assert fit["floor"] > run.DELTA_Z and not ok["resolution"]


def test_w1_floor_above_c_leaf_fails_the_recheck_but_resolves_delta_z():
    """Resolution at W = 1 (B.5's re-check, unchanged by C.3): noise that leaves the floor between c_leaf and δ_z
    resolves the zone-statistics class but not a growth of c_leaf beyond c_leaf, and fails as inconclusive."""
    rng = random.Random(1)
    runs = [
        (rows, [60 * MIB + rng.gauss(0, 8 * MIB) for _ in range(10)])
        for rows, _ in design(1)
    ]
    fit, ok = within(1, runs)
    assert run.c_leaf(E_LEAF) < fit["floor"] <= run.DELTA_Z
    assert ok["resolution"] and not ok["re-check"]


def test_fewer_than_two_fitted_runs_or_missing_tenths_fail():
    _, checks = run.within_run_test(4, tenth_runs(design(4)[:1]), E_LEAF)
    assert [ok for _, ok, _ in checks] == [False]
    gap = design(4)
    gap[2] = (gap[2][0], gap[2][1][:5] + [0] + gap[2][1][6:])
    _, checks = run.within_run_test(4, tenth_runs(gap), E_LEAF)
    assert [ok for _, ok, _ in checks] == [False] and "no sample" in checks[0][2]
    short = design(4)
    short[0] = (short[0][0], short[0][1][:9])
    _, checks = run.within_run_test(4, tenth_runs(short), E_LEAF)
    assert [ok for _, ok, _ in checks] == [False] and "9 tenths" in checks[0][2]


def test_pre_759_writer_fails_the_within_run_slope():
    """C.4's fail-before: tessera-715-r3-failbefore-2, the pre-#759 writer at W = 1, 64 and 573 leaves. b̂ 17,219
    B/leaf against a bound of 9,237, resolved (floor 2,513 ≤ c_leaf)."""
    fit, ok = within(1, list(R3_FAILBEFORE.items()), e=2966)
    assert (fit["G"], fit["N"]) == (2, 18)
    assert (
        round(fit["b"]),
        round(fit["se"]),
        round(fit["bound"]),
        round(fit["floor"]),
    ) == (
        17_219,
        199,
        9_237,
        2_513,
    )
    assert not ok["slope"] and ok["resolution"] and ok["re-check"]


R1_LOGS = Path(__file__).parent / "fixtures" / "r1-logs"


def r1_runs(w):
    """r1's fitted runs at W, from the committed extracts of its logs (C.4)."""
    out = []
    for path in sorted(R1_LOGS.glob(f"w{w}-r*.log")):
        rows = int(run.LOG_NAME.match(path.name)[2])
        out.append(
            {"rows": rows, "rep": 0, "tenths": run.log_tenths(path.read_text(), rows)}
        )
    return out


@pytest.mark.parametrize(
    "w,g,big_n,b,se,bound,floor,fails",
    [
        (1, 3, 28, 19_761, 2_414, 15_028, 14_096, {"slope", "re-check"}),
        (4, 2, 17, 73_901, 3_292, 28_763, 41_566, {"slope"}),
    ],
)
def test_r1_pre_759_writer_fails_the_within_run_slope_from_its_logs(
    w, g, big_n, b, se, bound, floor, fails
):
    """C.4's fail-before on r1 itself (tessera-715-sweep-1, a3df683): the tenths rebuilt from its logs' own ~1 s
    sample lines (its 10 ms CSVs are over `ksb out`'s 1 MiB) fail the slope at both W; at W = 1 the floor is also
    above c_leaf (inconclusive), which fails too."""
    fit, checks = run.within_run_test(w, r1_runs(w), run.E_LEAF_LOGS)
    got = (
        fit["G"],
        fit["N"],
        round(fit["b"]),
        round(fit["se"]),
        round(fit["bound"]),
        round(fit["floor"]),
    )
    assert got == (g, big_n, b, se, bound, floor)
    assert {name.split(": ")[-1] for name, ok, _ in checks if not ok} == fails


def test_r1_extracts_carry_their_provenance():
    """Each extract names its source log and that log's sha256, so it can be checked against `ksb out`."""
    names = sorted(p.name for p in R1_LOGS.glob("*.log"))
    assert names == [
        "w1-r1073741824.log",
        "w1-r2400000000.log",
        "w1-r268435456.log",
        "w4-r1073741824.log",
        "w4-r2400000000.log",
    ]
    for path in R1_LOGS.glob("*.log"):
        head = path.read_text().splitlines()[0]
        assert head.startswith(f"# extract of tessera-715-sweep-1 {path.name}: sha256 ")
        assert len(head.split("sha256 ")[1].split(",")[0]) == 64


def test_log_extract_keeps_the_logs_tenths():
    """`log_extract` keeps one line per tenth, and `log_tenths` reads the same tenths from it as from the log; lines
    that aren't samples, and samples after the input ends, are handled as `peaks` handles the CSV's."""
    rng = random.Random(7)
    total = 64 * run.BLOCK_ROWS
    lines = ["reorder=false rows=… peak_RssAnon=1"]
    for i in range(400):
        lines.append(
            f"sample rows={min(total, i * total // 380)} RssAnon={60 * MIB + rng.randrange(MIB)} VmHWM=1"
        )
    text = "\n".join(lines) + "\n"
    extract = run.log_extract(text, total, "job w1-r268435456.log")
    assert run.log_tenths(extract, total) == run.log_tenths(text, total)
    assert sum(1 for line in extract.splitlines() if line.startswith("sample")) == 10
    assert hashlib.sha256(text.encode()).hexdigest() in extract.splitlines()[0]
    with pytest.raises(SystemExit, match="no `sample"):
        run.log_tenths("no samples here\n", total)


def test_log_tenths_cli_fails_on_r1(capsys):
    assert run.evaluate_logs(sorted(str(p) for p in R1_LOGS.glob("*.log"))) == 1
    assert "VERDICT: FAIL 2 plateau W=1: slope" in capsys.readouterr().out


def records(tmp_path, name, runs, job="proof"):
    d = tmp_path / name
    d.mkdir()
    (d / "records.json").write_text(json.dumps({"job": job, "runs": runs}))
    return d


def test_shards_merge_only_from_one_binary_and_without_duplicates(tmp_path):
    run_ = {
        "workers": 1,
        "rows": 0,
        "rep": 0,
        "binary": "/nix/store/a/bin/stream_memory",
    }
    a = records(tmp_path, "a", [run_])
    b = records(tmp_path, "b", [run_ | {"rows": 1}])
    assert len(run.load([a, b])[1]) == 2
    with pytest.raises(SystemExit, match="more than once"):
        run.load([a, records(tmp_path, "dup", [run_])])
    with pytest.raises(SystemExit, match="binaries"):
        run.load(
            [
                a,
                records(
                    tmp_path,
                    "other",
                    [run_ | {"rows": 1, "binary": "/nix/store/b/bin/x"}],
                ),
            ]
        )
    with pytest.raises(SystemExit, match="jobs"):
        run.load([a, records(tmp_path, "par", [run_ | {"rows": 1}], job="parallel")])


def full_records(grow=lambda w, n, rep: 0):
    """A complete, passing record set of the `job` design (every point, every repeat), shaped like run_once's:
    W = 1 at r3's level with ±0.25 MiB per repeat, W = 4 at r3's nine runs' levels; each run flat over its tenths
    with ±64 KiB of jitter. `grow(w, n, rep)` adds that many bytes per leaf through the writer to a run's tenths."""
    w4 = {n: [y for m, y in R3[4] if m == n] for n in (64, 256, 573)}
    runs = []
    for w in run.WORKERS:
        for rows in run.ROWS:
            n = run.blocks(rows)
            for rep in range(run.repeats(w, rows)):
                if rows == 0:
                    tenths = [2 * MIB] + [0] * 9
                elif n == 1:
                    tenths = [3 * MIB] * 9 + [56 * MIB]
                else:
                    y = (
                        60 * MIB + (rep % 2 - 0.5) * MIB / 2
                        if w == 1
                        else w4.get(n, [186 * MIB])[rep % 3]
                    )
                    tenths = [
                        y
                        + (64 << 10) * ((t + rep) % 3 - 1)
                        + grow(w, n, rep) * run.tenth_leaves(rows, t)
                        for t in range(10)
                    ]
                y = max(tenths)
                runs.append(
                    {
                        "workers": w,
                        "rows": rows,
                        "rep": rep,
                        "exit": 0,
                        "wall_s": 1.0,
                        "util": None,
                        "memory_max": str(4 << 30),
                        "session_fs": "0xef53",
                        "output_fs": "0xef53",
                        "cpus": 4,
                        "binary": "/nix/store/a/bin/stream_memory",
                        "cgroup_before": {
                            "anon": 20 * MIB,
                            "procs": [[1, "python3", 20 * MIB]],
                        },
                        "cgroup_after": {"anon": 20 * MIB, "procs": []},
                        "leaves": n,
                        "e_leaf": E_LEAF,
                        "blocks": n + 2,
                        "content_hash": f"blake3:{rows}",
                        "canonical": {"blocks": f"{rows}", "schema": "s"},
                        "rss_anon": y,
                        "tenths": tenths,
                        "peak_row_group": 1,
                        "memory_current": y + 100 * MIB,
                        "anon": y + 20 * MIB,
                        "file": 50 * MIB,
                        "shmem": 0,
                    }
                )
    return runs


def evaluate(runs, capsys):
    code = run.evaluate_proof(run.Verdicts(runs, want_tmpfs=False))
    out = capsys.readouterr().out
    return code, [line for line in out.splitlines() if line.startswith("FAIL")], out


def test_complete_records_pass_every_criterion(capsys):
    code, fails, out = evaluate(full_records(), capsys)
    assert (code, fails) == (0, [])
    assert "VERDICT: PASS, criteria 1-5" in out
    # C.3: the across-run slope is printed as context at both W, never as a verdict.
    assert [
        line.split(" (")[0] for line in out.splitlines() if "across-run" in line
    ] == [
        "CONTEXT 2 across-run W=1",
        "CONTEXT 2 across-run W=4",
    ]
    assert "within-run (fixed effects, C.2)" in out
    # Criterion 5 compares every run at 268 M rows, the 6 W = 1 and 3 W = 4 repeats, with W = 1's first.
    assert out.count("PASS 5 bytes and names") == run.K1 - 1 + run.REPEATS


def test_a_missing_repeat_fails(capsys):
    runs = [
        r
        for r in full_records()
        if (r["workers"], r["rows"], r["rep"]) != (1, run.MID, 5)
    ]
    code, fails, _ = evaluate(runs, capsys)
    assert code == 1
    assert any(f.startswith("FAIL 2 plateau W=1:") and "needs 6" in f for f in fails)
    assert any(f.startswith("FAIL 5 bytes and names:") for f in fails)


def test_a_differing_repeat_fails_criterion_5(capsys):
    runs = full_records()
    next(r for r in runs if (r["workers"], r["rows"], r["rep"]) == (1, run.MID, 3))[
        "content_hash"
    ] = "blake3:x"
    _, fails, _ = evaluate(runs, capsys)
    assert [f.split(":")[0] for f in fails] == [
        f"FAIL 5 bytes and names rows={run.MID} W=1 run 4 vs W=1 run 1"
    ]


def test_growth_of_c_leaf_beyond_c_leaf_fails_the_slope_end_to_end(capsys):
    """B.5's target, a W = 1 slope of 2 c_leaf within every run: 4.6 MB over the sweep, under every run's ceiling, so
    only criterion 2 sees it."""
    runs = full_records(lambda w, n, rep: 2 * run.c_leaf(E_LEAF) if w == 1 else 0)
    _, fails, _ = evaluate(runs, capsys)
    assert [f.split(":")[0] + ":" + f.split(":")[1] for f in fails] == [
        "FAIL 2 plateau W=1: slope"
    ]


def test_c_leaf_uses_the_largest_e_leaf_of_the_fitted_runs(capsys):
    runs = full_records()
    next(r for r in runs if (r["workers"], r["rows"], r["rep"]) == (4, run.MID, 1))[
        "e_leaf"
    ] = E_LEAF + 10
    _, fails, out = evaluate(runs, capsys)
    assert fails == []
    assert (
        f"c_leaf {run.c_leaf(E_LEAF + 10):,} B (e_leaf {E_LEAF + 10} B)"
        in out.split("2 plateau W=4: slope")[1]
    )


def test_the_default_plan_is_the_full_design_and_spelled_out_is_too():
    assert run.job_plan([1, 4], list(run.ROWS)) == run.job_plan()
    assert len(run.job_plan()) == 2 + 4 * run.K1 + 3 + 3 * run.REPEATS
    assert run.job_plan(reps=[0, 1, 2, 3, 4, 5]) == run.job_plan()
    assert run.job_plan([1], reps=[4]) != run.job_plan()


def test_a_shard_checks_its_own_runs(tmp_path, capsys):
    plan = [(1, rows, 4) for rows in run.ROWS if run.repeats(1, rows) > 4]
    shard = [r for r in full_records() if (r["workers"], r["rows"], r["rep"]) in plan]
    d = records(tmp_path, "k4", shard)
    assert run.check_shard(d, plan) == 0
    d = records(tmp_path, "k4-short", shard[:-1])
    assert run.check_shard(d, plan) == 1
    assert "needs 1 completed run(s), has 0" in capsys.readouterr().out
