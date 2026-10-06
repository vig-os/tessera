---
type: issue
state: open
created: 2026-10-05T10:09:20Z
updated: 2026-10-05T10:09:20Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/556
comments: 0
labels: bug
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-06T08:32:58.852Z
---

# [Issue 556]: [ingest: hdf-compound's streaming knobs still enter spec_hash, against the ADR-0026 rule](https://github.com/vig-os/tessera/issues/556)

## The rule

#538 added a rule to **ADR-0026** (and its corollary):

> Every ingest path that produces a table product MUST partition it into `BLOCK_ROWS`-sized blocks
> through the format SSoT … The corollary: **the streaming knobs are not content.** `streaming` and
> `batch_rows` choose how the work is done and cannot change a sealed byte, so they are excluded from
> `spec_hash` entirely (ADR-0035 hashes the parsed spec into every member's `ingested_via_spec` edge,
> so anything in `spec_hash` is in `manifest_hash`).

The three generic table lanes comply: their `streaming` / `batch_rows` are `#[serde(skip_serializing)]`.

## The gap

`FormatOptions::HdfCompound`'s own `streaming` and `slave_rows`-equivalent (`slab_rows`) are still
serialised, so they still enter `spec_hash` and therefore `manifest_hash` via `ingested_via_spec`.

The consequence is the one the rule exists to prevent: re-running an archived `hdf-compound` ingest
with `streaming = "stream"` — on a smaller machine, which is exactly when an operator reaches for it —
cannot reproduce the archived `manifest_hash` for byte-identical data, even though the streaming and
batch paths seal the same bytes (`ge_hdf5` has partitioned at `BLOCK_ROWS` on both paths since
`c5a48a2`, so the content really is identical).

## Why it was not fixed in #538

Changing it **moves the `spec_hash` of every existing `hdf-compound` spec**, which moves the
`manifest_hash` of every product ingested from one. That is plausibly a corpus event and certainly a
format decision, so it wants its own review rather than riding along in a PR about the generic table
lanes.

Note the asymmetry is currently *visible*: two lanes treat the same knob as identity-bearing and
three do not.

## Scope

- `tessera/crates/tessera-ingest/src/spec.rs` — `FormatOptions::HdfCompound { streaming, slab_rows, … }`
- Check whether any corpus fixture or trycmd `.tsra` was ingested through an `hdf-compound` spec; if
  so, this is a corpus regeneration and the goldens move.
- `docs/adr/0026-streaming-table-ingest.md` — the rule to cite.

