"""Build table.json from two triple.py outputs (alpha.6 side, dev side) per corpus.

usage: python3 -I build_table.py <out.json> <about> <label>=<a6.json>,<dev.json> [...]
"""

import json
import sys

rows = []
for spec in sys.argv[3:]:
    label, files = spec.split("=", 1)
    a6_path, dev_path = files.split(",")
    a6, dev = json.load(open(a6_path)), json.load(open(dev_path))
    for name in sorted(a6):
        keys = ["id", "content_hash", "manifest_hash", "blocks"]
        rows.append(
            {
                "corpus": label,
                "name": name,
                "alpha6": {k: a6[name][k] for k in keys},
                "dev": {k: dev[name][k] for k in keys},
            }
        )
with open(sys.argv[1], "w") as f:
    json.dump({"about": sys.argv[2], "products": rows}, f, indent=1, sort_keys=True)
