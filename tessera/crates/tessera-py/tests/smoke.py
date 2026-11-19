"""Hermetic smoke test for the `tessera` Python PACKAGE (#210 + the ergonomic layer).

Run as: python3 smoke.py <corpus_files_dir>

Exercises the ERGONOMIC surface over the committed conformance corpus — numpy ``ndarray`` for arrays
(+ ROI sub-cube), polars ``DataFrame`` / pyarrow ``Table`` / dict-of-arrays + projected column for
tables — plus the typed-error path and a numpy write→read roundtrip. Driven by the `tessera-py-import`
flake check (the assembled `tessera/` package — pure-Python `__init__.py` + `_native.so` — on PYTHONPATH).
"""

import io
import json
import math
import pathlib
import struct
import sys
import tempfile
import zipfile

import numpy as np
import polars as pl
import pyarrow as pa

import tessera


def jcs(value) -> bytes:
    """RFC 8785 JCS bytes of a parsed JSON value: a stdlib-only oracle for the stored-form checks below
    (the check environment has no JCS library). Keys sort by UTF-16 code unit; strings use `json`'s
    escapes, which are RFC 8785's (`\\b \\f \\n \\r \\t`, other controls as lowercase `\\u00xx`,
    nothing else escaped). A number is written in ECMAScript form only where `repr` already is
    (an integral value below 1e21, or a fraction with |x| >= 1e-4, which `repr` writes as plain
    decimals); anything else is refused rather than guessed at."""

    def num(x) -> str:
        if isinstance(x, bool):
            return "true" if x else "false"
        if isinstance(x, int):
            return str(x)
        assert math.isfinite(x), x
        if x == int(x) and abs(x) < 1e21:
            return str(int(x))
        assert abs(x) >= 1e-4, f"jcs oracle: no ECMAScript form for {x!r} here"
        return repr(x)

    def enc(v) -> str:
        if v is None or isinstance(v, (bool, int, float)):
            return "null" if v is None else num(v)
        if isinstance(v, str):
            return json.dumps(v, ensure_ascii=False)
        if isinstance(v, list):
            return "[" + ",".join(enc(x) for x in v) + "]"
        keys = sorted(v, key=lambda k: k.encode("utf-16-be"))
        return "{" + ",".join(enc(k) + ":" + enc(v[k]) for k in keys) + "}"

    return enc(value).encode("utf-8")


def assert_stored_form(tsra, what: str) -> None:
    """The `manifest.json` member of `tsra` is exactly its own JCS form (ADR-0066 rule 1, #912)."""
    with zipfile.ZipFile(tsra) as z:
        stored = z.read("manifest.json")
    assert stored == jcs(json.loads(stored)), (
        f"{what}: manifest.json is not its own JCS form: {stored[:120]!r}"
    )


corpus = pathlib.Path(sys.argv[1])
files = sorted(corpus.glob("*.tsra"))
assert files, f"no .tsra fixtures under {corpus}"

assert isinstance(tessera.__version__, str)
assert issubclass(tessera.TesseraError, Exception)

