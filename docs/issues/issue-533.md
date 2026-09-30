---
type: issue
state: closed
created: 2026-09-29T15:44:29Z
updated: 2026-09-29T17:37:06Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/533
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-30T07:57:00.970Z
---

# [Issue 533]: [feat(py): expose the array codec on Builder.add_array — the binding hard-codes pcodec](https://github.com/vig-os/tessera/issues/533)

## The gap

`tessera-py`'s `Builder.add_array` hard-codes the array codec:

```rust
spec.codec = "pcodec".into(); // tessera-py/src/lib.rs:260
```

and takes no codec argument. The **Rust** API has the choice — `tessera_io::array` validates and dispatches `"pcodec"`, `"zstd"` and `"auto"`, and its own module docs say *"this is the reason `zstd` / `auto` are safe to expose"*. The Python binding simply never wired it.

So a Python caller cannot tune the array path at all. That is a usability gap on its own, and it also showed up in the #485 cross-ecosystem bench: every other format is reported at a default **and** its standard tuning, while Tessera could only be reported at one setting — documented there as `SINGLE_VARIANT_REASON` rather than papered over with an invented knob.

## Why it matters — the choice is worth ~20% either way

Measured through the binding, 256³ int16 (32 MiB raw), bit-exact both ways:

| fixture | pcodec | zstd | auto picks | best |
|---|---|---|---|---|
| **gradient** (linear ramp — constant deltas, long LZ matches) | 0.250 MiB | **0.220 MiB** | zstd | zstd |
| **acquired** (phantom + detector noise — a real reconstruction) | **13.117 MiB** | 15.972 MiB | pcodec | pcodec |

Neither codec dominates: pcodec is **18% smaller** on acquisition-shaped data, zstd is **12% smaller** on synthetic ramps and anything with long byte-level repeats (masks, packed bitfields). `"auto"` picked the winner in both directions.

The ratio is also size-invariant on realistic data (pcodec/zstd ≈ 0.82 measured across n = 64…320), so this is not a small-file artifact — the absolute saving scales linearly with volume size, which is the whole point for archival PET/CT.

## The change

```rust
#[pyo3(signature = (name, code, shape, data, codec="pcodec"))]
fn add_array(&mut self, name: &str, code: &str, shape: Vec<u64>, data: &[u8], codec: &str)
```

A pass-through. `array_block` already validates the value and rejects anything else with a clear error, so no new validation is added and no behaviour changes for existing callers — the default stays `"pcodec"`.

## Guard

`tests/api_drift.py` (the docstring-vs-behaviour gate, #412) now probes the live module for `pcodec` / `zstd` / `auto` **and** for `lz4`, `""`, `PCODEC`, asserting the accepted set is exactly the three the docstring advertises. It also asserts `add_array` is still callable *without* a codec, so the default cannot be lost silently.

Verified the gate is live rather than vacuously passing: the probe returns `['auto', 'pcodec', 'zstd']` and the unknown codec is rejected with

```
codec: array codec 'lz4' is not supported (expected 'pcodec', 'zstd', or 'auto')
```

Refs: #485

