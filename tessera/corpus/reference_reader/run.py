"""
Run the independent reader over the conformance corpus and compare the triple
(id, content_hash, manifest_hash) against ../corpus.json goldens, then reach the
verdict of every stored manifest in ../seal-cases.json (the Rust reader and tessera-wasm check the
same file).
"""

from __future__ import annotations

import json
import os
import sys
import tempfile
import zipfile

from reader import (
    content_root,
    digest,
    jcs_bytes,
    schema_verdict,
    stored_verdict,
    verify_tsra,
)

CORPUS_DIR = os.path.dirname(
    os.path.dirname(os.path.abspath(__file__))
)  # .../tessera/corpus
FILES_DIR = os.path.join(CORPUS_DIR, "files")
GOLDEN_JSON = os.path.join(CORPUS_DIR, "corpus.json")
SEAL_CASES_JSON = os.path.join(CORPUS_DIR, "seal-cases.json")
SCHEMA_CASES_JSON = os.path.join(CORPUS_DIR, "schema-cases.json")
REFUSALS_DIR = os.path.join(CORPUS_DIR, "refusals")
# The committed refusal fixtures (make_refusal_fixtures.py) and the note each refusal must carry.
# The Rust reader refuses the same files (tessera-io tests/canonical_members.rs).
REFUSAL_FIXTURES = {
    "duplicate-manifest-seal.tsra": "listed more than once: ['manifest.seal']",
    "duplicate-manifest-json.tsra": "listed more than once: ['manifest.json']",
    "deflated-manifest-seal.tsra": "not STORED (SPEC §6): ['manifest.seal']",
    "deflated-manifest-json.tsra": "not STORED (SPEC §6): ['manifest.json']",
}


def check_seal_cases() -> int:
    """Each seal case's verdict; returns the number of disagreements."""
    with open(SEAL_CASES_JSON, encoding="utf-8") as f:
        cases = json.load(f)["cases"]
    fails = 0
    for c in cases:
        # The member bytes: the UTF-8 string they spell, or `manifest_hex` when they are not UTF-8.
        raw = (
            bytes.fromhex(c["manifest_hex"])
            if "manifest_hex" in c
            else c["manifest"].encode("utf-8")
        )
        got = stored_verdict(raw, c["seal"])
        if got != c["verdict"]:
            fails += 1
            print(f"seal case {c['name']}: expected {c['verdict']}, got {got}")
    print(f"{len(cases) - fails}/{len(cases)} seal cases agreed")
    return fails


def check_schema_cases(goldens: list[dict]) -> int:
    """Each schema case's verdict (SPEC §5 "Field rules", #895), and `ok` for the embedded schema
    and metadata of every corpus product that carries one; returns the number of disagreements."""
    with open(SCHEMA_CASES_JSON, encoding="utf-8") as f:
        cases = json.load(f)["cases"]
    case_fails = 0
    for c in cases:
        got = schema_verdict(c["schema"], c["metadata"])
        if got != c["verdict"]:
            case_fails += 1
            print(f"schema case {c['name']}: expected {c['verdict']}, got {got}")
    print(f"{len(cases) - case_fails}/{len(cases)} schema cases agreed")
    checked = own_fails = 0
    for g in goldens:
        with zipfile.ZipFile(os.path.join(FILES_DIR, f"{g['name']}.tsra")) as zf:
            m = json.loads(zf.read("manifest.json"))
        if m.get("schema") is None:
            continue
        checked += 1
        got = schema_verdict(m["schema"], m.get("metadata", {}))
        if got != "ok":
            own_fails += 1
            print(f"corpus product {g['name']}: its own schema gives {got}")
    print(f"{checked - own_fails}/{checked} corpus products pass their own schema")
    return case_fails + own_fails


def _resealed(src: str, dst: str, edit, encode) -> None:
    """Copy fixture `src` to `dst` with `edit` applied to its manifest and the manifest re-sealed
    (`content_hash` too, when every block still has a digest): stored as its JCS bytes, encoded by
    `encode`, with manifest.seal = digest of exactly the bytes written. Built the way a hostile
    producer would, so only the reader's own refusal can stop it."""
    with zipfile.ZipFile(src) as zf:
        members = [(i.filename, zf.read(i.filename)) for i in zf.infolist()]
    manifest = json.loads(dict(members)["manifest.json"])
    edit(manifest)
    digests = [b.get("digest") for b in manifest["blocks"]]
    if all(isinstance(d, str) for d in digests):
        manifest["content_hash"] = content_root(manifest["blocks"])
    stored = encode(jcs_bytes(manifest).decode("utf-8"))
    with zipfile.ZipFile(dst, "w", zipfile.ZIP_STORED) as out:
        for name, data in members:
            if name == "manifest.json":
                data = stored
            elif name == "manifest.seal":
                data = digest(stored).encode("ascii")
            out.writestr(name, data)


