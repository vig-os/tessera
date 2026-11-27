"""Print one JSON per .tsra: id, content_hash, manifest_hash (seal member, or in-band for alpha.6), blocks, schema digest."""

import argparse
import json
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


parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument(
    "--relative-to", help="key fixtures by relative path, retaining duplicate basenames"
)
parser.add_argument("output")
parser.add_argument("archives", nargs="+")
args = parser.parse_args()
products = {}
for path in args.archives:
    name = (
        os.path.relpath(path, args.relative_to)[:-5]
        if args.relative_to
        else os.path.basename(path)[:-5]
    )
    if name in products:
        parser.error(f"duplicate product key: {name}; use --relative-to")
    products[name] = triple(path)
with open(args.output, "w") as output:
    json.dump(products, output, indent=1, sort_keys=True)
