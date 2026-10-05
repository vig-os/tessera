# ADR-0059 — The chunk-index statistics schema: one format event

**Status:** Proposed (2026-09-29, spike for [#522](https://github.com/vig-os/tessera/issues/522) /
[#347](https://github.com/vig-os/tessera/issues/347) P1). Extends **ADR-0028** §3 (the extensible
`{hash, stats}` factory) and **ADR-0027** (sub-block Merkle / the `.cidx` block). Bounded by
**ADR-0057** §5 (feature-graph determinism), **ADR-0020** (identity/determinism) and **ADR-0042**
(sealed vs `aux/`). Consumes [#523](https://github.com/vig-os/tessera/issues/523) (the `sum_sq`
overflow) and, if the owner includes it, [#526](https://github.com/vig-os/tessera/issues/526)
(hash-algorithm agility).

> Design-only. **No sealed-layout code is written until the owner ratifies this document.** Every
> size and cost number below was measured in the nix devShell against the real workspace on a
> 127,664,128-voxel `int16` volume (487×512×512, 64³ chunks, 151 MiB sealed) — the shape #347 is
> about. The overflow bounds are computed, not estimated.

## Why this is *one* event and not five

Attaching a `.cidx` adds a block digest, and `content_hash = merkle_root(&digests)` over every block
ref (`product.rs:177`). So turning the sidecar on by default moves `content_hash` **and**
`manifest_hash` for every integer array product: a corpus regeneration. Changing `ChunkStats`'s
fields changes the `.cidx` payload bytes, which moves the same hashes again.

Today both are free — **no product in `tessera/corpus/` carries a `.cidx` at all**, and the sidecar is
opt-in by construction (`stream.rs:56-58`, `chunk_index.rs:8-10`). After the first default-on seal,
each further schema change is another regeneration. **Everything that wants to be in `ChunkStats`
should land in the same event**, which is what this ADR is for.

## §0 — The decision, in one place

| | in the event | |
|---|---|---|
| **M1** | Bind the index to its block's **content digest** | mandatory (§7) |
| **M2** | Fail closed on accumulator overflow; never wrap | mandatory (§6, #523) |
| **M3** | Define the per-chunk digest over **native element bytes**, not the `i64` gather buffer | mandatory (§7a) |
| **S1** | Keep `count / min / max / sum / sum_sq` | unchanged |
| **S2** | Add `nan / pos_inf / neg_inf / masked` counts | recommended (§2) |
| **S3** | Add **one block-level histogram**, integer counts | recommended (§3–§5) |
| **S4** | Do **not** add `sum³` / `sum⁴` | recommended against (§2) |
| **S5** | Derive quantiles, skew, kurtosis, outlier fences from S3 at read time | recommended (§3, §4) |
| **C1** | Hash-algorithm agility (blake3 default + optional sha256 fixity) | **separable candidate** (§9) |

## §1 — The constraint that decides most of this: determinism

ADR-0057 §5 requires sealed bytes to be a pure function of the input. For a statistic that is folded
per chunk and rolled up, that means the merge must not depend on **worker count or merge order**.

Two strengths of "order-independent" matter here, and the difference is not pedantic:

- **Order-independent by convention.** `f64` addition is not associative, so an `f64` accumulator is
  reproducible only while the fold order is fixed. It is one parallelisation away from drifting.
- **Order-independent by algebra.** Integer addition is associative *and commutative*: counts can be
  merged in any order, by any number of workers, and give identical bytes. Nothing a future
  optimisation does can break it.

There is precedent for the weaker form failing in this repo: **#472** found a cross-architecture seal
divergence caused by a *persisted float* statistic (a `Stat::Sum` NaN, where `inf + -inf` produced
`fff8…` on x86 and `7ff8…` on ARM — one bit, in sealed bytes).

> **Rule adopted here: the `.cidx` stores integers only.** Every sealed statistic is an integer
> counter or an integer sum. Everything real-valued — mean, variance, std, skewness, kurtosis,
> quantiles — is **derived at read time** from those integers. This makes the sealed bytes
> order-independent by algebra and removes float NaN/±0.0/rounding from the format entirely.

It also rules out a family of otherwise-attractive sketches; see §3.

## §2 — Exact mergeable scalars

**Keep** `count`, `min`, `max`, `sum`, `sum_sq`. All exact integer monoids; `min`/`max` are also the
`k = 1` case of §4's extremes.

**Add `nan`, `pos_inf`, `neg_inf`, `masked`** (u64 counts). Exact, trivially mergeable, and they
answer a question a viewer genuinely has ("is this volume clean?") that no other field can. They cost
4 integers per chunk. They are also the honest way to admit float data exists without sealing a float.

**Do NOT add `sum³`/`sum⁴` for skewness and kurtosis.** They do not fit, and no accumulator width
fixes it. With `M` the dtype's maximum magnitude, the largest sample count `n` before `Σxᵖ` overflows:

| dtype | `Σx²` | `Σx³` | `Σx⁴` |
|---|---|---|---|
| `int16` | 1.6e29 | 4.8e24 | 1.5e20 |
| `int32` | 3.7e19 | 1.7e10 | **7** |
| `int64` | **1** | **0** | **0** |

`int32` kurtosis overflows `i128` at the **eighth sample**. Widening to `i256` does not rescue it:
`int64 Σx⁴` still overflows at n = 7. A statistic that works only for ≤16-bit dtypes, and silently
corrupts at n = 8 for `int32`, is not a statistic — it is #523 with a new name. The histogram (§3)
gives skewness and kurtosis for every dtype, with bounded error and no overflow, because its
accumulators are small counts.

## §3 — Quantiles: the candidates, judged on determinism first

Quantiles cannot be exact from summaries alone in general. The question is which *approximate*
structure is **mergeable and deterministic**.

| candidate | deterministic? | verdict |
|---|---|---|
| **Fixed-bin histogram** (linear bins over a fixed range) | **yes, by algebra** — counts merge commutatively | **recommended** |
| **DDSketch / log-buckets** | yes — bucket index is `⌈log_γ x⌉`, an integer function of the value | keep for floats (§8) |
| **HDR histogram** | yes — fixed precision per magnitude band | subsumed by the above |
| **t-digest** | **no** — centroid merging depends on insertion order | **rejected** |
| **KLL / reservoir sampling** | **no** — randomised; even seeded, the merge is order-sensitive and the seed becomes part of the format | **rejected** |

t-digest and KLL are rejected on the §1 constraint alone, before accuracy is even discussed: under
them the sealed bytes would depend on how many workers happened to run.

**Error guarantee of the recommendation.** A quantile read from an equal-width histogram lies in a
known bin, so the absolute error is **at most `width`** — one number, not "the widest bin" — with no
probabilistic caveat. And in the
case that matters most it is not approximate at all:

> On the measured CT volume the values span exactly **4096 distinct levels** (a 12-bit
> reconstruction: range −1024..3071, span 4096, and 4096 distinct values actually present). With one
> bin per value the histogram is a **complete description of the data's distribution**, so quantiles,
> median, mode and fences are **exact**, not estimated.

That is the common case for medical integer arrays, and it is why a fixed-bin histogram beats a
cleverer sketch here: for ≤16-bit data, "approximate quantiles" is a problem we do not actually have.

## §4 — Outliers, decode-free

Everything below is **derived from §3's histogram at read time and costs no extra bytes**:

- **Tukey fences.** `Q1 − 1.5·IQR` and `Q3 + 1.5·IQR` from the histogram's quartiles, plus the count
  beyond each fence (a partial sum over bins).
- **k·σ counts.** `mean` and `std` are already exact from the scalars; the count beyond `mean ± kσ`
  is again a partial sum over bins.
- **Mode and the distribution's shape**, including skewness and kurtosis, as bin-weighted moments —
  which is what replaces S4.

**Optional, if operators ask: exact top-k extremes per chunk.** Top-k *is* a proper mergeable monoid
(merge two sorted k-lists, keep k; ties broken by value, so it is deterministic) and gives exact
extreme *values* rather than a bin. Cost `k × 8` bytes per chunk — for k = 8 over 512 chunks ≈ 4 KB.
**Not recommended for this event**: `min`/`max` already answer "what is the most extreme value", and
the histogram answers "how many are out there". Left specified so it can be added later without
redesign.

## §5 — Bin-edge policy, and where the histogram lives

**Per-chunk histograms are the wrong shape.** Measured, in the `.cidx`'s actual encoding (compact
JSON), against a baseline of **85,373 B for 512 chunks = 167 B per entry**:

| histogram placement | bins | added | resulting `.cidx` | vs today | % of a 151 MiB file |
|---|---|---|---|---|---|
| per chunk | 256 | +597 B/chunk | 382 KiB | 4.6× | 0.25 % |
| per chunk | 1024 | +2245 B/chunk | 1206 KiB | 14.5× | 0.78 % |
| **block level** | 1024 | +2.8 KiB total | 86 KiB | **1.03×** | 0.056 % |
| **block level** | 4096 | +10.5 KiB total | 94 KiB | **1.13×** | 0.061 % |
| block level | 65536 | +137 KiB total | 221 KiB | 2.65× | 0.143 % |

**Decision: one histogram per block, not per chunk.** It is what #347's consumer actually needs (a
whole-array histogram), it keeps the sidecar at ~0.06 % of the file, and a per-chunk histogram buys
only sub-region histograms, which nothing asks for. Pruning is already served exactly by per-chunk
`min`/`max`. The block histogram is still *computed* as the monoid roll-up of per-chunk folds — the
fold is the same, only the persistence differs.

**Edges, for integer arrays — STRICTLY EQUAL WIDTHS** (owner decision, 2026-10-05):

```
span  = hi − lo + 1                  (in i128: a full i64 range is 2^64, which i64 cannot hold)
width = ceil(span / MAX_BINS)        (at least 1)
bins  = ceil(span / width)           (≤ MAX_BINS)
MAX_BINS = 4096 — NORMATIVE for kind "equal-width-int" (~10.5 KiB dense, 1.13× the sidecar)

bin(v) = (clamp(v, lo, hi) − lo) / width            (exact integer division)
bin b  covers [lo + b·width, lo + (b+1)·width − 1]  (computed in i128)
```

**`width` is derived first and `bins` second.** That ordering is the whole decision: every bin is
then exactly `width` wide and only the *count* of bins absorbs the remainder. Fixing
`bins = MAX_BINS` and deriving a width is what produces bins of differing widths.

**`MAX_BINS = 4096` is normative, not a default** (owner decision, 2026-10-05). A reader recomputes
`width`/`bins` from `lo`/`hi` with this exact value and rejects an index that disagrees, so the budget
is part of what `kind: "equal-width-int"` *means*. A different budget is a different layout under the
same declared kind — two valid-looking indexes that cannot be compared — so it requires a **new kind
name**. In code the constant lives beside `HistKind` as `EQUAL_WIDTH_INT_BINS`, not as a parameter;
that also makes the stored field widths provably safe, since `width ≤ ceil(2^64 / 4096) = 2^52` fits
`u64` and `bins ≤ 4096` fits `u32` for every possible `lo`/`hi`. (A configurable budget could produce
`width = 0` or a `bins` past `u32`, which an earlier revision silently truncated.)

**The binned range is widened, so the last bin may reach past `hi`.** For `[0, 12288]` (span 12289)
the layout is `width = 4`, `bins = 3073`, and the top bin covers `12288..12291` — three values
beyond anything observed. This is why **the observed range is stored separately from the bin
geometry**: `lo`/`hi` are the extremes actually seen, `width`/`bins` are the layout, and conflating
them is exactly what makes an edge unreconstructible. A reader needs `lo`, `width` and `bins` to
recover every edge, and `hi` to know where the real data stopped.

**Everything is integer arithmetic, in `i128`.** No float math appears anywhere in the layout or the
lookup, so there is no rounding mode to agree on. `i128` is not belt-and-braces: `hi − lo + 1` for
the full `i64` range is `2^64`, and `lo + bins·width` passes `i64::MAX` near the top of the range —
`Histogram::new(i64::MIN, i64::MAX, 4096)` gives `width = 2^52`, `bins = 4096`, and a top edge of
exactly `i64::MAX`. Both are pinned by test.

**What this costs, stated plainly — and the worst case is not small.** Equal widths mean `bins` is
usually *below* `MAX_BINS`. The worst case is a span of **4097**: `width = 2` forces
`bins = 2049`, which is **barely half** the resolution a near-equal layout would give for the same
byte budget. (The example above, span 12289, gives 3073 bins — three quarters.) Resolution is lost
whenever the span is just past a multiple of the budget. In exchange, a quantile's error bound is a single number
(`width`) instead of "the widest bin", and an implementer cannot get the edges wrong. The case that
matters most is unaffected: when `span ≤ MAX_BINS` the layout is `width = 1`, one bin per value, and
the histogram is a **complete** description of the distribution — quantiles off it are exact, not
estimated.

**`exact` is derived, not stored.** It is exactly `width == 1`. A stored boolean that is a pure
function of a stored integer is a field that can disagree with itself, and one more thing a reader
must validate rather than compute; `Histogram::exact()` computes it, and the `spec` descriptor does
not carry it either.

Recorded in the `.cidx` block `spec`, so a reader is never guessing:

```json
"hist": { "kind": "equal-width-int", "lo": -1024, "hi": 3071, "width": 1, "bins": 4096 }
```

`kind` names the equal-width rule and is what makes §8's float story additive later. `bins` is stored
explicitly as a cross-check on `counts.len()`, which the reader validates. **There is no `exact`
field** (owner decision, 2026-10-05): it is exactly `width == 1`, so `inspect` computes it — a
descriptor field that merely restates another field is one more thing that can disagree with itself.
Counts are stored **dense** — at 4096 bins a dense array is ~8.2 KB against ~11.5 KB for a
sparse map at the same density, and dense has no key-ordering question to get wrong. (Both figures
are for a per-chunk histogram of a few thousand samples; the block-level histogram quoted above at
~10.5 KiB holds larger counts, hence more digits per entry — the §5 table is the one to read for the
sidecar's actual cost.)

**Two copies, one authority.** The `hist` descriptor above duplicates edges that the `.cidx`
**payload** also carries, because `counts` is meaningless without them and a content-hashed block must
be interpretable without its manifest. The rule: **the payload is authoritative**; the `spec`
descriptor exists so `inspect` can report a histogram's shape without reading the block. A writer
derives both from the same value in one place, so they cannot diverge when written, and a hand-edited
`spec` fails **`manifest_hash`** — note: *not* `content_hash`, which is the Merkle root over block
digests only and so is blind to a spec edit. A reader that nonetheless finds them disagreeing must
treat the index as corrupt and fall back as if it were absent — never prefer one silently.

**This rule is enforced by the reader in C5**, together with the rest of the index's validation: the
payload is rejected (and the index treated as absent) when `lo > hi`, when `counts` is empty, or when
a stored `exact` disagrees with `bins == span` — `exact` is **recomputed**, never trusted as written.
`ChunkIndex::from_bytes` itself is a deserializer and validates none of this, so an unvalidated caller
would inherit whatever the bytes claimed.

**The two-level ordering is real but already paid for.** Fixing edges from the observed range needs
the global `min`/`max` first, i.e. a second pass over the data. The write path *already* makes a full
pass to compute per-chunk stats, so this is pass 2 of 2, and §6 measures it. (The alternative —
binning over the *dtype* domain, which needs no prepass — was rejected: for `int16` that is 65536 bins
at 2.65× the sidecar, and it wastes most of them on values a CT recon never contains.)

## §6 — Cost, measured

On the 127.7 M-voxel `int16` volume, release build:

| phase | time | vs encode |
|---|---|---|
| `array_block` (pcodec encode) | 280 ms | 1.00× |
| `array_chunk_index` (today's stats fold) | 1459 ms | 5.21× |
| histogram fold (1024 bins, 2nd pass) | 290 ms | 1.03× |

Two things follow, and the first is a surprise worth flagging on its own:

1. **The index fold already costs 5× the encode.** It walks the chunk grid with an odometer, copies
   each chunk into a `Vec<i64>`, and blake3-hashes it. That is the write-path cost to watch, not the
   histogram.
2. **The histogram adds ~20 % to the index build** (290 ms on 1459 ms), or ~1× the encode. For a
   statistic that turns a 0.58 s full decode into a 0.006 s lookup (#347, measured 95×), that is a
   good trade — but it should be a **writer opt-out**, because it is not free.

Size cost, from §5: **+10.5 KiB per block** at the recommended 4096 bins — the sidecar goes from
0.056 % to 0.061 % of the file.

## §7 — M1 (mandatory): bind the index to its block's content

The `.cidx` records the **name** of the block it indexes, never its digest. So an index cannot be
proven to describe the current bytes of that block, and the failure is silent and confident:

```
tessera commit --remove-block volume --add-block other.tsra:volume
```

left `volume.cidx` in place, and `tessera stats` then served the **old** volume's min/max/mean for the
**new** block, labelled `exact: true`. (#524 closed that specific path — `commit` now removes the
sidecar with its block, and the reader checks dtype, chunk-grid and voxel-count compatibility — but
those are guards around a missing binding, not the binding.)

**Add to the `.cidx` spec:**

```json
"indexes": "volume",
"indexed_digest": "blake3:…"     // the data block's BlockRef.digest at index time
```

A reader trusts the index only when `indexed_digest == bref.digest` for the block it claims to
describe. Cost: ~80 bytes per sidecar, once. This closes the whole class — stale, swapped,
regenerated, or copied-from-another-product — rather than the one path #524 patched. **Mandatory, and
it must land in this event**, because after default-on there will be indexes in the wild that cannot
be validated retroactively.

## §7a — M3 (mandatory): the per-chunk digest is over native element bytes

Today `array_chunk_index` hashes a **derived** buffer: every element is widened to `i64` and written
little-endian, so an `int16` chunk is hashed as 8 bytes per voxel — four times the necessary hashing
work, over a representation that is not the data's own.

**This is a now-or-never change.** The per-chunk digest feeds the index root, so redefining it moves
every `.cidx`. Today that costs nothing (**no product carries one**); after this event's default-on
seal it would be a second corpus regeneration and a break for every index already written. It
therefore rides this event or it never happens.

**Definition.** The digest of a chunk is `blake3` over the concatenation of its elements, in **C-order
with the last axis varying fastest** and edge chunks clipped to the array bounds (unchanged — this is
the existing odometer order), where each element is encoded in a **fixed little-endian** width
determined by the block's dtype:

| dtype | encoding |
|---|---|
| `int8` / `uint8` | 1 byte |
| `bool` | 1 byte, `0x00` or `0x01` — never the host's `bool` representation |
| `int16` / `uint16` | 2 bytes, little-endian |
| `int32` / `uint32` | 4 bytes, little-endian |
| `int64` / `uint64` | 8 bytes, little-endian |
| floats | no index today (§8); when added, IEEE-754 little-endian with a canonicalised NaN |

Little-endian is fixed **by the format, not by the host**: a big-endian machine must byte-swap on the
way into the hasher, not `memcpy`. That is the only way the digest is a function of the data rather
than of the architecture, and #472 is the standing reminder that this class of assumption gets checked,
not asserted — so C3's determinism tests prove it **cross-architecture (CI runs x86_64 and aarch64)
and cross-profile (dev vs release)**, alongside the worker-count invariance.

Only the per-element *encoding* changes. Chunk order, element order and clipping are as they were.

## §8 — Floats

Float arrays get no index today (`as_i64` returns `None`), which is why §1's integer-only rule costs
nothing yet. When they need one, the additive path is: `kind: "log-bucket"` with a DDSketch-style
integer bucket index `⌈log_γ |x|⌉`, separate bucket arrays for negatives and a zero counter, and the
relative-error guarantee `α = (γ−1)/(γ+1)` stated in the spec. Deterministic (integer indices,
commutative merge) and needs no prepass, which is what recommends it for unbounded dynamic range.
Deferred — it is not needed for the integer core, and `kind` leaves the door open.

## §9 — C1 (separable candidate): hash-algorithm agility — #526

**This section is independently include/exclude.** Nothing else in this ADR depends on it.

**Finding.** The digest is hard-wired to blake3: `format!("blake3:{}", …)` in `hash.rs` (5 sites) and
**109 `"blake3:` occurrences** across the crates. There is no `hash_alg` field and no secondary
fixity. BagIt and OCFL consumers expect sha256/sha512.

**Owner's direction, reflected here:** blake3 stays the default identity hash (its parallel tree over
the per-block Merkle is the point); sha256 is held *in parallel at the top level*, computed in the
same single streaming seal pass, with an explicit `hash_alg` label.

**What the sha256 must cover.** It has to be reproducible by a verifier holding only the file — and a
file cannot hash itself, so "sha256 of the sealed product" is circular. Two non-circular options:

1. **Per-block sha256 co-digests** — a sha256 beside each block's blake3. This is what BagIt actually
   consumes (`manifest-sha256.txt` is *per file*), so it is the one that makes #527 an export rather
   than a re-hash. Cost: 64 hex chars × blocks.
2. **One top-level sha256 over the concatenated block payloads in manifest order.** A single fixity
   value, reproducible from the file alone, ~80 bytes.

**Recommendation: both, and both optional.** (1) is what preservation tooling wants; (2) is the cheap
single-value check. They come from one pass, so the marginal cost of having both is zero.

**Identity impact — the line that decides "purely additive" vs "breaking":**

- If the sha256 values are a **side field** not folded into `content_hash`/`id`: purely additive,
  old readers ignore them, no identity derivation changes. **Recommended.**
- If they were folded into `content_hash`: identity derivation changes for every product, and every
  existing id/hash in every manifest, corpus golden and published artifact moves. **Not recommended.**

**Measured cost** (512 MiB, page-cache warm, single thread):

| | throughput | 151 MiB product |
|---|---|---|
| blake3 alone (`digest_reader`) | 2930 MB/s | 54 ms |
| sha256 alone | 1305 MB/s | 121 ms |
| dual, one thread (both hashers per buffer) | ~900 MB/s | ~176 ms |
| dual, sha256 on a second thread | ~1305 MB/s (bounded by the slower) | ~121 ms |

So dual-hashing costs **3.2× blake3's time serially, or 2.25× with the second hasher on its own
thread** — in absolute terms, ~120 ms on a 151 MiB product. `sha2` is **already in the dependency
graph** (two versions, transitively), so this adds no new dependency.

**Gate B.** `sha2` becomes a direct dependency, and ADR-0057 §5 defines that gate's crate list as
*everything that can move sealed bytes* — which a sha256 co-digest in the manifest does whenever the
flag is on. So `sha2` joins the list (a 12th feature snapshot) in the same commit, rather than being
discovered as a ~60-minute CI failure later.

**Default: off.** On request (`--fixity sha256`) or implied by BagIt/OCFL export (#527). Turning it on
by default would pay 2.25–3.2× on every seal for a value most users never read.

## §10 — What the event does NOT include

- Per-chunk histograms (§5) — measured 4.6–14.5× the sidecar for no consumer.
- `sum³`/`sum⁴` (§2) — structurally broken above 16-bit.
- t-digest / KLL quantiles (§3) — non-deterministic merges.
- Float chunk indexes (§8) — deferred behind `kind`.
- Folding sha256 into `content_hash` (§9) — would move every existing identity.

## §11 — Landing plan (after ratification)

1. `tessera-core`: extend `ChunkStats` with S2 counts + the block histogram type; `Monoid::combine`
   fail-closed per M2; `spec` gains `indexed_digest` per M1; the per-chunk digest is redefined per M3.
   Bump `recipe` `chunk_index@1` → `@2`; a missing field reads as **absent**, never as zero (the
   `sum_sq` `#[serde(default)]` precedent — "absent" and "all-zero bins" must not be confusable).
2. `tessera-io`: two-pass build (scalars → edges → histogram), writer opt-out for the histogram.
3. Reader (C5): verify `indexed_digest` and the `recipe` (`@1` must not be read as `@2`); serve
   quantiles/fences from the histogram; keep #524's `exact`/`method` contract and extend it with the
   histogram's `exact` flag. **Validate the payload rather than trusting it** — `ChunkIndex::from_bytes`
   is a deserializer and checks none of this. Reject the index (treating it as absent) when `lo > hi`,
   when `counts` is empty, when `counts.len()` disagrees with the recorded `bins`, when `width` is 0,
   or when `width`/`bins` do not match the §5 layout recomputed from `lo`/`hi` with the **normative**
   4096-bin budget; **`exact` is derived
   from `width == 1`, never read from the wire**. The
   payload-versus-`spec` disagreement rule of §5 is enforced here too: disagree ⇒ corrupt ⇒ fall back.
4. Flip the default on for integer array blocks (#347 P1) **and regenerate the corpus in the same
   PR**, with the golden movement stated as the declared format event.
5. `#526`/`#527` only if §9 is included.

**One PR, one corpus regeneration, one changelog entry.**