def _listed_twice(second_digest: str | None):
    """The #734 verify bypass: the first block listed a second time, carrying `second_digest` (or
    its own digest when None)."""

    def edit(manifest: dict) -> None:
        second = dict(manifest["blocks"][0])
        if second_digest is not None:
            second["digest"] = second_digest
        manifest["blocks"].append(second)

    return edit


def _orphan_index(cls: str | None):
    """#720 round 5: a chunk index of a block the product doesn't hold, of class `cls` (None =
    primary). Its payload is the index's own bytes, so only the block list is wrong."""

    def edit(manifest: dict) -> None:
        block = {
            "name": "absent.cidx",
            "kind": "chunk_index",
            "digest": digest(b"index"),
            "spec": {"recipe": "chunk_index@1", "indexes": "absent"},
        }
        if cls is not None:
            block["class"] = cls
        manifest["blocks"].append(block)

    return edit


def _first_block(key: str, value: object):
    """The first block's `key` set to `value`, or deleted when `value` is `_DELETE`."""

    def edit(manifest: dict) -> None:
        if value is _DELETE:
            del manifest["blocks"][0][key]
        else:
            manifest["blocks"][0][key] = value

    return edit


_DELETE = object()


def _unchanged(manifest: dict) -> None:
    """No edit: the case changes the archive's bytes, not the manifest."""


def utf8(text: str) -> bytes:
    """The manifest's encoding in a well-formed archive (SPEC §4)."""
    return text.encode("utf-8")


def same(path: str) -> None:
    """No change to the written archive."""


def _drop_member(name: str):
    """The archive rewritten without member `name`, entry order otherwise kept."""

    def post(path: str) -> None:
        with zipfile.ZipFile(path) as zf:
            members = [(i.filename, zf.read(i.filename)) for i in zf.infolist()]
        with zipfile.ZipFile(path, "w", zipfile.ZIP_STORED) as out:
            for n, data in members:
                if n != name:
                    out.writestr(n, data)

    return post


def _first_member_marked_encrypted(path: str) -> None:
    """Set the 'encrypted' flag (bit 0) on the archive's first member, in its local header and
    its central-directory entry: a zip reader can no longer read it."""
    with open(path, "r+b") as f:
        data = bytearray(f.read())
        for signature, flag_at in ((b"PK\x03\x04", 6), (b"PK\x01\x02", 8)):
            i = data.index(signature) + flag_at
            data[i] |= 1
        f.seek(0)
        f.write(data)


def check_refusals(goldens: list[dict]) -> int:
    """`verify_tsra` refuses each crafted archive for its reason, and returns a result rather than
    raising: a block listed twice (SPEC §5), whether the second entry's digest is foreign or the
    block's own; a chunk index whose block the product doesn't hold (SPEC §5d, #720); a block name that is not a string; a block class other than "derived"; a block
    without a digest; a manifest.json in
    UTF-16 or with a byte-order mark (SPEC §4: UTF-8 I-JSON); a manifest.json that is not its own
    JCS form (ADR-0066 rule 4); an archive without manifest.seal (the alpha layout, rule 10); an
    archive zipfile can't read (a member marked encrypted); and the committed fixtures outside
    SPEC §6's member set (a repeated or a DEFLATED manifest.seal or manifest.json, each of which a
    last-entry-wins reader verifies). Returns the number of misses."""
    src = None
    for g in goldens:
        path = os.path.join(FILES_DIR, f"{g['name']}.tsra")
        with zipfile.ZipFile(path) as zf:
            if json.loads(zf.read("manifest.json"))["blocks"]:
                src = path
                break
    assert src is not None, "no corpus fixture with a block to list twice"
    # (case, manifest edit, note the refusal must carry, manifest encoding, archive post-edit)
    refusals = [
        (
            "block listed twice (foreign digest)",
            _listed_twice(digest(b"NOT THE BYTES")),
            "listed more than once",
            utf8,
            same,
        ),
        (
            "block listed twice (own digest)",
            _listed_twice(None),
            "listed more than once",
            utf8,
            same,
        ),
        (
            "block name an object",
            _first_block("name", {}),
            "blocks[0].name is not a string",
            utf8,
            same,
        ),
        (
            "block class spelled 'primary'",
            _first_block("class", "primary"),
            "blocks[0].class is 'primary'",
            utf8,
            same,
        ),
        (
            "chunk index without its block (derived)",
            _orphan_index("derived"),
            "chunk indexes without their block",
            utf8,
            same,
        ),
        (
            "block without a digest",
            _first_block("digest", _DELETE),
            "without a digest",
            utf8,
            same,
        ),
        (
            "manifest in UTF-16",
            _unchanged,
            "not JSON",
            lambda text: text.encode("utf-16"),
            same,
        ),
        (
            "manifest with a byte-order mark",
            _unchanged,
            "not JSON",
            lambda text: b"\xef\xbb\xbf" + utf8(text),
            same,
        ),
        (
            "manifest not in its JCS form (pretty, resealed)",
            _unchanged,
            "non-canonical bytes",
            lambda text: utf8(json.dumps(json.loads(text), indent=2)),
            same,
        ),
        (
            "no manifest.seal (the alpha layout)",
            _unchanged,
            "alpha",
            utf8,
            _drop_member("manifest.seal"),
        ),
        (
            "first member marked encrypted",
            _unchanged,
            "encrypted",
            utf8,
            _first_member_marked_encrypted,
        ),
    ]
    misses = 0
    with tempfile.TemporaryDirectory() as tmp:
        for case, edit, reason, encode, post in refusals:
            dst = os.path.join(tmp, "crafted.tsra")
            _resealed(src, dst, edit, encode)
            post(dst)
            res = verify_tsra(dst)
            refused = not res.ok and any(reason in n for n in res.notes)
            print(f"refusal: {case}: {'refused' if refused else 'MISSED'}")
            misses += 0 if refused else 1
    fixtures = sorted(os.listdir(REFUSALS_DIR))
    assert fixtures == sorted(REFUSAL_FIXTURES), fixtures
    for fixture in fixtures:
        res = verify_tsra(os.path.join(REFUSALS_DIR, fixture))
        refused = not res.ok and any(REFUSAL_FIXTURES[fixture] in n for n in res.notes)
        print(f"refusal: {fixture}: {'refused' if refused else 'MISSED'}")
        misses += 0 if refused else 1
    total = len(refusals) + len(fixtures)
    print(f"{total - misses}/{total} refusal cases refused")
    return misses


