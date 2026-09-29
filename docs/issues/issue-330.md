---
type: issue
state: closed
created: 2026-07-03T12:35:59Z
updated: 2026-09-28T17:55:02Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/330
comments: 1
labels: feature, discussion, priority:high
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:48.492Z
---

# [Issue 330]: [feat(io): nullable table columns (validity bitmap) — NaN/missing is NULL, not a float sentinel](https://github.com/vig-os/tessera/issues/330)

## Context — DUPLET first-user run

GE listmode `events_3p.lt_corr` is **0.7 % NaN** (99.3 % finite, ±17 ns). The NaN means "lifetime
correction not computed for this event" — semantically **NULL / missing**, not a number. Today
tessera's `ColumnData` has no nullable variant:

```rust
pub enum ColumnData { I8(Vec<i8>), I16(Vec<i16>), … F32(Vec<f32>), F64(Vec<f64>) }
```

So a column with any NaN can't be quantized (the int16 transform, #310, skips it) and is forced to
stay `f4` — wasting ~2× on 99.3 % good data to carry 0.7 % NaN, and conflating "missing" with a float
value. Sparse (ADR-0031) doesn't fit either (99.3 % *present*, not mostly-empty).

## The right representation
A **nullable column**: a validity bitmap (1 bit/row — 0.7 % set compresses to ~nothing) + int16 values
for the valid rows. NaN/None → NULL; the finite 99.3 % quantize to int16 (physically lossless, ~2×
smaller, #310). Vortex/Arrow support validity bitmaps natively, so this is a tessera-side exposure gap,
not a backend limitation.

## Ask
1. Add a validity/nullness path to `ColumnData` (either a per-variant `validity: Option<BitVec>` or a
   `Nullable(Box<ColumnData>, BitVec)` wrapper) that maps to Vortex's native null support on
   encode/decode and round-trips through the seal deterministically.
2. Ingest maps `f4`/`f8` **NaN → NULL** (opt-in or by policy), so `lt_corr` becomes nullable int16.
3. `read`/`sql`/export surface NULL (not NaN) so downstream `AVG`/`WHERE` treat missing correctly.

**Interim (no feature):** quantize with a reserved int16 sentinel (e.g. `i16::MIN` = missing) recorded
in column metadata — works today, but a reader must know the sentinel; nullability is the correct fix.

Found during: DUPLET DP01 listmode (`events_3p.lt_corr`, 0.7 % NaN). Relates #310 (quantize), #307
(column annotation), ADR-0031 (sparse — the *other* missing-data axis).
---

# [Comment #1]() by [gerchowl]()

_Posted on September 28, 2026 at 05:55 PM_

Closing as **done** — verified on `origin/dev` in the 2026-09-28 backlog triage.

Evidence: commit dd11576 'feat(io): nullable table columns — a validity mask, not a sentinel (#360)' + 1fe4a50 (streaming path).

https://claude.ai/code/session_01XdERKMVDAwfMJSKdTytNnK

