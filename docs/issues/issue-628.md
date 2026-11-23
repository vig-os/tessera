---
type: issue
state: open
created: 2026-10-18T22:23:57Z
updated: 2026-11-22T14:18:10Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/628
comments: 6
labels: feature, question, priority:medium, effort:large, area:core, area:io, area:integrity, adr
assignees: none
milestone: none
projects: none
parent: 778
children: none
synced: 2026-11-23T10:16:34.732Z
---

# [Issue 628]: [perf(format): string-heavy tables are ~3x larger than Parquet — normalize forbids Vortex's zstd compact encoding](https://github.com/vig-os/tessera/issues/628)

**Observed** on an event table, one of whose columns holds JSON text (most of the bytes), at commit `e771a9c`:
- **Size:** about 348 B/row in a sealed product, against about 103 B/row for Parquet (zstd-3) and about 97 B/row for bare Vortex with its compact preset.
- **Where the bytes are:** the JSON column is 41.7 MB of a 47.8 MB table block, encoded with FSST only.
- **Why:** Vortex's compact mode (zstd for strings) exists behind its `zstd` feature, but tessera's session refuses it: `normalize forbids encoding (vortex.zstd)`.

**Ask:** decide whether `vortex.zstd` (or the compact preset) may be part of the normalized encoding set. If it isn't for format-stability reasons, document the size cost for string-heavy tables, and say whether a future format version will allow it.

