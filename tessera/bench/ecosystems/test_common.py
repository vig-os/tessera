"""Tests for the cross-ecosystem harness scaffold (#485).

These guard the three methodology changes backported from #487/#497/#503, and they exist because
each one has already been got wrong once in this repo:

- **median + spread** replacing min-of-N, because min hides variance and cannot show when two
  formats are within noise of each other;
- **cold-cache eviction that is MEASURED, not assumed** — #487's first implementation evicted
  nothing (cold matched warm to four decimals) because `POSIX_FADV_DONTNEED` silently skips dirty
  pages, so a cold-labelled row was really a warm one;
- **both table fixtures**, because #497 established that the periodic fixture is adversarial for
  value-distribution codecs and publishing one fixture chooses the answer.

Run: `uv run python -m pytest test_common.py -q`
"""

from __future__ import annotations

import os

import numpy as np
import pytest

import common


# --------------------------------------------------------------------------- stats
def test_stats_reports_median_not_min():
    """The headline number must be the median. min-of-N is what #485 replaces."""
    seq = iter([0.05, 0.01, 0.03])  # min 0.01, median 0.03
    s = common.stats(lambda: None, 3, _elapsed=lambda: next(seq, 0.0))
    assert s["median"] == pytest.approx(0.03)
    assert s["min"] == pytest.approx(0.01)
    assert s["max"] == pytest.approx(0.05)
    assert s["n"] == 3


def test_stats_keeps_n_and_spread_so_noise_is_visible():
    """A format within noise of another must be visibly so: n and [min..max] are not optional."""
    s = common.stats(lambda: None, 5)
    assert s["n"] == 5
    assert s["min"] <= s["median"] <= s["max"]
    assert set(s) >= {"median", "min", "max", "n"}


def test_stats_rejects_zero_iters_rather_than_returning_a_vacuous_number():
    """An empty sample has no median; returning 0.0 or inf would be a number with no measurement
    behind it. Refuse instead -- every predicate over an empty collection is true of nothing."""
    with pytest.raises(ValueError):
        common.stats(lambda: None, 0)


# --------------------------------------------------------------------------- cold cache
def test_evict_actually_evicts_a_freshly_written_file(tmp_path):
    """#487's exact bug: DONTNEED skips DIRTY pages, so evicting a just-written file without an
    fsync first evicts nothing and the 'cold' row is served from RAM."""
    p = tmp_path / "hot.bin"
    p.write_bytes(os.urandom(8 << 20))  # 8 MiB, certainly in page cache and dirty
    before = common.residency(str(p))
    assert before > 0.5, f"fixture not actually cached (residency {before:.1%})"
    worst = common.evict(str(p))
    after = common.residency(str(p))
    assert after < 0.05, f"eviction ineffective: {after:.1%} still resident"
    assert worst == pytest.approx(after, abs=0.02)


def test_evict_walks_directory_trees(tmp_path):
    """Zarr stores and DICOM series are DIRECTORIES. Evicting one fd would leave the store warm
    and label the row cold -- worse than no row."""
    d = tmp_path / "store.zarr"
    (d / "c" / "0").mkdir(parents=True)
    for i in range(4):
        (d / "c" / "0" / f"chunk{i}").write_bytes(os.urandom(2 << 20))
    assert common.residency(str(d)) > 0.5
    common.evict(str(d))
    assert common.residency(str(d)) < 0.05


def test_residency_of_missing_path_is_not_silently_zero(tmp_path):
    """0.0 residency reads as 'perfectly evicted'. A missing file must not be able to claim it."""
    with pytest.raises((FileNotFoundError, OSError)):
        common.residency(str(tmp_path / "nope.bin"))


# --------------------------------------------------------------------------- fixtures
def test_both_table_fixtures_exist_and_differ():
    """#497: publishing one fixture without the other is choosing the answer."""
    per = common.make_table("periodic")
    con = common.make_table("continuous")
    assert set(per) == set(con) == {"t", "e0", "e1"}
    assert not np.array_equal(per["e0"], con["e0"])


def test_periodic_fixture_is_the_one_documented_as_adversarial():
    """Pins the property #497 identified: short exact periods, which deflate's LZ77 window exploits
    and value-distribution codecs cannot. If this stops holding the label is a lie."""
    per = common.make_table("periodic")
    assert len(np.unique(per["e0"])) == 7
    assert len(np.unique(per["e1"])) == 5


def test_continuous_clock_runs_are_poisson_not_a_fixed_stride():
    """Lifted from #497. An exact stride would smuggle the periodic fixture's adversarial property
    into the fixture that is supposed to be realistic; a real clock's run lengths vary."""
    con = common.make_table("continuous")
    t = con["t"]
    runs = np.diff(np.flatnonzero(np.diff(t)) if t.size else np.array([0]))
    distinct = len(np.unique(runs))
    assert distinct > 1, (
        f"run lengths look like a fixed stride: {distinct} distinct values"
    )


def test_continuous_floats_really_are_continuous():
    """The realistic fixture's e0/e1 must come from a continuous distribution.

    An earlier version asserted only `> 1000` distinct values -- an arbitrary threshold weak enough
    to PASS on a degenerate generator that produced 1143 distinct values in 1,000,000 draws (a
    dropped XOR in xorshift64*). A categorical column wearing the "continuous" label is exactly the
    #497 failure: the fixture that exists to be realistic quietly acquiring the adversarial property
    of the one it is contrasted with. Assert a large FRACTION of n, not a magic constant.
    """
    con = common.make_table("continuous")
    for col in ("e0", "e1"):
        frac = len(np.unique(con[col])) / con[col].size
        assert frac > 0.5, (
            f"{col}: only {frac:.2%} distinct — not a continuous distribution"
        )


def test_continuous_energy_columns_are_independent():
    """e1 was derived as `450 + 120*(1 - f)` from e0's own draw, making them perfectly
    anti-correlated (r = -1.0). The second column then carried no information the first did not,
    which no two-detector energy pair looks like and which flatters any format exploiting it."""
    con = common.make_table("continuous")
    r = float(np.corrcoef(con["e0"], con["e1"])[0, 1])
    assert abs(r) < 0.05, f"e0/e1 correlation {r:.6f} — the columns are not independent"


def test_make_table_rejects_an_unknown_fixture_name():
    """A typo must not silently yield one of the two and mislabel every row it produces."""
    with pytest.raises(ValueError):
        common.make_table("listmode-ish")


def test_fixtures_are_byte_reproducible():
    """Fixed-seed generation: two calls must be identical or no result is comparable to a rerun."""
    for kind in ("periodic", "continuous"):
        a, b = common.make_table(kind), common.make_table(kind)
        for k in a:
            assert np.array_equal(a[k], b[k]), f"{kind}/{k} not reproducible"