def check_derived_block(goldens: list[dict]) -> int:
    """ADR-0028 §4 (#720) on a whole archive: a corpus product with a derived chunk-index block
    added (its payload a member like any block's, its ref `"class": "derived"`, the manifest
    resealed) verifies with the golden content_hash unchanged and a new seal; the same archive with
    the derived block's payload altered is refused, since every block's bytes are checked against
    its digest whatever its class. Returns the number of misses."""
    g = next(g for g in goldens if g["name"] == "recon_int16")
    src = os.path.join(FILES_DIR, "recon_int16.tsra")
    misses = 0
    with tempfile.TemporaryDirectory() as tmp:
        for case, stored, want_ok in [
            ("derived block added", b"index", True),
            ("derived block's payload altered", b"index, altered", False),
        ]:
            dst = os.path.join(tmp, "derived.tsra")

            def edit(manifest: dict) -> None:
                manifest["blocks"].append(
                    {
                        "name": "volume.cidx",
                        "kind": "chunk_index",
                        "digest": digest(b"index"),
                        "class": "derived",
                        "spec": {"recipe": "chunk_index@1", "indexes": "volume"},
                    }
                )

            _resealed(src, dst, edit, utf8)
            with zipfile.ZipFile(dst, "a", zipfile.ZIP_STORED) as zf:
                zf.writestr("blocks/volume.cidx", stored)
            res = verify_tsra(dst)
            held = (
                res.ok == want_ok
                and res.computed_content_hash == g["content_hash"]
                and res.computed_manifest_hash != g["manifest_hash"]
            )
            print(f"derived: {case}: {'held' if held else 'MISSED'}")
            misses += 0 if held else 1
    return misses


def main() -> int:
    with open(GOLDEN_JSON) as f:
        goldens = json.load(f)

    print(
        f"{'fixture':<22} {'id':<7} {'content':<7} {'manifest':<8} {'blocks':<6} verdict"
    )
    print("-" * 78)

    fails = 0
    for g in goldens:
        name = g["name"]
        res = verify_tsra(os.path.join(FILES_DIR, f"{name}.tsra"))
        id_ok = res.computed_id == g["id"]
        ch_ok = res.computed_content_hash == g["content_hash"]
        mh_ok = res.computed_manifest_hash == g["manifest_hash"]
        blocks_ok = all(b[1] for b in res.block_digest_ok)
        verdict = "PASS" if (id_ok and ch_ok and mh_ok and blocks_ok) else "FAIL"
        if verdict == "FAIL":
            fails += 1
        print(
            f"{name:<22} {'ok' if id_ok else 'BAD':<7} {'ok' if ch_ok else 'BAD':<7} "
            f"{'ok' if mh_ok else 'BAD':<8} {'ok' if blocks_ok else 'BAD':<6} {verdict}"
        )
        if verdict == "FAIL":
            print(f"    expected/computed id       : {g['id']} / {res.computed_id}")
            print(
                f"    expected/computed content  : {g['content_hash']} / {res.computed_content_hash}"
            )
            print(
                f"    expected/computed manifest : {g['manifest_hash']} / {res.computed_manifest_hash}"
            )
            for n in res.notes:
                print(f"    note: {n}")

    print("-" * 78)
    print(f"{len(goldens) - fails}/{len(goldens)} fixtures passed")
    fails += check_seal_cases()
    fails += check_schema_cases(goldens)
    misses = check_refusals(goldens) + check_derived_block(goldens)
    return 0 if fails == 0 and misses == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
