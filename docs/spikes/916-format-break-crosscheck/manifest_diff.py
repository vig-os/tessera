"""Structural diff of two products' parsed manifest.json, path by path (alpha.6 -> dev).

usage: python3 -I manifest_diff.py <a.tsra> <b.tsra>
"""

import json
import sys
import zipfile


def manifest(path):
    with zipfile.ZipFile(path) as z:
        return json.loads(z.read("manifest.json"))


def diff(a, b, path=""):
    if isinstance(a, dict) and isinstance(b, dict):
        for k in sorted(set(a) | set(b)):
            if k not in a:
                yield f"+ {path}/{k} = {json.dumps(b[k])}"
            elif k not in b:
                yield f"- {path}/{k}"
            else:
                yield from diff(a[k], b[k], f"{path}/{k}")
    elif isinstance(a, list) and isinstance(b, list) and len(a) == len(b):
        for i, (x, y) in enumerate(zip(a, b)):
            yield from diff(x, y, f"{path}[{i}]")
    elif a != b:
        yield f"~ {path}: {json.dumps(a)} -> {json.dumps(b)}"


for line in diff(manifest(sys.argv[1]), manifest(sys.argv[2])):
    print(line)  # guardrails-ok: this helper's output is its report
