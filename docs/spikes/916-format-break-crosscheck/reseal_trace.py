"""Trace proof: alpha.6 manifest - manifest_hash, with only the named top-level keys taken from dev, JCS+blake3
must equal the dev seal.  usage: reseal_trace.py <a6.tsra> <dev.tsra> key[,key...]"""

import json
import sys
import zipfile
import jcs
import blake3


def mm(p):
    with zipfile.ZipFile(p) as z:
        n = z.namelist()
        return (
            json.loads(z.read("manifest.json")),
            (z.read("manifest.seal").decode() if "manifest.seal" in n else None),
            z.read("manifest.json"),
        )


a, _, _ = mm(sys.argv[1])
d, seal, dbytes = mm(sys.argv[2])
a.pop("manifest_hash", None)
keys = [k for k in sys.argv[3].split(",") if k]
for k in keys:
    a[k] = d[k]
b = jcs.canonicalize(a)
h = "blake3:" + blake3.blake3(b).hexdigest()
print(
    sys.argv[2].rsplit("/", 1)[-1],
    "keys",
    keys,
    "reseal",
    "MATCH" if h == seal and b == dbytes else f"MISMATCH {h} vs {seal}",
)
