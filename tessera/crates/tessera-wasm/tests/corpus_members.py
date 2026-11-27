"""Extract stored corpus manifest members for the Node conformance gate (stdlib only)."""

import json
from pathlib import Path
import sys
import zipfile

corpus = Path(sys.argv[1])
products = []
for golden in json.loads((corpus / "corpus.json").read_text()):
    with zipfile.ZipFile(corpus / "files" / f"{golden['name']}.tsra") as archive:
        products.append(
            {
                "golden": golden,
                "manifest_hex": archive.read("manifest.json").hex(),
                "seal_hex": archive.read("manifest.seal").hex(),
            }
        )
Path(sys.argv[2]).write_text(json.dumps(products))
