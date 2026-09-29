---
type: issue
state: closed
created: 2026-09-29T05:11:15Z
updated: 2026-09-29T05:53:47Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/493
comments: 3
labels: priority:medium
assignees: none
milestone: 0.1.0-beta
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:24.680Z
---

# [Issue 493]: [perf(table): tessera's table encoding is 6.2x larger than HDF5 shuffle+gzip on the bench-compare listmode fixture](https://github.com/vig-os/tessera/issues/493)

Surfaced by the corrected #487 benchmark: on the 4M-row listmode-like fixture (`u64` monotonic counter `t` + `f4 e0` + `f4 e1`, 61 MiB raw), HDF5 with standard shuffle+deflate seals to **155.3 KiB**, while tessera's Vortex table seals to **959.4 KiB**, **6.2x larger**.

A monotonic counter should delta/FoR-encode to almost nothing, and the two floats have a tiny repeating period. So either btrblocks is not picking the right scheme for these columns, or the sealed container overhead (stats, chunk index, per-block layout) dominates at this size.

Investigate per column (what scheme is chosen; compressed bytes per column; the container and stat overhead) and compare against what Pco / delta+bitpacking would achieve.

Constraints:
- Any encoder change is a **format/determinism event**. It must pass the both-profiles gate (#474), keep the scheme selection a function of the data, and must not move existing dev goldens without an owner decision.
- Do not tune to the fixture. Check the same effect on real DUPLET listmode (`/mnt/HDD/…`, see memory) before claiming a win.

The fixture is near-ideal for shuffle, which is the benchmark's own stated caveat. So the real-data comparison decides the priority.

Refs: #487, #388
---

# [Comment #1]() by [gerchowl]()

_Posted on September 29, 2026 at 05:40 AM_

## Investigated: fixture artefact. No encoder change recommended.

On **real DUPLET listmode** tessera is already **1.03× smaller** than HDF5 shuffle+gzip. The 6.2×
gap is a property of the benchmark fixture's float columns, and it *inverts* on real data.

### 1. Where the fixture's bytes go

Each column sealed **alone** through the production encoder (`tessera-io::table::encode`):

| column | raw | sealed alone | ratio |
|---|---|---|---|
| `t` (u64 monotonic) | 30.5 MiB | **14.1 KiB** | 2213× |
| `e0` (f32, period 7) | 15.3 MiB | 574.7 KiB | 27× |
| `e1` (f32, period 5) | 15.3 MiB | 368.1 KiB | 42× |
| all three together | | 954.0 KiB | |
| 1-row container floor | | 2.2 KiB | |

**Container / stats / chunk-index overhead is 0.4% of the file.** It is not the cause. The two float
columns are 942.8 KiB of the 954 KiB, and the integer path is excellent.

### 2. Which scheme, and why gzip wins there

Introspected with the same compressor configuration `encode` uses:

```
t  (u64 sequence) -> root: vortex.sequence(u64, len=4000000) nbytes=0 B (0.00%) [strict]
e0 (f32 period 7) -> root: vortex.pco(f32, len=4000000) nbytes=576.18 kB (100.00%)
```

The integer sequence scheme recognises `t` exactly and stores **zero bytes**. Pco is chosen for the
floats and stores essentially everything.

The reason is the fixture's data, not the codec: `e0`/`e1` have **cardinality 7 and 5** and are
strictly **periodic**. Per column, shuffle+gzip-4 gets `e0` to **21.0 KiB** and `e1` to **19.1 KiB**,
because LZ77's sliding window locks onto the repeating 28-byte / 20-byte block. Pco — and dictionary,
and bit-packing — model the **value distribution**, not **sequence repetition**, so none of them can
exploit periodicity at all.

The mirror image is in the same benchmark: on the sequence column tessera beats shuffle+gzip **5×**
(14.1 KiB vs 70.0 KiB). The fixture is two pathological columns against one ideal one.

### 3. Real DUPLET listmode — the decider

`/events_2p`, first 4M rows of a 94M-row acquisition. Identical bytes fed to both sides (columns
dumped once, then compressed by each). Read-only with respect to the acquisition tree.

| column | raw | shuffle+gzip4 | tessera | tessera vs gzip |
|---|---|---|---|---|
| `en_0` | 15.3 MiB | 11398.4 KiB | 10660.0 KiB | 1.07× |
| `en_1` | 15.3 MiB | 11421.5 KiB | 10694.3 KiB | 1.07× |
| `id_0` | 7.6 MiB | 7263.0 KiB | 7334.3 KiB | 0.99× |
| `id_1` | 7.6 MiB | 7263.0 KiB | 7334.2 KiB | 0.99× |
| `ms` | 15.3 MiB | **49.2 KiB** | 101.0 KiB | **0.49×** |
| `vtx_0/1/2` | 15.3 MiB ea | ~13.4 MiB ea | ~13.0 MiB ea | 1.03× |
| **TOTAL** | **106.8 MiB** | **75.7 MiB** (1.41×) | **73.4 MiB** (1.46×) | **1.03×** |

Real energies and vertices are continuous and noisy — shuffle+gzip achieves only ~1.4× on them, and
Pco beats it. A synthetic continuous f32 column shows the same: shuffle+gzip manages 1.37×.

### 4. The one genuine deficit: `ms`

`ms` is **2× worse** than shuffle+gzip (101.0 KiB vs 49.2 KiB). It is a coarse millisecond clock over
4M events: monotonic, **only 3 distinct deltas, 99.6% of them zero** — long runs of the same value.
That is a run-end / RLE shape.

For completeness, the other candidate in the ask — **delta + bitpacking — would be far worse**
(976.6 KiB): 2 bits/value × 4M beats nothing when 99.6% of the deltas are zero.

This is **52 KiB out of 73.4 MiB = 0.07% of the file**. Against a format/determinism event (the
both-profiles gate, cross-arch goldens, an owner decision on dev goldens) that is not a close call.
Filed separately as a low-priority beta item, to be justified on real data if it is ever taken up.

### Conclusion

Closing as a **fixture artefact, no change**. Follow-ups:

- The run-end evaluation is filed separately (low priority, beta).
- The benchmark will report **both** fixtures — the existing periodic one, labelled as adversarial
  for value-distribution codecs, *and* a continuous listmode-like one — rather than replacing the
  periodic one, so the result is not improved by choosing friendlier data.

Scheme names for the individual real columns are still computing (isolated compression of 8 × 4M-row
columns) and will be added as a comment; the conclusion does not depend on them, since the sizes
already answer the question.

Refs: #487, #388

---

# [Comment #2]() by [gerchowl]()

_Posted on September 29, 2026 at 05:40 AM_

Closing per the investigation above: fixture artefact, no encoder change. On real DUPLET listmode tessera is 1.03x smaller than shuffle+gzip; the 6.2x gap comes from the fixture's periodic, cardinality-5/7 float columns and inverts on real data.

---

# [Comment #3]() by [gerchowl]()

_Posted on September 29, 2026 at 05:53 AM_

### Scheme names per column (the pending item from the report above)

Introspected with the production compressor configuration, root scheme per column:

**#388 fixture**

```
t  (u64 sequence) -> vortex.sequence(u64)  nbytes = 0 B      (0.00%)   [strict]
e0 (f32 period 7) -> vortex.pco(f32)       nbytes = 576.18 kB (100.00%)
```

**Real DUPLET `/events_2p`**

```
en_0  -> vortex.pco(f32)              10.90 MB
en_1  -> vortex.pco(f32)              10.94 MB
id_0  -> fastlanes.bitpacked(u16)      7.50 MB   [min=0, max=29375]
id_1  -> fastlanes.bitpacked(u16)      7.50 MB   [min=0, max=29375]
ms    -> vortex.dict(u32)            119.81 kB
vtx_0 -> vortex.pco(f32)              13.31 MB
vtx_1 -> vortex.pco(f32)              13.34 MB
vtx_2 -> vortex.pco(f32)              13.23 MB
```

Nothing here changes the conclusion, but two details are worth recording for #494:

- **`ms` is dictionary-encoded, not run-end.** Dict is a reasonable pick for 3 distinct values, but it
  spends a code per row and so cannot exploit the *runs* — and 99.6 % of this column's deltas are
  zero. That is why gzip (49.2 KiB) beats it (101.0 KiB sealed): deflate collapses the runs. So the
  follow-up is specifically "is a run-end scheme selectable for this shape", not "add a codec".
- **`id_*` take `fastlanes.bitpacked`**, which is the right call (dense 0..29375 crystal indices) and
  lands within 1 % of shuffle+gzip. No action.

The float columns all take `vortex.pco` as intended after the ALP exclusion (#380), and beat
shuffle+gzip by 3–7 % on real continuous data.

