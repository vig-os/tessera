"""Contract tests for every ecosystem adapter (#485).

These run on TINY data — they check the contract and bit-exactness, not performance. The point is
that a mislabelled or asymmetric adapter is caught here rather than in a published table, which is
how the three fairness omissions (#487 HDF5 shuffle, #503 Parquet BYTE_STREAM_SPLIT, and this
issue's ROOT TTree-vs-RNTuple mislabel) each got out in the first place.

Run: `uv run python -m pytest test_adapters.py -q`
"""

from __future__ import annotations

import importlib

import numpy as np
import pytest

import common

ADAPTERS = ["tessera", "hdf5", "zarr_", "nexus", "nifti", "dicom", "parquet", "root"]

# Small enough to be fast, large enough that chunked layouts are exercised.
_VOL = (np.arange(8 * 32 * 32, dtype=np.int64).reshape(8, 32, 32) % 900 - 400).astype(
    "<i2"
)
_COLS = {
    "t": np.arange(4096, dtype="<u8"),
    "e0": (511.0 + (np.arange(4096) % 7)).astype("<f4"),
    "e1": (510.0 - (np.arange(4096) % 5)).astype("<f4"),
}


# `tessera` is the subject of the comparison, not one of the ecosystems it is compared against.
# If it is missing the whole run is meaningless, so it must FAIL rather than skip: a skipped
# reference adapter still reports the suite green, and "not a failure" silently standing in for
# "passed" is the exact shape that lets an unmeasured thing look measured.
_REFERENCE = "tessera"


def _mod(name):
    try:
        return importlib.import_module(f"adapters.{name}")
    except Exception as e:  # noqa: BLE001
        if name == _REFERENCE:
            pytest.fail(
                f"adapters.{name} not importable: {e}\n"
                "This is the reference adapter — build it first:\n"
                "  cargo build -p tessera-py --release, then assemble the package: see README.md "
                "'Run' (the cdylib is lib_native.so -> tessera/_native.so, not libtessera.so)"
            )
        pytest.skip(f"adapters.{name} not importable: {e}")


@pytest.mark.parametrize("name", ADAPTERS)
def test_adapter_declares_at_least_default_and_tuned(name):
    """The #485 contract. A single hard-coded codec cannot express 'default AND standard tuning',
    which is exactly how #487's missing HDF5 shuffle and #503's missing BYTE_STREAM_SPLIT survived.
    """
    mod = _mod(name)
    assert hasattr(mod, "VARIANTS"), f"{name}: no VARIANTS"
    assert "default" in mod.VARIANTS, f"{name}: no 'default' variant"
    if len(mod.VARIANTS) < 2:
        # A single-variant adapter is allowed ONLY when the library genuinely exposes no tuning
        # lever, and only if it says so in a string the report prints. #487's rule is "no invented
        # knobs" -- fabricating a second row to satisfy a symmetry requirement is the same dishonesty
        # as omitting a real one, so the exception is declared and reviewable rather than implicit.
        assert getattr(mod, "SINGLE_VARIANT_REASON", "").strip(), (
            f"{name}: only {list(mod.VARIANTS)} and no SINGLE_VARIANT_REASON — every format must be "
            "shown at a sensible default AND its standard tuning, or state why it cannot be"
        )


@pytest.mark.parametrize("name", ADAPTERS)
def test_every_variant_has_a_nonempty_printed_settings_string(name):
    """Settings are printed on every row, and the string must say what was ACTUALLY used —
    #487 shipped a draft whose settings line claimed a chunk geometry the code did not use."""
    mod = _mod(name)
    for key in mod.VARIANTS:
        for modality in ("volume", "table"):
            if not mod.CAPS.get(modality):
                continue
            desc = common.settings_for(mod, key, modality)
            assert isinstance(desc, str) and desc.strip(), (
                f"{name}/{key}/{modality}: empty settings string"
            )


