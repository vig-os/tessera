"""Reference reader over every corpus product (.tsra) against the committed goldens. argv: reader_dir corpus_dir tsra..."""

import json
import os
import sys

sys.path.insert(0, sys.argv[1])
import reader  # noqa: E402

cd = sys.argv[2]
g = {x["name"]: x for x in json.load(open(os.path.join(cd, "corpus.json")))}
g.update(
    {
        x["name"]: x
        for x in json.load(open(os.path.join(cd, "ingest-corpus.json")))["fixtures"]
    }
)
ok = 0
for p in sys.argv[3:]:
    n = os.path.basename(p)[:-5]
    r = reader.verify_tsra(p)
    gg = g[n]
    match = (
        r.ok
        and r.computed_id == gg["id"]
        and r.computed_content_hash == gg["content_hash"]
        and r.computed_manifest_hash == gg["manifest_hash"]
        and all(b for _, b in r.block_digest_ok)
    )
    ok += match
    print(  # guardrails-ok: this helper's output is its report
        json.dumps(
            {
                "name": n,
                "ok": r.ok,
                "golden_match": match,
                "blocks": len(r.block_digest_ok),
                "notes": r.notes,
            }
        )
    )
print(
    f"{ok}/{len(sys.argv) - 3} products verify and match their goldens (reference reader)"
)
