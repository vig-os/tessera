---
type: issue
state: closed
created: 2026-09-28T21:47:43Z
updated: 2026-09-29T02:48:29Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/472
comments: 3
labels: bug, priority:high
assignees: none
milestone: 0.1.0-alpha.2
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:27.813Z
---

# [Issue 472]: [determinism: ingest_parquet_floats seals differently on aarch64 vs x86_64 in the SAME (debug) profile — genuine cross-arch float divergence](https://github.com/vig-os/tessera/issues/472)

On CI, `nix flake check (aarch64-linux)` computes `content_hash 33b065…` for the #460 fixture `ingest_parquet_floats`, while x86_64 in both debug and release computes `2ee5bf…` (the committed golden). Only that fixture moves on aarch64. It holds NaN `0x7ff8_0000_dead_beef`, ±inf, −0.0, MIN_POSITIVE and the largest subnormal.

This depends on the **arch**, not the build profile, so it can't be reproduced on x86 and is a separate mechanism from #468's i64 debug/release split. On x86, the same f64 values encode identically in both profiles.

Next: bisect per column and per value on an aarch64 runner (workflow_dispatch with a probe), and check NaN-payload canonicalisation and the subnormal/FTZ behaviour of any float→int path in the encoder.

Refs: #468, #460
---

# [Comment #1]() by [gerchowl]()

_Posted on September 28, 2026 at 10:41 PM_

## Mechanism found and confirmed: the persisted `Stat::Sum`, not a codec

It is **one bit** in the whole file — the sign bit of a NaN that Vortex computes and writes as the
column's `Sum` statistic. It is neither Pco nor a scheme, so `deterministic_table_compressor`'s
ALP/ALPRD exclusion cannot address it: that exclusion is aimed at a different layer.

### The chain

1. `Stat::Sum` is in `PRUNING_STATS` (`vortex-array-0.75.0/src/stats/mod.rs`), documented as the
   stats "we want to ensure … are computed when compressing/writing", and
   `StatsSet::write_flatbuffer` (`src/stats/flatbuffers.rs`) serialises it. So the sum is **in the
   file bytes** on every write.

2. `vortex-array-0.75.0/src/aggregate_fn/fns/sum/primitive.rs:63` skips NaNs, then accumulates:

   ```rust
   pub(super) fn sum_float_all<T: NativePType>(acc: &mut f64, slice: &[T]) {
       for &v in slice {
           if !v.is_nan() {
               *acc += ToPrimitive::to_f64(&v).vortex_expect("float to f64");
           }
       }
   }
   ```

3. Because NaNs are skipped, the column's `+inf` and `-inf` meet in the accumulator and perform
   `inf + (-inf)` — an IEEE 754 **invalid** operation. IEEE does not specify the resulting NaN's bit
   pattern, and the two architectures differ:

   | target | `inf + (-inf)` |
   |---|---|
   | x86_64 (SSE) | `0xfff8_0000_0000_0000` — sign bit **set** |
   | aarch64 | `0x7ff8_0000_0000_0000` — sign bit **clear** |

4. That value is written into the stats → the file differs in one bit → different `content_hash`.

The NaN-*skipping* is what exposes this. Without it the accumulator would be NaN from element 1 and
would **propagate that element's payload**, which both architectures do identically (measured below).
Skipping is what lets the two infinities meet and *generate* a fresh default NaN.

### Evidence

Both arches in one workflow matrix, same pinned rustc 1.96.0, debug profile, so the target
architecture is the only variable. Encoding the fixture's column through `table::encode` per
cumulative prefix:

| case | len (both) | verdict |
|---|---|---|
| `control_1_2_3` (ordinary finite) | 2564 | **byte-identical** |
| each of the 7 values alone | 2204 / 2548 | **byte-identical** |
| `prefix_1` … `prefix_4` | 2204–2572 | **byte-identical** |
| `prefix_5` (adds `-inf`) | 2580 | **1 byte, 1 bit**: `ff`→`7f` at offset 223 |
| `prefix_6`, `prefix_7`, full column | 2588 | **1 byte, 1 bit**: `ff`→`7f` at offset 231 |

The differing byte is the top byte of an 8-byte little-endian field:

