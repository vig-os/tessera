# /// script
# requires-python = ">=3.11"
# dependencies = ["cbor2==6.3.0", "jcs==0.2.1", "blake3==1.0.11"]
# ///
"""Spike #897 reader agreement, Python side: what does a reference-reader-style Python reader make
of the same bytes the Rust prototype wrote?  Usage: uv run py/agree.py out/vectors"""

import json
import pathlib
import sys

import blake3
import cbor2
import jcs

MAX_EXACT = 2**53 - 1
d = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "out/vectors")
WANT = {
    "epoch_ns": 1730000000123456789,
    "i64_min": -(2**63),
    "p53_plus_1": 9007199254740993,
    "u64_max": 2**64 - 1,
}


def digest(b: bytes) -> str:
    return "blake3:" + blake3.blake3(b).hexdigest()


def exact(got: dict) -> str:
    return ", ".join(
        f"{k}={'exact' if got.get(k) == v else repr(got.get(k))}"
        for k, v in WANT.items()
    )


def refuse_wide(tok: str):
    """The reference reader's I-JSON hook (PR #894): refuse an integer a double can't hold."""
    v = int(tok)
    if abs(v) > MAX_EXACT:
        raise ValueError(f"{tok} exceeds ±(2^53−1)")
    return v


print("## 64-bit values")
raw = (d / "ints.json").read_bytes()
print("A  json.loads (plain)        ", exact(json.loads(raw)))
try:
    json.loads(raw, parse_int=refuse_wide)
    print("A  json.loads (I-JSON hook)   accepted")
except ValueError as e:
    print("A  json.loads (I-JSON hook)   refused:", e)
s = json.loads((d / "ints_str.json").read_bytes())
print("A/A′ strings + int()          ", exact({k: int(v) for k, v in s.items()}))
c = cbor2.loads((d / "ints.cbor").read_bytes())
print(
    "B  cbor2.loads                ",
    exact(c),
    "| types:",
    sorted({type(v).__name__ for v in c.values()}),
)

print("\n## Corpus: does Python reproduce the stored bytes and the seal?")
seals = json.loads((d / "seals.json").read_bytes())
ok_ap = ok_b = ok_b0 = ok_seal = 0
for stem, s in seals.items():
    ap = (d / "aprime" / f"{stem}.json").read_bytes()
    b = (d / "b" / f"{stem}.cbor").read_bytes()
    same_ap = jcs.canonicalize(json.loads(ap)) == ap
    same_b = cbor2.dumps(cbor2.loads(b), canonical=True) == b
    b0 = (d / "b0" / f"{stem}.cbor").read_bytes()
    ok_b0 += cbor2.dumps(cbor2.loads(b0), canonical=True) == b0
    seal = digest(ap) == s["aprime"] and digest(b) == s["b"]
    ok_ap += same_ap
    ok_b += same_b
    ok_seal += seal
    if not (same_ap and same_b and seal):
        print(
            f"  {stem}: A′ JCS re-encode {same_ap}, B canonical re-encode {same_b}, seals {seal}"
        )
n = len(seals)
print(f"A′ jcs.canonicalize(json.loads(b)) == b : {ok_ap}/{n}")
print(f"B  cbor2.dumps(loads(b), canonical=True) == b : {ok_b}/{n}  (profile B)")
print(f"B0 cbor2.dumps(loads(b), canonical=True) == b : {ok_b0}/{n}  (profile B0)")
print(f"blake3 seals over stored bytes match Rust : {ok_seal}/{n}")

print("\n## Out-of-profile CBOR: does cbor2.loads refuse it on its own?")
for p in sorted((d / "bad").glob("*.cbor")):
    b = p.read_bytes()
    try:
        v = cbor2.loads(b)
        canon = cbor2.dumps(v, canonical=True) == b
        print(
            f"  {p.stem:28} accepted → {v!r:.40}  (canonical re-encode equal: {canon})"
        )
    except Exception as e:  # noqa: BLE001 - a probe reports every refusal class
        print(f"  {p.stem:28} refused: {type(e).__name__}: {e}")

print("\n## Lone surrogate and a float-spelled integer")
try:
    v = json.loads((d / "lone_surrogate.json").read_bytes())
    print("A  json.loads lone surrogate  accepted →", repr(v["description"]))
except ValueError as e:
    print("A  json.loads lone surrogate  refused:", e)
try:
    jcs.canonicalize({"description": "\ud800"})
    print("A′ jcs.canonicalize lone surrogate accepted")
except UnicodeEncodeError as e:
    print("A′ jcs.canonicalize lone surrogate refused:", type(e).__name__)
v = json.loads((d / "shape_float.json").read_bytes())
print(
    "A  json.loads shape           ", v["shape"], [type(x).__name__ for x in v["shape"]]
)
v = cbor2.loads((d / "shape_float.cbor").read_bytes())
print(
    "B0 cbor2.loads shape          ", v["shape"], [type(x).__name__ for x in v["shape"]]
)
