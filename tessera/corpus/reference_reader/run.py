"""
Run the independent reader over the conformance corpus and compare the triple
(id, content_hash, manifest_hash) against ../corpus.json goldens, then reach the
verdict of every manifest in ../seal-cases.json (the Rust reader checks the same file).
"""

from __future__ import annotations

import json
import os
import sys
import tempfile
import zipfile

from reader import digest, manifest_seal, manifest_verdict, merkle_root, verify_tsra

CORPUS_DIR = os.path.dirname(
    os.path.dirname(os.path.abspath(__file__))
)  # .../tessera/corpus
FILES_DIR = os.path.join(CORPUS_DIR, "files")
GOLDEN_JSON = os.path.join(CORPUS_DIR, "corpus.json")
SEAL_CASES_JSON = os.path.join(CORPUS_DIR, "seal-cases.json")


def check_seal_cases() -> int:
    """Each seal case's verdict; returns the number of disagreements."""
    with open(SEAL_CASES_JSON, encoding="utf-8") as f:
        cases = json.load(f)["cases"]
    fails = 0
    for c in cases:
        got = manifest_verdict(c["manifest"])
        if got != c["verdict"]:
            fails += 1
            print(f"seal case {c['name']}: expected {c['verdict']}, got {got}")
    print(f"{len(cases) - fails}/{len(cases)} seal cases agreed")
    return fails


def _listed_twice(src: str, dst: str, second_digest: str | None) -> None:
    """Copy fixture `src` to `dst` with its first block listed a second time (carrying
    `second_digest`, or its own digest when None) and the manifest re-sealed over that list:
    the #734 verify bypass, built the way a hostile producer would."""
    with zipfile.ZipFile(src) as zf:
        members = [(i.filename, zf.read(i.filename)) for i in zf.infolist()]
    manifest = json.loads(dict(members)["manifest.json"])
    second = dict(manifest["blocks"][0])
    if second_digest is not None:
        second["digest"] = second_digest
    manifest["blocks"].append(second)
    manifest["content_hash"] = merkle_root([b["digest"] for b in manifest["blocks"]])
    manifest["manifest_hash"] = manifest_seal(manifest)
    with zipfile.ZipFile(dst, "w", zipfile.ZIP_STORED) as out:
        for name, data in members:
            out.writestr(
                name, json.dumps(manifest) if name == "manifest.json" else data
            )


def check_refusals(goldens: list[dict]) -> int:
    """The reader refuses a manifest listing a block twice, for that reason (SPEC §5), whether the
    second entry's digest is foreign or the block's own. Returns the number of misses."""
    src = None
    for g in goldens:
        path = os.path.join(FILES_DIR, f"{g['name']}.tsra")
        with zipfile.ZipFile(path) as zf:
            if json.loads(zf.read("manifest.json"))["blocks"]:
                src = path
                break
    assert src is not None, "no corpus fixture with a block to list twice"
    refusals = [
        ("foreign digest", digest(b"NOT THE BYTES")),
        ("own digest", None),
    ]
    misses = 0
    with tempfile.TemporaryDirectory() as tmp:
        for case, second in refusals:
            dst = os.path.join(tmp, "dup.tsra")
            _listed_twice(src, dst, second)
            res = verify_tsra(dst)
            refused = not res.ok and any(
                "listed more than once" in n for n in res.notes
            )
            print(
                f"refusal: block listed twice ({case}): {'refused' if refused else 'MISSED'}"
            )
            misses += 0 if refused else 1
    print(f"{len(refusals) - misses}/{len(refusals)} refusal cases refused")
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
    misses = check_refusals(goldens)
    return 0 if fails == 0 and misses == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
