---
type: issue
state: closed
created: 2026-09-29T00:00:42Z
updated: 2026-09-29T03:52:46Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/478
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:27.001Z
---

# [Issue 478]: [feat(cli): emit native npy dtypes so int64 above 2^53 survives `--format npy`](https://github.com/vig-os/tessera/issues/478)

Raised by the review of PR #476 (#387). That PR added `tsra slice|project --format npy`, which always
writes `'<f8'` (little-endian float64). The reviewer's preferred fix — emit a native integer descr such as
`'<i8'` when no `--physical` rescale is in play — **cannot be done at the writer**, so the PR reworded the
fidelity claim precisely instead and this issue tracks the real fix.

## Why relabelling would not work

The whole CLI array path is `f64` well before `write_npy` sees anything:

```rust
let values = region_to_f64(&region, rescale);   // nav.rs — shared with CSV/TSV/JSON and `stats`
```

By then an `int64`/`uint64` magnitude above 2^53 has **already** lost precision, because `f64` has a
53-bit mantissa. Emitting `'<i8'` from those values would produce a file that *claims* int64 fidelity
while carrying rounded values — strictly worse than an honest `'<f8'`, which at least declares the type it
really holds.

## Scope of the actual gap

`f64` is **exact** for every dtype whose values fit its mantissa, which is most of the envelope closed in
#418/#420:

| dtype | exact as `f64`? |
| --- | --- |
| `int8`/`int16`/`int32`, `uint8`/`uint16`/`uint32` | yes |
| `float16`/`float32` | yes |
| `bool` | yes |
| `float64` | yes (identity) |
| **`int64`/`uint64`** | **only below 2^53** |

So the gap is narrow but real, and silent where it bites.

## Fix

Carry `ArrayData` (or a small typed enum) through to the writer rather than converting to `f64` at the
door, and pick the npy descr from the source dtype:

- `int8→'<i1'`, `int16→'<i2'`, `int32→'<i4'`, `int64→'<i8'`, the `uint` equivalents, `float16→'<f2'`,
  `float32→'<f4'`, `float64→'<f8'`, `bool→'|b1'`;
- `--physical` still forces `'<f8'`, because a rescale genuinely produces floats;
- `project --mode mean`/`sum` likewise (`mean` is fractional, `sum` can leave the source range) — the same
  rule `GridDtype::computed_from` already encodes for the JSON `dtype` field.

`region_to_f64` is shared with the CSV/TSV/JSON and `stats` paths, so this is a nav-wide refactor rather
than a local change — which is why it was not folded into #476.

## Acceptance criteria

- [ ] An `int64` array holding `2^53 + 1` round-trips **exactly** through `--format npy` (the value that
      fails today); assert it against the bytes, not against our own reader.
- [ ] Each supported dtype emits its native descr, verified by decoding the header — and `numpy.load`
      compatibility is preserved (the 64-byte header alignment is already asserted).
- [ ] `--physical`, `--mode mean` and `--mode sum` still emit `'<f8'`.
- [ ] The CSV/TSV/JSON paths are unchanged for every dtype that is exact in `f64`, so no existing output
      moves.
- [ ] The `write_npy` doc comment's fidelity caveat is deleted rather than reworded — it should stop being
      true.

## References

- PR #476 · #387 · `tessera/crates/tessera-cli/src/nav.rs` — `write_npy`, `region_to_f64`, `GridDtype`
- #418/#420 (the array dtype envelope this must cover)