```
x86_64 : … 00 00 00 00 00 00 f8 ff …   ->  fff8000000000000
aarch64: … 00 00 00 00 00 00 f8 7f …   ->  7ff8000000000000
```

`prefix_4` already contains `+inf`; `prefix_5` is the first to add `-inf`. So the trigger is exactly
"**both** infinities in one column", which is precisely what makes `inf + (-inf)` reachable. The
offset moves 223 → 231 only because the array grew by one 8-byte element.

Direct measurement of the hardware semantics, same build:

```
x86_64   nan_semantics  inf_minus_inf=fff8000000000000  inf_div_inf=fff8000000000000
                        zero_div_zero=fff8000000000000  zero_times_inf=fff8000000000000
aarch64  nan_semantics  inf_minus_inf=7ff8000000000000  inf_div_inf=7ff8000000000000
                        zero_div_zero=7ff8000000000000  zero_times_inf=7ff8000000000000

both     nan_payload    identity=7ff80000deadbeef  plus_zero=7ff80000deadbeef
                        times_one=7ff80000deadbeef  min_with_1=3ff0…  max_with_1=3ff0…
```

So NaN *payload propagation* is identical on both (the fixture's `0xdeadbeef` payload survives
arithmetic either way), and `f64::min`/`max` correctly ignore NaN on both. The only divergence is the
default NaN **generated** by an invalid operation.

### Why this explains everything that made it look strange

- **Profile-independent**, unlike #468: the accumulation is a scalar `+=`, so no vectorisation is
  involved and there is no `[profile.dev.package."*"]` opt-level override in the workspace. This is
  pure hardware semantics, which is why x86_64 debug and release agree and both differ from aarch64.
- **Unreproducible on x86** by construction — a single machine can never show it. Note, though, that
  it *is* diagnosable on x86 alone: the encoded bytes contain exactly one `fff8000000000000`, a value
  that appears nowhere in the input data. That was the tell, before any ARM runner was involved.
- **Only this fixture moves**: it is the only one carrying both infinities.

### Not the same bug as #468

#468's `ingest_parquet_scalars` moves by **profile** (the side-effecting `is_sorted` assert on patch
indices). This one moves by **arch**, in a different layer. Both are real; neither fix covers the
other.

### Next

Not proposing a fix in this comment, per the agreed order. The upstream vortex issue is drafted and
held, with three candidate fixes — the cheapest being to canonicalise a NaN sum before persisting it
(the value *is* "not a number", so a platform-chosen payload carries no information). A tessera-side
mitigation is also possible without waiting for upstream. Both need an owner decision, and no golden
on `dev` should move without one.

Probe and the throwaway dual-arch workflow are on `spike/472-float-cross-arch-probe` (not for merge).

Refs #468, #460

---

# [Comment #2]() by [gerchowl]()

_Posted on September 29, 2026 at 12:26 AM_

Filed upstream: **vortex-data/vortex#10120** (issue) and **vortex-data/vortex#10122** (fix PR).

Fix is to report any NaN sum as the canonical quiet NaN. On `develop` this needed **two** sites, not one — `sum` and the newer `sum_v2` each finalise floats through their own path — so the rebase widened the change. A useful precedent: `sum_v2` already writes `f64::NAN` explicitly on its non-`skip_nans` poisoning path, so canonicalising makes the *computed* NaN agree with the one it already hardcodes.

Verified end to end under the pin in #481: all 21 byte-carrying probe lines are byte-identical across x86_64 and aarch64, covering this issue's float shapes and #468's int shapes. The raw hardware NaN probe still differs per arch, as it should — only the persisted statistic is normalised.

On the ingest lane (tsra-386's run), `ingest_parquet_floats` moves `2ee5bf4d1b33 → 33b065510f5e`, which is exactly the value aarch64 was already producing. So the fix brings x86 into agreement with ARM rather than inventing a third value. Every other ingest fixture is unchanged.

#480 tracks removing the pin once released.

---

# [Comment #3]() by [gerchowl]()

_Posted on September 29, 2026 at 02:48 AM_

Fixed on dev via the pinned vortex fork (#481). CI guards cover it on both arches. Upstream fixes: vortex-data/vortex#10121 (#468) and #10122 (#472). Removing the pin once upstream releases is tracked in #480.

