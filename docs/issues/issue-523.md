---
type: issue
state: open
created: 2026-09-29T14:20:04Z
updated: 2026-09-29T14:20:04Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/523
comments: 0
labels: bug, area:core
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-30T07:57:04.946Z
---

# [Issue 523]: [fix(core): ChunkStats sum_sq overflows for 64-bit samples — silently wraps into a sealed .cidx in release builds](https://github.com/vig-os/tessera/issues/523)

## What happens

`ChunkStats::from_values` (and therefore `Monoid::combine`) accumulates `sum_sq: i128` with plain `+`.
For 64-bit sample values that overflows:

- `i64::MAX²` ≈ 8.5e37 and `i128::MAX` ≈ 1.7e38 → **three** `i64::MAX` samples overflow.
- (The `u64` side of the same arithmetic is worse: `u64::MAX²` ≈ 3.4e38 vs `u128::MAX` ≈ 3.4e38 → **two**.)

Reproduced on `dev` (c5a5163):

```rust
let v = vec![i64::MAX, i64::MAX, i64::MAX];
let s = tessera_core::chunk_index::ChunkStats::from_values(&v);
```
```
thread '...' panicked at crates/tessera-core/src/chunk_index.rs:117:21:
attempt to add with overflow
```

## Why it matters more than a panic

`crates/tessera-core/src/chunk_index.rs:117` is inside `combine`, which is on the **write path**:
`array_chunk_index` → `ChunkStats::from_values` → the `.cidx` sidecar that gets **sealed**.

Debug builds panic. **Release builds wrap silently**, and the wrapped `sum_sq` is then serialized into
the `.cidx`, so a product would carry a sealed, content-hashed, wrong `variance`/`std_dev` with no
indication anything went wrong. A reader that trusts the index (as #347 now does, for exactly the
right reasons) would serve that wrong number and label it `exact`.

`count`, `min`, `max` and `sum` are unaffected — `sum` of `i64` into `i128` cannot overflow for any
realistic voxel count. It is specifically `sum_sq`, and therefore `variance`/`std_dev`.

## Scope

Real medical volumes are `int16`/`uint16`, where this cannot happen (`i32::MAX²` is 2^62 and you would
need >2^65 samples). It bites `int64`/`uint64` arrays with extreme magnitudes — synthetic data, counters,
timestamps-as-values, or anything adversarial.

## Options

1. **Checked accumulation + an inexact signal.** `checked_add`/`checked_mul`; on overflow mark the stat
   as not-exact rather than storing a wrong number. Needs somewhere to put that signal — a field on
   `ChunkStats` is a `.cidx` schema change (see #522 for the ordering constraint), so it is not free.
2. **Saturate and refuse to index.** On overflow, emit no sidecar for that block (the precedent already
   exists: `as_i64()` returns `None` for `u64 > i64::MAX`, so those arrays get no index at all). Cheapest,
   no schema change, and fails closed.
3. **A wider accumulator** (`i256`/`u256` via a small helper or a dep). Exact for every input, but changes
   the serialized field type — a `.cidx` schema change, and a dependency question.

**(2) looks right for now**: it fails closed, matches the existing `as_i64` precedent, needs no format
change, and leaves (1)/(3) open if exact stats on extreme 64-bit arrays ever become a real requirement.

## Note

Found while building #347 P2, by a test that fed `u64::MAX` through the *read* path. The read path is
now overflow-safe: `nav::array_stats` uses checked accumulation and falls back to an `f64` reduction
labelled `exact: false` rather than wrapping. This issue is the **write** path, which still wraps.

Refs: #347, #522

