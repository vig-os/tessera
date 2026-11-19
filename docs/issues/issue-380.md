---
type: issue
state: closed
created: 2026-08-18T10:29:12Z
updated: 2026-11-18T14:55:59Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/380
comments: 2
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-19T09:52:43.514Z
---

# [Issue 380]: [table_block stores f64 columns UNCOMPRESSED — Pco is neither enabled nor registered, so excluding ALP leaves no float scheme](https://github.com/vig-os/tessera/issues/380)

## Summary

`tessera-io::table::encode` produces **byte-for-byte raw output** for f64 columns. Measured on real physics data: **42 209 852 B output for a 42 183 520 B raw payload — +0.06 % overhead, zero compression.**

The same values through `tessera-io::array` with `codec = "pcodec"` come to **15 144 309 B — a genuine 64 % compression.** Identical data, identical dtypes, different block type.

## Cause

`table.rs` builds its compressor as:

```rust
// Exclude the ALP float schemes from the compressor so it never *chooses* them; floats then use
// the deterministic Pco/flat schemes. Integer schemes (chosen by exact integer math) are kept.
let compressor =
    BtrBlocksCompressorBuilder::default().exclude_schemes([ALPScheme.id(), ALPRDScheme.id()]);
```

The comment's premise does not hold. In `vortex-btrblocks` 0.75.0:

1. **`PcoScheme` is not in `ALL_SCHEMES`.** The float section of `builder.rs` is exactly:
   ```rust
   &float::FloatConstantScheme,
   &float::ALPScheme,
   &float::ALPRDScheme,
   &float::FloatDictScheme,
   &float::NullDominatedSparseScheme,
   &float::FloatRLEScheme,
   ```
   `BtrBlocksCompressorBuilder::default()` sets `schemes: ALL_SCHEMES.to_vec()`, so Pco is never a candidate.
2. **`PcoScheme` is `#[cfg(feature = "pco")]`-gated** (`schemes/float/mod.rs`), and tessera declares `vortex-btrblocks = "0.75"` with **no features**, so it is not even compiled.

With ALP and ALPRD excluded, the remaining float schemes are Constant, Dict, NullDominatedSparse and RLE. For a high-cardinality continuous column none applies, and the encoder falls through to flat.

So the exclusion intended to remove *one* float scheme for determinism removes **all** float compression.

## Reproduction

`neutron_capture_xs_05` from strata's physics tables — 2 636 470 rows × 2 f64 columns (`energy_ev`, `xs_barn`), CSR form, no key columns:

| path | bytes | vs raw |
|---|---|---|
| raw f64 | 42 183 520 | — |
| **`table_block`** | **42 209 852** | **+0.06 %** |
| `array_block`, `codec="pcodec"` | 15 144 309 | **−64 %** |
| `array_block`, `codec="zstd"` | 27 754 103 | −34 % |
| parquet zstd-19 (for reference) | 23 827 958 | −44 % |

The data is well suited to a numeric codec: `energy_ev` is monotonically increasing within each of 80 groups and spans 1e-05 … 2e8 — 13 orders of magnitude. Byte-oriented codecs do poorly on raw f64 mantissas; pcodec models the sequence.

Note the table block is currently **worse than storing parquet**, which is the comparison that surfaced this.

## Suggested fix

Enable the feature and register the scheme explicitly:

```toml
vortex-btrblocks = { version = "0.75", features = ["pco"] }
```

```rust
use vortex_btrblocks::schemes::float::PcoScheme;

let compressor = BtrBlocksCompressorBuilder::default()
    .exclude_schemes([ALPScheme.id(), ALPRDScheme.id()])
    .with_new_scheme(&PcoScheme);
```

`with_new_scheme` exists for exactly this ("Adds an external compression scheme not in `ALL_SCHEMES`"). Pco should also preserve the determinism property the ALP exclusion was protecting — worth confirming, since that is the constraint the current code is trying to satisfy.

## Why it matters

Anything storing float columns through the table block is paying full raw size. In our case that is the difference between the table block being unusable for physics data and being competitive with the array path.

Found while benchmarking storage options for strata's 900 MB physics-table set ([private-repo]#1142). Happy to supply the harness or test data.
---

# [Comment #1]() by [gerchowl]()

_Posted on August 18, 2026 at 12:55 PM_

Landed in #384 (squash `bac299b` on `dev`). Vortex table blocks now compress float columns via the registered Pco scheme (`features=["pco"]` + `with_new_scheme(&PcoScheme)` in a shared `deterministic_table_compressor()` helper; ALP stays excluded). Measured: corpus `listmode_events` table 39470→7382 B (−81%); a 200k-row continuous f64 column 0.116× (was raw ~1.0006×) — lands in the array-pcodec region, beats parquet-zstd on floats. Determinism CONFIRMED by the aarch64 CI conformance leg (byte-identical x86==ARM on the regenerated corpus). Regression guarded by `f64_column_is_compressed_by_pco_not_stored_raw`.

---

# [Comment #2]() by [gerchowl]()

_Posted on August 20, 2026 at 12:38 PM_

Confirmed fixed on `dev` (`eeb55a4d`) — thank you for the fast turnaround.

Re-ran the same reproduction. The table block went **42 209 852 → 15 371 432 bytes on identical input**, a **2.75×** improvement, and from *no compression at all* to within **1.5 %** of the array block with pcodec.

| path | before (`999bcd93`) | after (`eeb55a4d`) |
|---|---|---|
| `table_block`, CSR payload | 42 209 852 (+0.06 % vs raw) | **15 371 432** |
| `array_block`, `codec="pcodec"` | 15 144 309 | 15 144 309 |
| parquet zstd-19 (reference) | 23 827 958 | 23 827 958 |

Two findings from the follow-up that may be of interest:

**Multi-block beats a single block, against expectation.** Splitting the same payload into **80 per-isotope table blocks** gives **13 272 588 B** — **13.6 % smaller** than one block, and better than the array path. Each isotope's (energy, xs) curve is internally homogeneous, so a single pooled block asks pcodec to model a mixture of 80 distributions while per-group blocks let it fit each one. It also gives per-group range fetch for free, so it wins on size *and* access.

**Schema separation still matters.** Keeping per-group key columns and a per-row `pair_idx` counter in the same table costs **+27 %** versus a payload-only block with the properties in a separate 80-row block — even with Pco registered.

Neither is a tessera defect; recording them in case they are useful guidance for `table_block` users, since "one big table" is the intuitive layout and measurably not the best one.

