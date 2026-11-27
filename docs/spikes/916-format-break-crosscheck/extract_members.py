"""Extract manifest.json + manifest.seal (bytes, hex) of each .tsra given, for the wasm run."""

import json
import sys
import zipfile

out = []
for p in sys.argv[2:]:
    with zipfile.ZipFile(p) as z:
        names = z.namelist()
        mj = z.read("manifest.json")
        ms = z.read("manifest.seal") if "manifest.seal" in names else None
    out.append(
        {
            "path": p,
            "manifest_hex": mj.hex(),
            "seal_hex": ms.hex() if ms is not None else None,
        }
    )
json.dump(out, open(sys.argv[1], "w"))
print(len(out), "products extracted")
