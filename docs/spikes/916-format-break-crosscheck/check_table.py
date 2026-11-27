"""Check that every shared sealed archive is paired, with exact hashes and block digests.

Usage: python3 -I check_table.py table.json v0.1.0-alpha.6 <dev-revision>
Ingest products are generated separately, rather than tracked as archives.
"""

import io
import json
import subprocess
import sys
import zipfile


def tracked(revision):
    return set(
        subprocess.check_output(
            ["git", "ls-tree", "-r", "--name-only", revision], text=True
        ).splitlines()
    )


def stored(revision, path):
    raw = subprocess.check_output(["git", "show", f"{revision}:{path}"])
    with zipfile.ZipFile(io.BytesIO(raw)) as archive:
        manifest = json.loads(archive.read("manifest.json"))
        seal = (
            archive.read("manifest.seal").decode()
            if "manifest.seal" in archive.namelist()
            else None
        )
    return manifest, seal


table_path, alpha6, dev = sys.argv[1:]
rows = json.load(open(table_path))["products"]
indexed = {(row["corpus"], row["name"]): row for row in rows}
assert len(indexed) == len(rows), "duplicate table keys"
expected_fixtures = {
    "tessera/corpus/files/blob_opaque.tsra",
    "tessera/corpus/files/empty_no_blocks.tsra",
    "tessera/corpus/files/listmode_events.tsra",
    "tessera/corpus/files/multiblock_study.tsra",
    "tessera/corpus/files/recon_float32_mumap.tsra",
    "tessera/corpus/files/recon_int16.tsra",
    "tessera/corpus/files/segmentation_uint8.tsra",
    "tessera/corpus/files/spectrum_lifetime.tsra",
    "tessera/crates/tessera-cli/tests/cmd/logical_tables.in/scan.tsra",
    "tessera/crates/tessera-cli/tests/cmd/persona_signing.in/study.tsra",
    "tessera/crates/tessera-cli/tests/cmd/versioning.in/study.tsra",
    "tessera/crates/tessera-io/tests/fixtures/legacy_flat_events.tsra",
    "tessera/crates/tessera-py/tests/fixtures/partitioned_pairs.tsra",
}
checked = 0
checked_fixtures = set()
for path in sorted(tracked(alpha6) & tracked(dev)):
    if not path.endswith(".tsra"):
        continue
    current, seal = stored(dev, path)
    if (
        seal is None
    ):  # alpha_layout.tsra is a refusal, outside the paired sealed products.
        continue
    corpus = (
        "corpus/files" if path.startswith("tessera/corpus/files/") else "test fixtures"
    )
    name = path.rsplit("/", 1)[1][:-5] if corpus == "corpus/files" else path[:-5]
    row = indexed.get((corpus, name))
    assert row is not None, f"missing paired fixture: {path}"
    for side, revision in [("alpha6", alpha6), ("dev", dev)]:
        manifest, seal = stored(revision, path)
        expected = {
            "id": manifest["id"],
            "content_hash": manifest["content_hash"],
            "manifest_hash": seal or manifest["manifest_hash"],
            "blocks": {
                block["name"]: [block["digest"], block.get("class")]
                for block in manifest.get("blocks", [])
            },
        }
        assert row[side] == expected, f"{side} drift: {path}"
    checked += 1
    checked_fixtures.add(path)
assert checked == 13, f"expected 13 paired archives, checked {checked}"
assert checked_fixtures == expected_fixtures, (
    f"paired fixture names drifted: {checked_fixtures ^ expected_fixtures}"
)
print(f"{checked}/{checked} shared sealed archives match exhaustive table")
