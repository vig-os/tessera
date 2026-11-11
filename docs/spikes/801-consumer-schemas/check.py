"""Structural check of the #801 drafts against tessera's serde shapes (schema.rs, block/table.rs, dtype.rs).

A Python mirror of the Rust serde shapes, not a Rust round-trip. Exits 1 on the first run with any error.
Usage (from the repo root): python3 docs/spikes/801-consumer-schemas/check.py docs/spikes/801-consumer-schemas
"""

import json
import sys
from pathlib import Path

SENS = {"public", "coded", "sensitive", "identifying", "unknown"}
KINDS = {"array", "table", "chunk_index", "blob"}
FIELD_DT = {
    "int8",
    "int16",
    "int32",
    "int64",
    "uint8",
    "uint16",
    "uint32",
    "uint64",
    "float16",
    "float32",
    "float64",
    "bool",
    "string",
    "coded",
    "json",
}
COL_DT = {"i1", "i2", "i4", "i8", "u1", "u2", "u4", "u8", "f4", "f8", "b1", "str"}
errs = []


def need(o, k, t, where):
    if k not in o:
        errs.append(f"{where}: missing {k}")
    elif not isinstance(o[k], t):
        errs.append(f"{where}: {k} not {t}")


for p in sorted(Path(sys.argv[1]).glob("*.schema.json")):
    s = json.loads(p.read_text())
    w = p.name
    for k in ("product", "version", "description"):
        need(s, k, str, w)
    for f in s.get("fields", []):
        for k in ("id", "description", "dtype"):
            need(f, k, str, f"{w}:{f.get('id')}")
        if f["dtype"] not in FIELD_DT:
            errs.append(f"{w}:{f['id']} dtype {f['dtype']}")
        if f.get("sensitivity", "public") not in SENS:
            errs.append(f"{w}:{f['id']} sensitivity")
    for b in s.get("blocks", []):
        need(b, "role", str, w)
        need(b, "min_count", int, w)
        need(b, "description", str, w)
        if "kind" in b and b["kind"] not in KINDS:
            errs.append(f"{w}:{b['role']} kind")
    t = p.with_name(p.name.replace(".schema.json", ".tables.json"))
    roles = {b["role"] for b in s.get("blocks", []) if b.get("kind") == "table"}
    if t.exists():
        tables = json.loads(t.read_text())["tables"]
        if set(tables) != roles:
            errs.append(
                f"{t.name}: roles {sorted(tables)} != table blocks {sorted(roles)}"
            )
        for role, spec in tables.items():
            names = [c["name"] for c in spec["columns"]]
            if len(names) != len(set(names)):
                errs.append(f"{t.name}:{role} duplicate column names")
            for c in spec["columns"]:
                need(c, "name", str, f"{t.name}:{role}")
                need(c, "dtype", str, f"{t.name}:{role}")
                if c["dtype"] not in COL_DT:
                    errs.append(f"{t.name}:{role}:{c['name']} dtype {c['dtype']}")
            for k in (
                spec["sequence_key"]
                + [c for key in spec["keys"] for c in key]
                + [c for r in spec["references"] for c in r["columns"]]
            ):
                if k not in names:
                    errs.append(f"{t.name}:{role}: key column {k} not declared")
            for r in spec["references"]:
                if r["to"]["scope"] == "block" and r["to"]["block"] not in tables:
                    errs.append(
                        f"{t.name}:{role}: reference to unknown block {r['to']['block']}"
                    )
    elif roles:
        errs.append(f"{w}: table roles {sorted(roles)} but no tables file")
    sys.stdout.write(
        f"checked {w}: {len(s.get('fields', []))} fields, {len(s.get('blocks', []))} blocks\n"
    )
sys.stdout.write(("\n".join(errs) or "OK: 0 errors") + "\n")
sys.exit(1 if errs else 0)
