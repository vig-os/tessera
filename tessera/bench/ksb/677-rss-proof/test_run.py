"""Self-test of run.py's criterion 2 (ADR-0064 §8.1 as amended by #770, B.2-B.5) on recorded ksb data.

The r3 numbers are `tessera-715-r3-sweep-2`'s records.json (PR #767 at 0b66160, the post-#759 writer), exact bytes of
peak RssAnon. The r1 numbers are `tessera-715-sweep-1`'s job log (tessera a3df683, the pre-#759 writer), at the log's
0.1 MiB precision: its records.json is over `ksb out`'s 1 MiB. ADR-0064 B.4 tabulates both; these tests pin run.py to
that table, so the evaluator and the ADR can't drift apart.
"""

import json
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


def full_records(rss=lambda w, n, rep: None):
    """A complete, passing record set of the `job` design (every point, every repeat), shaped like run_once's:
    W = 1 at r3's level with ±0.25 MiB per repeat, W = 4 cycling r3's nine runs. `rss` may override a run's peak."""
    w4 = {n: [y for m, y in R3[4] if m == n] for n in (64, 256, 573)}
    runs = []
    for w in run.WORKERS:
        for rows in run.ROWS:
            n = run.blocks(rows)
            for rep in range(run.repeats(w, rows)):
                if rows == 0:
                    y = 2 * MIB
                elif n == 1:
                    y = 56 * MIB
                elif w == 1:
                    y = 60 * MIB + (rep % 2 - 0.5) * MIB / 2
                else:
                    y = w4.get(n, [186 * MIB])[rep % 3]
                y = rss(w, n, rep) or y
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
    """B.5's target, a W = 1 slope of 2 c_leaf: 4.6 MB over the sweep, under every run's ceiling, so only criterion 2
    sees it."""
    runs = full_records(
        lambda w, n, rep: n >= 16 and w == 1 and 60 * MIB + 2 * run.c_leaf(E_LEAF) * n
    )
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
