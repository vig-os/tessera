---
type: issue
state: open
created: 2026-10-18T22:23:57Z
updated: 2026-11-08T11:44:04Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/628
comments: 3
labels: feature, question, priority:medium, effort:large, area:core, area:io, area:integrity, adr
assignees: none
milestone: none
projects: none
parent: 778
children: none
synced: 2026-11-09T09:04:28.245Z
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
1. **A determinism spike, run as ksb jobs.** Is Vortex's zstd string encoding byte-identical across x86_64/aarch64, across feature sets (Gate B), and across dev/release profiles? Also measure the size win on the consumer's string-heavy tables against Parquet. Repeat for an FSST-only or dictionary-only option, which needs no zstd in the reader and stays WASM-friendly.
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
- **Range reads:** the "whole-block reads" note matters here too. The corpus will live in an S3-compatible object store (versitygw, gerchowl/g-fleet#1121), and queries project a few columns out of wide rows.