@pytest.mark.parametrize("name", ADAPTERS)
def test_settings_string_does_not_claim_a_chunk_geometry_the_other_modality_uses(name):
    """#487's exact defect: a draft printed "chunked 64^3 (= tessera's)" on the TABLE rows while
    the code chunked 1-D at 65536. If an adapter supports both modalities and its layout differs,
    the two settings strings must differ too -- one string covering both is the bug."""
    mod = _mod(name)
    if not (mod.CAPS.get("volume") and mod.CAPS.get("table")):
        pytest.skip(f"{name}: single-modality, nothing to conflate")
    for key in mod.VARIANTS:
        vol_s = common.settings_for(mod, key, "volume")
        tab_s = common.settings_for(mod, key, "table")
        if "^3" in vol_s:
            assert "^3" not in tab_s, (
                f"{name}/{key}: table settings claim a cubic chunk geometry ({tab_s!r}) — "
                "tables are chunked 1-D"
            )


@pytest.mark.parametrize("name", ADAPTERS)
def test_volume_roundtrips_bit_exact_in_every_variant(name, tmp_path):
    mod = _mod(name)
    if not mod.CAPS.get("volume"):
        pytest.skip(f"{name}: no volume modality")
    for variant in mod.VARIANTS:
        base = str(tmp_path / f"{name}_{variant}_vol")
        mod.write_volume(base, _VOL, variant)
        back = mod.read_volume(base, variant)
        assert back.shape == _VOL.shape, f"{name}/{variant}: shape"
        assert np.array_equal(back, _VOL), f"{name}/{variant}: volume not bit-exact"
        z = _VOL.shape[0] // 2
        assert np.array_equal(mod.read_volume_zslice(base, z, variant), _VOL[z]), (
            f"{name}/{variant}: zslice mismatch"
        )


@pytest.mark.parametrize("name", ADAPTERS)
@pytest.mark.parametrize("fixture", common.FIXTURES)
def test_table_roundtrips_bit_exact_in_every_variant(name, fixture, tmp_path):
    """Both fixtures, every variant. #497: same encoder, same command, opposite verdicts."""
    mod = _mod(name)
    if not mod.CAPS.get("table"):
        pytest.skip(f"{name}: no table modality")
    cols = {k: v[:4096] for k, v in common.make_table(fixture).items()}
    for variant in mod.VARIANTS:
        base = str(tmp_path / f"{name}_{variant}_{fixture}_tab")
        mod.write_table(base, cols, variant)
        back = mod.read_table(base, variant)
        for k, v in cols.items():
            assert k in back, f"{name}/{variant}/{fixture}: missing column {k}"
            assert np.array_equal(back[k], v), (
                f"{name}/{variant}/{fixture}: column {k} not bit-exact"
            )
        one = mod.read_table_column(base, "e0", variant)
        assert np.array_equal(one, cols["e0"]), (
            f"{name}/{variant}/{fixture}: projected column mismatch"
        )


@pytest.mark.parametrize("name", ADAPTERS)
def test_name_does_not_claim_a_container_the_writer_does_not_produce(name):
    """ROOT shipped as 'uproot/TTree' while uproot 5.7 wrote a ROOT::RNTuple for a dict assignment —
    a different ROOT format with different performance, under the classic format's name. Same class
    as #503's false 'default' label. Any adapter naming a specific container must produce it."""
    mod = _mod(name)
    if name != "root":
        pytest.skip("container-name claim is ROOT-specific")
    import uproot

    for variant, desc in mod.VARIANTS.items():
        claimed = (
            "TTree" if "TTree" in desc else "RNTuple" if "RNTuple" in desc else None
        )
        assert claimed, f"root/{variant}: settings string names no container: {desc!r}"
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            base = f"{d}/probe"
            mod.write_table(base, _COLS, variant)
            with uproot.open(mod.path_for(base, "table")) as f:
                actual = f["events"].classname
        assert claimed in actual, (
            f"root/{variant}: settings say {claimed!r} but the file contains {actual!r}"
        )


def _assert_materialised(arr, where):
    assert not isinstance(arr, np.memmap), f"{where}: returned a np.memmap"
    assert not isinstance(getattr(arr, "base", None), np.memmap), (
        f"{where}: returned a view backed by a np.memmap — the bytes were never read, so its "
        "timing is not comparable with the other adapters"
    )


