---
type: issue
state: closed
created: 2026-09-28T21:41:23Z
updated: 2026-09-29T02:48:27Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/468
comments: 5
labels: bug, priority:high
assignees: none
milestone: 0.1.0-alpha.2
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:28.249Z
---

# [Issue 468]: [determinism: vortex writes different container bytes under debug_assertions (side-effecting is_sorted assert on patch indices) — debug- vs release-built tools seal differently](https://github.com/vig-os/tessera/issues/468)

**Found by #460's corpus fixtures (#386).** It is a pre-existing `tessera-io` defect, not something #386 introduced.

The same tree and the same inputs seal a different `content_hash` depending on target arch and build profile. Each result was verified on a clean target dir, which rules out stale build artifacts:

| | ingest_parquet_scalars | ingest_parquet_floats |
|---|---|---|
| x86_64 debug | b4fe24 | 2ee5bf |
| x86_64 release | **b26b10** | 2ee5bf |
| aarch64 release | b4fe24 | **33b065** |

Two different fixtures move under two different (arch × opt-level) combinations. Both are the float-bearing fixtures:
- `floats` holds NaN payloads, ±inf, −0.0, MIN_POSITIVE and subnormals.
- `scalars` holds f32 and f64, integer extremes, Date32, Timestamp and Decimal128.

That points to float codegen (FMA and opt-level differences) reaching the encoder's output bytes.

`tessera-io::table::deterministic_table_compressor` already excludes ALP/ALPRD for exactly this reason (their exponent search is sensitive to codegen). So either:
- that exclusion is not effective on these columns, or
- another scheme, or btrblocks' own sample-based cost model (float compression-ratio comparisons), has the same property for short or pathological float columns.

Pco has only been proven cross-arch on normal listmode data.

**Ruled out:** tzdb, LC_ALL, CPU count, SOURCE_DATE_EPOCH, workspaceFeatures/sql, nextest isolation, the nix sandbox, a stale rlib, and float `pow` in our own seal path.

**Plan**
1. Bisect to the exact column, dtype and scheme with a per-column probe run in both profiles.
2. Make scheme selection a function of the data alone. Pin the float scheme, or exclude the remaining float-cost-model paths, or make the cost comparison integer-exact.
3. Add a regression gate that seals the pathological float fixtures under **debug and release** in CI, since Gate A currently runs one profile per arch.

**Constraint:** prefer a fix that leaves every golden already on `dev` unmoved. If an existing `dev` golden must move, that is a format event and needs an explicit owner decision.

#460 cannot go green until this lands, because its corpus would otherwise assert a value that depends on the host.

Refs: #460, #386
---

# [Comment #1]() by [gerchowl]()

_Posted on September 28, 2026 at 09:47 PM_

**Correction from bisection: this is NOT float codegen.** A single-column probe through `tessera_io::table::encode` (no arrow, no ingest code) shows exactly one column differs between debug and release:

- **i64 `[i64::MIN, 0, i64::MAX]`**: debug len=2940, release len=2916. A *different encoding* is chosen.
- All other dtypes are identical in both profiles: i8–u64, f32/f64 (including NaN payload, ±inf, −0.0 and subnormals), bool, text, date, timestamps and decimal. So are the two-element i64 spans and every narrower i64 range.

This explains the x86_64 failure of `ingest_parquet_scalars`, which carries the i64 extremes. **The aarch64 failure of `ingest_parquet_floats` is not explained by it**. It depends on the arch, not the profile, and is tracked separately.

Already ruled out by reading vortex 0.75.0: the sampling RNG uses a fixed seed; the scheme order is a fixed slice; `best_bit_width` is integer-only; `max_minus_min` goes through i128; candidate ratios are single exact int→f64 divisions; and no `debug_assertions` or `catch_unwind` paths exist. The selector compares f64 ratios with a strict `>` (estimate.rs:156), but no codegen-sensitive ratio has been found yet. A TRACE probe of the per-candidate verdicts is next.

---

# [Comment #2]() by [gerchowl]()

