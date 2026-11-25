"""Print one JSON per .tsra: id, content_hash, manifest_hash (seal member, or in-band for alpha.6), blocks, schema digest."""

import json
import sys
import zipfile
import os


def triple(p):
    with zipfile.ZipFile(p) as z:
        names = z.namelist()
        m = json.loads(z.read("manifest.json"))
        seal = z.read("manifest.seal").decode() if "manifest.seal" in names else None
    return {
        "name": os.path.basename(p)[:-5],
        "layout": "A'" if seal else "alpha",
        "id": m.get("id"),
        "content_hash": m.get("content_hash"),
        "manifest_hash": seal or m.get("manifest_hash"),
        "blocks": {
            b["name"]: [b.get("digest"), b.get("class")] for b in m.get("blocks", [])
        },
        "id_inputs": m.get("id_inputs"),
        "schema": m.get("schema"),
        "generation": m.get("generation"),
        "top_keys": sorted(m.keys()),
    }


json.dump(
    {os.path.basename(p)[:-5]: triple(p) for p in sys.argv[2:]},
    open(sys.argv[1], "w"),
    indent=1,
    sort_keys=True,
)
