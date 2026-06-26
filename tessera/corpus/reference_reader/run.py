"""
Run the independent reader over the conformance corpus and compare the triple
(id, content_hash, manifest_hash) against ../corpus.json goldens.
"""

from __future__ import annotations

import json
import os
import sys

from reader import verify_tsra

CORPUS_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))  # .../tessera/corpus
FILES_DIR = os.path.join(CORPUS_DIR, "files")
GOLDEN_JSON = os.path.join(CORPUS_DIR, "corpus.json")


def main() -> int:
    with open(GOLDEN_JSON) as f:
        goldens = json.load(f)

    print(f"{'fixture':<22} {'id':<7} {'content':<7} {'manifest':<8} {'blocks':<6} verdict")
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
            print(f"    expected/computed content  : {g['content_hash']} / {res.computed_content_hash}")
            print(f"    expected/computed manifest : {g['manifest_hash']} / {res.computed_manifest_hash}")
            for n in res.notes:
                print(f"    note: {n}")

    print("-" * 78)
    print(f"{len(goldens) - fails}/{len(goldens)} fixtures passed")
    return 0 if fails == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
