# /// script
# requires-python = ">=3.11"
# dependencies = ["cbor2==6.3.0", "jcs==0.2.1"]
# ///
"""Spike #897 pre-mortem probes: what third-party writers produce without the reference library.
Usage: uv run py/premortem.py out/vectors"""

import json
import pathlib
import sys

import cbor2
import jcs

d = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "out/vectors")


def naive(v) -> bytes:
    """The obvious "canonical JSON" a Python writer reaches for without a JCS library."""
    return json.dumps(
        v, sort_keys=True, separators=(",", ":"), ensure_ascii=False
    ).encode()


print("## A′: does json.dumps(sort_keys, compact) equal JCS?")
same = 0
corpus = sorted((d / "aprime").glob("*.json"))
for p in corpus:
    b = p.read_bytes()
    same += naive(json.loads(b)) == b
print(f"13 corpus manifests: {same}/{len(corpus)} identical")
for v in [
    1e21,
    1e-7,
    1.0,
    -0.0,
    0.1,
    5e-324,
    2**53 - 1,
    "é",
    chr(0x2028),
    "\U0001f600",
    {"b": 1, "a": 2, "é": 3, "z": 4},
]:
    a, j = naive(v), jcs.canonicalize(v)
    if a != j:
        print(f"  {v!r:28} json.dumps {a.decode()!r:24} JCS {j.decode()!r}")

print("\n## B: does cbor2.dumps (default, not canonical=True) equal the profile bytes?")
same = 0
corpus = sorted((d / "b").glob("*.cbor"))
for p in corpus:
    b = p.read_bytes()
    same += cbor2.dumps(cbor2.loads(b)) == b
print(f"13 corpus manifests, default cbor2.dumps: {same}/{len(corpus)} identical")
same = sum(
    cbor2.dumps(cbor2.loads(p.read_bytes()), canonical=True) == p.read_bytes()
    for p in corpus
)
print(
    f"13 corpus manifests, cbor2.dumps(canonical=True): {same}/{len(corpus)} identical"
)
for v in [1.5, 1.0, 100000.0, 0.1, {"bb": 1, "c": 2}]:
    print(
        f"  {v!r:18} default {cbor2.dumps(v).hex():20} canonical {cbor2.dumps(v, canonical=True).hex()}"
    )

print(
    "\n## B: smuggling past a library-only Python reader (trailing bytes after the item)"
)
b = (d / "b" / "tessera__corpus__files__recon_int16.cbor").read_bytes()
smuggled = b + cbor2.dumps({"hidden": "payload"})
v = cbor2.loads(smuggled)
print(
    f"cbor2.loads(manifest + extra item) → accepted, {len(smuggled) - len(b)} sealed bytes never seen;"
    f" name = {v['name']!r}"
)