**Related: whole-block reads.** `Reader::read_block` returns the whole block as `Vec<u8>`, so a projected or filtered query over object storage fetches the entire block (about 812 KiB per GET at about 2k rows, versus 0–98 KiB for Parquet with row groups and page indexes). Range reads per row group or column through `object_store` (#291) would close most of the query-latency gap on large products.
---

# [Comment #1]() by [gerchowl]()

_Posted on October 18, 2026 at 10:51 PM_

Triage (architect): this is a **format decision, not a fix**, so it goes through an ADR before any code.

- **Encodings:** the normalized encoding set is part of the determinism contract: ADR-0056/0057 Gate A/B, and the ALP exclusion and
  `Stat::Sum` lessons in #468/#472. Admitting `vortex.zstd` (or the compact preset) needs: cross-arch and cross-feature byte
  determinism evidence for zstd inside Vortex (level, dictionary, threading), the effect on sealed bytes and corpus goldens, wasm
  reader support (zstd-sys is a known wasm blocker, #224), and whether it is per-column opt-in (string-heavy columns) or a default.
  Until that ADR lands, the size cost for string-heavy tables should be documented in FEATURE-MATRIX / why-tessera.
- **Whole-block reads:** the `Reader::read_block` → `Vec<u8>` part is a separate concern (range reads inside a table block over
  object storage). Please split it into its own issue. It relates to #225's cloud range reads and #347 (chunk-index pruning).

Queued behind the adoption release (0.1.0-alpha.2) and the P0 items (#576, #587, #573).


---

# [Comment #2]() by [gerchowl]()

_Posted on October 25, 2026 at 09:11 AM_

**Triage update (architect, 2026-10-25): still a format decision.** Added priority:medium and effort:large.

Plan, in order:
1. **A determinism spike, run as [batch-host] jobs.** Is Vortex's zstd string encoding byte-identical across x86_64/aarch64, across feature sets (Gate B), and across dev/release profiles? Also measure the size win on the consumer's string-heavy tables against Parquet. Repeat for an FSST-only or dictionary-only option, which needs no zstd in the reader and stays WASM-friendly.
2. **An ADR** that admits (or rejects) the encoding into the normalised set, with the reader/WASM consequences.
3. Implementation, with goldens and a size ratchet (#637).

Order: the spike is queued in the design wave after #632/#655. It's not started yet.


---

# [Comment #3]() by [gerchowl]()

_Posted on November 8, 2026 at 11:44 AM_

**Consumer data point for the format decision (epic #778, scitadel-hub).** The corpus is almost entirely strings: titles, abstracts, author and affiliation names, and US patent full text (claims and description).

- **Sizes (estimates):** about 8×10⁶ patents at 50–100 GB of raw text, and OpenAlex works up to about 4.8×10⁸ rows (the quarterly snapshot is about 770 GB as zstd Parquet).
- **What that means:** at the observed ≈3.4× of Parquet (348 vs 103 B/row), the full OpenAlex works set would seal at roughly 2.5 TB instead of about 770 GB. A topic subset of 10⁷ rows would be about 3× its Parquet size too. That makes this the biggest risk for the use case and the reason it sits in W0 of #778.
- **For the spike:** please include a long-prose column, abstracts at 150–300 tokens and patent descriptions at 5–50k tokens, next to the JSON column. FSST alone does well on short repetitive strings and much worse on long prose, so the result may differ.
- **Range reads:** the "whole-block reads" note matters here too. The corpus will live in an S3-compatible object store (versitygw, [fleet-config-repo]#1121), and queries project a few columns out of wide rows.

---

# [Comment #4]() by [gerchowl]()

_Posted on November 9, 2026 at 03:33 PM_

Re-checked against vortex 0.93.0 (PR #788, #635): the vortex-side blocker is gone. `vortex.zstd` is in the `core2025.06` edition, so it is admissible under the table edition tessera now enables (`TABLE_EDITION = core2026.08.3`). #788 deliberately keeps the 0.75 codec set (no zstd compiled in, compact preset with ALP/ALPRD, integer Pco, Delta and OnPair excluded), so whether string columns get zstd is now **tessera's choice**: a codec-set decision, and a format event of its own (it moves table bytes), not a vortex limitation.


---

# [Comment #5]() by [gerchowl]()

_Posted on November 16, 2026 at 12:35 AM_

**Split check (agent, from the kit storage review [private-repo]#97, 2026-11-16).** The first triage comment asked for the whole-block-read part of this issue to be split out. A new issue was about to be filed for it, but the tracker already covers it, so none was opened. Nothing links those issues back here yet:

| Part of the whole-block-read concern | Tracked in |
|---|---|
| `Reader::read_block` reads and re-hashes the whole block for every column; 4.6–5× the bytes of vendor HDF5 on column and time-range tasks | #823 |
| Projection, filter pushdown, chunk pruning and byte-range reads inside a table block, local first | #654, #666 (ADR-0062 PR1) |
| The same over object storage | #668 (ADR-0062 PR3) |
| Remote open, verify and projection over HTTP/S3 | #777 |
| Why a verified read still fetches whole blocks: digests exist only per block | #670, #672 |

The data point from this issue that those do not carry: a projected or filtered query over object storage fetched about 812 KiB per GET at about 2k rows, against 0–98 KiB for Parquet with row groups and page indexes. It is copied to #823.

If the architect agrees with this mapping, this issue can be narrowed to the encoding decision alone (`vortex.zstd` or another string codec in the normalised set), which after #788 is tessera's own codec-set choice.


---

# [Comment #6]() by [gerchowl]()

_Posted on November 22, 2026 at 02:17 PM_

Measured on real text, for the scitadel-hub size model (S-015, nerdmachines/scitadel-hub#89; tessera-py 0.1.0a1 built from `dev` at `f390528`):

| table (one block, whole sample) | rows | .tsra | Parquet zstd-3 | raw Arrow | tsra / Parquet |
|---|---|---|---|---|---|
| `elements` (paragraphs and PDF text blocks of 3,189 papers, 9 columns) | 683,030 | 90.8 MB | 52.2 MB | 159.9 MB | 1.74 |
| its `text` column alone | 683,030 | 83.6 MB | 43.8 MB | 127.8 MB | 1.91 |
| `citations` (two u8 columns) | 170,024 | 0.74 MB | 1.02 MB | 2.72 MB | 0.73 |

So on prose the cost is about 1.9× Parquet zstd, not 3×, and numeric columns are smaller than Parquet. At the hub's phase-1 scale (17 million papers with text) that is 0.46 TB against about 0.24 TB for the text column alone. The hub's hot set keeps this column, so the ask is the same as above: allow `vortex.zstd` for string columns in the normalised set, or document the cost.


