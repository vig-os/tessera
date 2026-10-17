---
type: issue
state: open
created: 2026-10-16T20:45:21Z
updated: 2026-10-16T20:45:21Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/567
comments: 0
labels: feature, python, area:bindings
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-17T06:32:48.222Z
---

# [Issue 567]: [feat(py): expose with_schema on Builder — a domain product schema (requires_generation, tiers, inherit) can't be embedded from Python](https://github.com/vig-os/tessera/issues/567)

## Context

Dogfooding from MorePET/anatomist#86 (ADR-0007 draft) at tessera `dev` `fa581f5`, synthetic data only.

ADR-0058 §3 says the "needs a recipe" rule is schema data and that "a domain schema is authoritative and may tighten this to `true`". The Rust write API supports it: `ProductBuilder::with_schema(value)` embeds a caller-supplied `ProductSchema`, and `validate_manifest` prefers the embedded schema over the built-in registry (`tessera-core/src/schema.rs:333`). `tessera schema <file>` then enforces `requires_generation`, required fields, sensitivity tiers and `inherit` from the file's own contract.

## Gap

The pyo3 `Builder` has no `with_schema` (its methods are `add_array`, `add_table`, `set_field`, `add_source`, `pack`). A Python producer with a domain product kind (anatomist writes `anatomist.job` and per-step derived products) gets no embedded schema, and `tessera schema` reports it as open-world and permissive:

```python
import json, numpy as np, tessera
b = tessera.Builder("anatomist.job", "probe", "domain product kind probe", "2026-10-16T00:00:00Z")
b.add_array("x", "i2", [2], np.array([1, 2], dtype="<i2").tobytes(), codec="zstd")
b.pack("probe.tsra")
print(json.loads(tessera.open("probe.tsra").manifest_json()).get("schema"))   # None
```

```console
$ tessera schema probe.tsra
product 'anatomist.job' — unknown schema (open-world: validation is permissive)
OK  schema-valid: probe.tsra
```

So a Python-written derived product can't carry `requires_generation = true`. Even once #562 lands (`with_generation`), nothing in the file enforces that the recipe is present. The domain's field sensitivity tiers (ADR-0040) and `inherit` flags (ADR-0058 §5) can't be declared from Python either.

Unpacking, editing `manifest.json` and running `tessera pack` is not a workaround: `pack` checks the old seal and refuses (`integrity: id mismatch`), which is correct.

## Ask

`Builder.with_schema(schema_json: str)`, mirroring `ProductBuilder::with_schema`: parse the JSON as a `ProductSchema` at call time so a malformed schema fails before `pack`, and embed it so the seal commits to it. Optional: have `pack()` run `validate_manifest` and raise on a violation, so a Python producer can't seal a product its own schema rejects. Today `seal` doesn't validate at all; `tessera schema` is the only check.

## Linked

#562 (with_producer / with_generation, the recipe half), #482 (no built-in schema sets `requires_generation`), MorePET/anatomist#86.

