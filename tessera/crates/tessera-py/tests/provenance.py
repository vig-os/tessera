"""The Python provenance surface: pinned source edges (#563).

Run as: python3 provenance.py

Every assertion reads the SEALED manifest back through `tessera.open(...).manifest()`. The builder's
in-memory state is not what a reader sees, so checking it would prove nothing about the file.
"""

import pathlib
import tempfile

import numpy as np

import tessera

out = pathlib.Path(tempfile.mkdtemp())
VOLUME = np.arange(8, dtype="<i2").tobytes()


def builder(name: str) -> "tessera.Builder":
    b = tessera.Builder("t_product", name, "", "2024-01-01T00:00:00Z")
    b.add_array("volume", "i2", [2, 2, 2], VOLUME)
    return b


def sealed(b: "tessera.Builder", name: str) -> dict:
    b.pack(str(out / f"{name}.tsra"))
    return tessera.open(out / f"{name}.tsra").manifest()


# --- #563: add_source pins the parent's seal --------------------------------------------------
parent = sealed(builder("parent"), "parent")
parent_seal = parent["manifest_hash"]
assert parent_seal.startswith("blake3:"), parent_seal

pinned = builder("pinned")
pinned.add_source("derived_from", "parent.tsra", content_hash=parent_seal)
got = sealed(pinned, "pinned")["sources"]
assert got == [
    {"role": "derived_from", "reference": "parent.tsra", "content_hash": parent_seal}
], got

# Without the pin the key is ABSENT, not null: a product written by an old caller seals exactly as
# before, so existing products and the conformance corpus keep their hashes.
unpinned = builder("unpinned")
unpinned.add_source("derived_from", "parent.tsra")
got = sealed(unpinned, "unpinned")["sources"]
assert got == [{"role": "derived_from", "reference": "parent.tsra"}], got

# Positional form, same edge (the signature mirrors Rust's `Source::new(..).with_content_hash(..)`).
positional = builder("positional")
positional.add_source("derived_from", "parent.tsra", parent_seal)
assert sealed(positional, "positional")["sources"][0]["content_hash"] == parent_seal

print("provenance: ok")
