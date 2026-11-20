"""
Generate ../refusals/*.tsra: archives outside SPEC §6's canonical member set, each of which every
reader refuses. The reference reader (run.py) and the Rust reader (tessera-io
tests/canonical_members.rs) open the same files.

Each archive is ../files/recon_int16.tsra with one fault in the zip layer, the manifest and its
seal left as they are. A reader that resolves a repeated name to its last entry, or inflates a
compressed member, verifies each of them: the members it ends up reading are the product's own.

- duplicate-manifest-seal.tsra: a second `manifest.seal` after the blocks. The first (entry 2)
  is the seal of other bytes, the last is the product's.
- duplicate-manifest-json.tsra: a second `manifest.json` after the blocks. The first (entry 3)
  is the manifest with another description under the product's seal, the last is the product's.
- deflated-manifest-seal.tsra, deflated-manifest-json.tsra: that member DEFLATED, not STORED.

    uv run --no-project --with-requirements requirements.txt python make_refusal_fixtures.py

Written with fixed entry metadata, so a run reproduces the committed bytes (the reference-reader
check compares them).
"""

from __future__ import annotations

import json
import os
import warnings
import zipfile

import blake3

HERE = os.path.dirname(os.path.abspath(__file__))
CORPUS_DIR = os.path.dirname(HERE)
SOURCE = os.path.join(CORPUS_DIR, "files", "recon_int16.tsra")
OUT_DIR = os.path.join(CORPUS_DIR, "refusals")

SEAL = "manifest.seal"
MANIFEST = "manifest.json"


def members() -> list[tuple[str, bytes]]:
    with zipfile.ZipFile(SOURCE) as zf:
        return [(i.filename, zf.read(i)) for i in zf.infolist()]


def write(name: str, entries: list[tuple[str, bytes, int]]) -> None:
    """Write `entries` (name, bytes, compression) in order, with fixed metadata."""
    with zipfile.ZipFile(os.path.join(OUT_DIR, name), "w") as out:
        for entry, data, method in entries:
            info = zipfile.ZipInfo(entry, date_time=(1980, 1, 1, 0, 0, 0))
            info.compress_type = method
            info.external_attr = 0o644 << 16
            # zipfile warns on a repeated name; repeating one is the point here.
            with warnings.catch_warnings():
                warnings.simplefilter("ignore", UserWarning)
                out.writestr(info, data)


def main() -> None:
    os.makedirs(OUT_DIR, exist_ok=True)
    stored = [(n, d, zipfile.ZIP_STORED) for n, d in members()]
    by_name = {n: d for n, d, _ in stored}
    seal, manifest = by_name[SEAL], by_name[MANIFEST]
    assert seal == ("blake3:" + blake3.blake3(manifest).hexdigest()).encode()

    other_seal = ("blake3:" + blake3.blake3(b"other bytes").hexdigest()).encode()
    write(
        "duplicate-manifest-seal.tsra",
        [(n, other_seal if n == SEAL else d, m) for n, d, m in stored]
        + [(SEAL, seal, zipfile.ZIP_STORED)],
    )

    forged = json.loads(manifest)
    forged["description"] = "forged"
    forged_bytes = json.dumps(forged, separators=(",", ":"), sort_keys=True).encode()
    write(
        "duplicate-manifest-json.tsra",
        [(n, forged_bytes if n == MANIFEST else d, m) for n, d, m in stored]
        + [(MANIFEST, manifest, zipfile.ZIP_STORED)],
    )

    for member, fixture in (
        (SEAL, "deflated-manifest-seal.tsra"),
        (MANIFEST, "deflated-manifest-json.tsra"),
    ):
        write(
            fixture,
            [(n, d, zipfile.ZIP_DEFLATED if n == member else m) for n, d, m in stored],
        )
    report = f"wrote 4 refusal fixtures to {OUT_DIR}"
    print(report)  # guardrails-ok: the script's report


if __name__ == "__main__":
    main()