verified = 0
for f in files:
    r = tessera.open(f)
    m = r.manifest()  # a dict, parsed from the canonical JSON
    for key in ("id", "product", "name", "blocks"):
        assert key in m, f"{f.name}: manifest missing {key!r}"
    assert r.product == m["product"]
    assert r.id == m["id"]
    assert r.block_names() == [b["name"] for b in m["blocks"]]
    r.verify()
    tessera.verify(f)
    assert_stored_form(f, f.name)

    for b in m["blocks"]:
        if b["kind"] == "array":
            arr = r.array(
                b["name"]
            )  # -> ndarray, already reshaped (native dtype, C-order)
            assert isinstance(arr, np.ndarray)
            assert list(arr.shape) == [int(d) for d in b["spec"]["shape"]]
            # ROI sub-cube equals the full array's corresponding slice (only intersecting chunks read).
            # equal_nan for float dtypes: the segmentation_uint8 fixture's f16 block carries IEEE
            # specials (NaN != NaN under plain array_equal); isnan doesn't exist for int dtypes.
            if all(d >= 2 for d in arr.shape):
                half = [max(1, d // 2) for d in arr.shape]
                sub = r.array_roi(b["name"], [0] * arr.ndim, half)
                ref = arr[tuple(slice(0, h) for h in half)]
                assert np.array_equal(sub, ref, equal_nan=arr.dtype.kind == "f"), (
                    f"{f.name}: ROI"
                )
        elif b["kind"] == "table":
            df = r.table(b["name"])  # -> polars DataFrame (via Arrow)
            assert isinstance(df, pl.DataFrame)
            at = r.table_arrow(b["name"])  # -> pyarrow Table
            assert isinstance(at, pa.Table)
            assert df.height == at.num_rows
            cols = r.table_dict(b["name"])  # -> {column: ndarray}
            first = next(iter(cols))
            proj = r.column(b["name"], first)  # projected single column
            assert isinstance(proj, np.ndarray)
            assert np.array_equal(proj, cols[first]), (
                f"{f.name}: projection != full read"
            )
            assert df.height == proj.shape[0]
    verified += 1


def hide_member_before_central_directory(tsra: bytes) -> bytes:
    """`tsra` with a complete local entry (`manifest.json`, forged) hidden in front of the central
    directory, and every end-record pointer moved past it. No central-directory record mentions it, so
    `zipfile` reads the archive unchanged; a streaming reader walking the local headers lists it as one
    more member (GHSA-m665-v7pm-7grj, SPEC §6)."""
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w", zipfile.ZIP_STORED) as z:
        z.writestr("manifest.json", b'{"forged": true}')
    tiny = buf.getvalue()
    tiny_eocd = tiny.rindex(b"PK\x05\x06")
    hidden = tiny[
        : struct.unpack_from("<I", tiny, tiny_eocd + 16)[0]
    ]  # local header + bytes only

    out = bytearray(tsra)
    eocd = out.rindex(b"PK\x05\x06")
    zip64 = eocd >= 20 and out[eocd - 20 : eocd - 16] == b"PK\x06\x07"
    if zip64:
        record = struct.unpack_from("<Q", out, eocd - 20 + 8)[0]
        cd_start = struct.unpack_from("<Q", out, record + 48)[0]
    else:
        cd_start = struct.unpack_from("<I", out, eocd + 16)[0]
    k = len(hidden)
    out[cd_start:cd_start] = hidden
    eocd += k
    if struct.unpack_from("<I", out, eocd + 16)[0] != 0xFFFFFFFF:
        struct.pack_into("<I", out, eocd + 16, cd_start + k)
    if zip64:
        struct.pack_into("<Q", out, eocd - 20 + 8, record + k)
        struct.pack_into("<Q", out, record + k + 48, cd_start + k)
    return bytes(out)


# `verify` is the full `tessera verify` pass, container rule included: a member hidden where only a
# streaming reader looks is refused, though every block digest checks and `zipfile` sees nothing amiss.
with tempfile.TemporaryDirectory() as td:
    honest = files[0]
    cloaked = pathlib.Path(td) / "cloaked.tsra"
    cloaked.write_bytes(hide_member_before_central_directory(honest.read_bytes()))
    with zipfile.ZipFile(honest) as a, zipfile.ZipFile(cloaked) as b:
        assert a.namelist() == b.namelist(), "the central-directory view is unchanged"
    r = tessera.open(
        cloaked
    )  # a cheap open reads only the tail: it does not walk local headers
    for refuse in (r.verify, lambda: tessera.verify(cloaked)):
        try:
            refuse()
            raise AssertionError(
                "verified an archive a streaming zip reader reads differently"
            )
        except tessera.TesseraError as e:
            assert "not a canonical .tsra" in str(e), e

# typed-error path: a missing file raises TesseraError, not a bare exception
try:
    tessera.open(corpus / "does_not_exist.tsra")
    raise AssertionError("expected TesseraError for missing file")
except tessera.TesseraError:
    pass

# write path: build from numpy, pack, reopen through the wrapper, assert the ergonomic reads match
vol = np.arange(2 * 3 * 4, dtype="<i2").reshape(2, 3, 4)
idx = np.arange(5, dtype="<u4")
en = np.array([0.5, 1.5, 2.5, 3.5, 4.5], dtype="<f4")

with tempfile.TemporaryDirectory() as td:
    out = pathlib.Path(td) / "roundtrip.tsra"
    b = tessera.Builder("recon", "rt", "py write roundtrip", "2024-01-01T00:00:00Z")
    b.add_array("volume", "i2", list(vol.shape), vol.tobytes())
    b.add_table(
        "events", [("idx", "u4", idx.tobytes()), ("en", "f4", en.tobytes())], "idx"
    )
    b.set_field("modality", '{"_vocabulary": "DICOM", "_code": "PT"}')
    cid = b.pack(str(out))
    assert cid.startswith("blake3:")
    assert_stored_form(out, "Builder.pack")  # the Python write door stores the JCS form

    rr = tessera.open(out)
    rr.verify()
    assert np.array_equal(rr.array("volume"), vol), "array write→read mismatch"
    cols = rr.table_dict("events")
    assert np.array_equal(cols["idx"], idx) and np.array_equal(cols["en"], en)
    # polars + pyarrow views agree with the numpy view
    assert rr.table("events")["idx"].to_list() == idx.tolist()
    assert rr.table_arrow("events").column("en").to_pylist() == en.tolist()

# #734: block names are unique within a product, so packing two blocks of one name is refused
# with the typed reason, and nothing is written.
with tempfile.TemporaryDirectory() as td:
    out = pathlib.Path(td) / "dup.tsra"
    b = tessera.Builder("recon", "dup", "duplicate block name", "2024-01-01T00:00:00Z")
    b.add_array("volume", "i2", list(vol.shape), vol.tobytes())
    b.add_array("volume", "i2", list(vol.shape), vol.tobytes())
    try:
        b.pack(str(out))
        raise AssertionError("packed a product listing block 'volume' twice")
    except tessera.TesseraError as e:
        assert "listed more than once" in str(e), f"refused for the wrong reason: {e}"
    assert not out.exists(), "a refused pack must write nothing"

# #886: a sealed JSON number is a double (I-JSON, RFC 7493 §2.2), so an integer beyond ±(2^53−1)
# can't be sealed exactly. Metadata refuses it with the fix to apply (write it as a string), naming
# the value; nothing is written and nothing is converted. (`with_generation` refuses such an int
# when it is passed: tests/provenance.py.)
with tempfile.TemporaryDirectory() as td:
    out = pathlib.Path(td) / "big.tsra"
    b = tessera.Builder("recon", "big", "integer beyond 2^53", "2024-01-01T00:00:00Z")
    b.add_array("volume", "i2", list(vol.shape), vol.tobytes())
    b.set_field("modality", '{"_vocabulary": "DICOM", "_code": "CT"}')
    b.set_field("daq_start_ns", "1730000000000000000")
    try:
        b.pack(str(out))
        raise AssertionError("sealed an integer beyond 2^53")
    except tessera.TesseraError as e:
        want = (
            "metadata.daq_start_ns = 1730000000000000000 is outside the range a sealed JSON number "
            "can hold (±(2^53−1), I-JSON RFC 7493 §2.2)"
        )
        assert want in str(e) and "write it as a string" in str(e), str(e)
    assert not out.exists(), "a refused pack must write nothing"
# Written as a string, the same value seals and reads back exactly.
with tempfile.TemporaryDirectory() as td:
    out = pathlib.Path(td) / "big-str.tsra"
    b = tessera.Builder("recon", "big", "integer as a string", "2024-01-01T00:00:00Z")
    b.add_array("volume", "i2", list(vol.shape), vol.tobytes())
    b.set_field("modality", '{"_vocabulary": "DICOM", "_code": "CT"}')
    b.set_field("daq_start_ns", '"1730000000000000000"')
    b.pack(str(out))
    assert (
        tessera.open(out).manifest()["metadata"]["daq_start_ns"]
        == "1730000000000000000"
    )

# str + b1 columns through the ergonomic reads (#421): str crashed _ndarray
# ("<str" is not a dtype) and b1 silently decoded as int8 instead of bool_.
els = ["He", "Li", "", "Beryllium"]  # empty string is the edge worth pinning
framed = b"".join(struct.pack("<I", len(s.encode())) + s.encode() for s in els)
flags = np.array([1, 0, 1, 1], dtype="<u1")

with tempfile.TemporaryDirectory() as td:
    out = pathlib.Path(td) / "strcols.tsra"
    b = tessera.Builder(
        "recon", "sc", "str/b1 column roundtrip", "2024-01-01T00:00:00Z"
    )
    b.add_table(
        "props",
        [
            ("el", "str", framed),
            ("stable", "b1", flags.tobytes()),
            ("z", "u1", bytes([2, 3, 4, 4])),
        ],
        None,
    )
    b.pack(str(out))

    rr = tessera.open(out)
    rr.verify()
    cols = rr.table_dict("props")
    assert cols["el"].tolist() == els, f"str column mangled: {cols['el']!r}"
    assert cols["stable"].dtype == np.bool_, (
        f"b1 must decode as bool_, got {cols['stable'].dtype}"
    )
    assert cols["stable"].tolist() == [True, False, True, True]
    assert rr.table_arrow("props").column("el").to_pylist() == els

print(
    f"tessera-py smoke OK: {verified} corpus archives via the ergonomic surface "
    "(numpy / polars / pyarrow) + numpy write→read roundtrip"
)