@pytest.mark.parametrize("name", ADAPTERS)
def test_reads_materialise_rather_than_returning_a_lazy_view(name, tmp_path):
    """No format may win by returning something cheaper than the others — on ANY read path.

    nibabel's ArrayProxy over an uncompressed `.nii` is MEMORY-MAPPED, so `np.asarray` returned a
    view over a `np.memmap`: no bytes read, nothing faulted until something later touched the
    pages. It timed at 96,407 MB/s warm and 63,342 MB/s cold, which are not measurements. The same
    guard now covers the z-slice, full-table and projected-column reads for every adapter and
    variant, since a lazy view on any timed path is the same defect.
    """
    mod = _mod(name)
    for variant in mod.VARIANTS:
        if mod.CAPS.get("volume"):
            base = str(tmp_path / f"{name}_{variant}_lazy_vol")
            mod.write_volume(base, _VOL, variant)
            _assert_materialised(
                mod.read_volume(base, variant), f"{name}/{variant}/read_volume"
            )
            _assert_materialised(
                mod.read_volume_zslice(base, _VOL.shape[0] // 2, variant),
                f"{name}/{variant}/read_volume_zslice",
            )
        if mod.CAPS.get("table"):
            base = str(tmp_path / f"{name}_{variant}_lazy_tab")
            mod.write_table(base, _COLS, variant)
            for col, arr in mod.read_table(base, variant).items():
                _assert_materialised(arr, f"{name}/{variant}/read_table[{col}]")
            _assert_materialised(
                mod.read_table_column(base, "e0", variant),
                f"{name}/{variant}/read_table_column",
            )


def _flip_payload_byte(path):
    """Flip one byte in the middle of a file (or of the largest file in a store directory)."""
    import os

    if os.path.isdir(path):
        files = [os.path.join(r, f) for r, _d, fs in os.walk(path) for f in fs]
        path = max(files, key=os.path.getsize)
    with open(path, "r+b") as f:
        f.seek(0, 2)
        n = f.tell()
        f.seek(n // 2)
        b = f.read(1)
        f.seek(n // 2)
        f.write(bytes([b[0] ^ 0xFF]))


@pytest.mark.parametrize("name", ADAPTERS)
def test_integrity_mechanism_actually_detects_a_flipped_byte(name, tmp_path):
    """Every mechanism the integrity table TIMES must actually DETECT corruption.

    Timing a checksum that never fires would be the NIfTI-memmap defect again: a row that looks
    like a measurement of work that is not being done. For each adapter declaring INTEGRITY, flip
    one byte in the middle of the written file and require the checked path to raise.

    What this proves, precisely — observed errors on a mid-file flip:

    - Parquet: "CRC checksum verification failed for page_ordinal 0" — the page CRC itself fired.
    - tessera: "io: Invalid checksum" — the STORED zip container's per-entry CRC32 fired first, before
      the blake3 re-derivation `verify` would reach; corruption is caught either way.
    - HDF5/NeXus: "filter returned failure during read" — the gzip OR the fletcher32 filter; the error
      does not say which, because both sit in the same filter pipeline.

    So it asserts the CHECKED PATH raises on corruption, which is what the integrity rows time. It does
    not claim the named mechanism is always the one that fires, because for three of the four it is
    not observable which one did.
    """
    mod = _mod(name)
    ig = getattr(mod, "INTEGRITY", None)
    if not ig:
        pytest.skip(f"{name}: declares no integrity mechanism")
    variant = ig["variant"]
    rng = np.random.default_rng(3)
    # incompressible payload so the middle of the file is data, not a header
    noisy = rng.integers(-2000, 2000, size=(8, 64, 64)).astype("<i2")
    cols = {
        "t": np.arange(20000, dtype="<u8"),
        "e0": rng.random(20000).astype("<f4"),
        "e1": rng.random(20000).astype("<f4"),
    }
    if mod.CAPS.get("volume"):
        base, modality = str(tmp_path / f"{name}_ig_vol"), "volume"
        mod.write_volume(base, noisy, variant)
        read = lambda: mod.read_volume(base, variant)  # noqa: E731
    else:
        base, modality = str(tmp_path / f"{name}_ig_tab"), "table"
        mod.write_table(base, cols, variant)
        read = lambda: mod.read_table(base, variant)  # noqa: E731
    check = (
        (lambda: mod.verify(base, modality, variant))
        if hasattr(mod, "verify")
        else read
    )
    check()  # intact file must pass
    path = (
        mod.path_for(base, modality, variant)
        if "variant" in mod.path_for.__code__.co_varnames
        else mod.path_for(base, modality)
    )
    _flip_payload_byte(path)
    with pytest.raises(Exception):
        check()