_Posted on September 28, 2026 at 09:59 PM_

**Root cause narrowed further. It is not scheme selection.** A TRACE of `vortex_compressor::encode` shows compression decisions that are byte-identical across profiles (zigzag → bitpacking, the same ratios and winner, and the same stats). The 24-byte difference is in **vortex file container/layout metadata** (the first differing byte is at offset 101, a flatbuffer length of 440 vs 416). It correlates with an extra `execute_parent` rewrite (`vortex.cast(u8, len=1) → vortex.primitive`) in the debug executor.

**Trigger:** i64/u64 with at least 3 rows, spanning nearly the full width **and** containing 0 (e.g. `[MIN,0,MAX]`, `[MIN+1,0,MAX]`, `u64 [MAX,0,MAX-1]`). Stable: `[MIN,1,MAX]`, `[MIN,-1,MAX]`, `[0,1,MAX]`.

**Impact:** CI shows aarch64 **release** equals x86 **debug**, which differs from x86 **release**. So the *released* x86 and aarch64 binaries seal this shape differently. That makes it a cross-arch S15 hole for such data. **The dev corpus is profile-stable and nothing on dev is mis-sealed**: gen_corpus is identical in both profiles and matches the committed corpus.json.

**Interim:**
- narrow the #460 fixture;
- add a known-limitation reproducer to a new both-profiles gate, which flips when upstream fixes it;
- write the ADR-0057 §7 note.

The upstream vortex report is being drafted, after a time-boxed hunt for the exact condition (UB, overflow or uninitialised-read hypotheses).

---

# [Comment #3]() by [gerchowl]()

_Posted on September 28, 2026 at 10:23 PM_

**Correction to my earlier comment: there is no cross-arch component.** The discriminator is `debug_assertions` alone (measured by toggling only that knob, in both directions). The cause is a side-effecting `#[cfg(debug_assertions)] assert!(is_sorted(&indices…))` in vortex-array `patches.rs:278`, which runs only for patch-producing inputs. It reproduces with stock vortex APIs and a bare `PrimitiveArray`.

The apparent aarch64 vs x86 split was two different checks with two different profiles: `ingestGateA` runs `cargo run` (dev, debug assertions on), and `workspace-test` runs crane nextest (release, off). **All release builds agree with each other on any arch.** What diverges is a debug-built tool against a release-built one. That is still an S15 violation, since the bytes must be a function of the data for *any* build of a version, but it does not hit two shipped binaries.

#472 (aarch64 `ingest_parquet_floats`) is a **separate, genuine same-profile cross-arch difference** and stays open.

---

# [Comment #4]() by [gerchowl]()

_Posted on September 29, 2026 at 12:26 AM_

Filed upstream: **vortex-data/vortex#10119** (issue) and **vortex-data/vortex#10121** (fix PR).

Root cause confirmed at the code level rather than by elimination: `is_sorted_impl` ends in `cache_is_sorted`, which does `array_stats.set(Stat::IsSorted, …)` — the check mutates the array it inspects, and cached statistics are serialised. A test at 0.75.0 shows `Patches::new` leaving exactly one entry behind (`IsSorted=Exact(Bool(true))`) and nothing else, so that annotation is the whole 24 bytes.

Fix is an `is_sorted_uncached` used for the assert; querying still caches. Re-verified as still present on vortex `develop` before filing, and rebased there (the file had moved to `legacy_session()`).

Carried into tessera meanwhile by the fork pin in #481; #480 tracks removing it. #474's known-limitation test is flipped there into a positive guard — both profiles now agree at the former release numbers (bare 2772, sealed 2916).

Same defect class as #472: a write-time statistic leaking into the serialised bytes.

---

# [Comment #5]() by [gerchowl]()

_Posted on September 29, 2026 at 02:48 AM_

Fixed on dev via the pinned vortex fork (#481). CI guards cover it on both arches. Upstream fixes: vortex-data/vortex#10121 (#468) and #10122 (#472). Removing the pin once upstream releases is tracked in #480.

