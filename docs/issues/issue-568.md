---
type: issue
state: open
created: 2026-10-16T20:45:22Z
updated: 2026-10-16T20:45:22Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/568
comments: 0
labels: feature, python, area:bindings
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-17T06:32:47.667Z
---

# [Issue 568]: [feat(py): Python pack writes no aux/provenance.json and has no producer-owned aux member for run facts](https://github.com/vig-os/tessera/issues/568)

## Context

Dogfooding from MorePET/anatomist#86 (ADR-0007 draft) at tessera `dev` `fa581f5`, synthetic data only.

ADR-0042 puts the non-deterministic facts of a run (wall clock, host) in unsealed `aux/` so the seal stays deterministic. The Rust ingest paths stamp `aux/provenance.json` through `tessera_io::provenance::stamp_ingest_provenance`.

## Gap

1. A `.tsra` written by the Python `Builder.pack()` has no `aux/provenance.json`:
   ```python
   import zipfile
   # after Builder(...).pack("smoke.tsra")
   print(zipfile.ZipFile("smoke.tsra").namelist())   # ['mimetype', 'manifest.json', 'blocks/inputs']
   ```
   So a Python product records no `ingested_at` or host, and `tree` / `inspect` show none, even though Rust-ingested siblings do.
2. A producer has run facts beyond `{ingested_at, producer, host}`: anatomist's job runner records `started`, `finished`, `argv` and the batch job name. These belong outside the seal for the same ADR-0042 reason, but `IngestProvenance` has no room for them, and neither Python nor the CLI can add a producer-owned `aux/` member. `sign` is the only aux writer and is fixed to `aux/signatures/`.

As a result, anatomist (MorePET/anatomist#86) must keep a separate `manifest.json` next to each `.tsra` for its run facts. The single file to share that ADR-0042 promises doesn't hold for Python producers.

## Ask

- `Builder.pack(path, provenance=None)`, or a module-level `tessera.stamp_provenance(path, ingested_at=None, host=None)`, that writes `aux/provenance.json` like the Rust ingest does, with the same `TESSERA_SKIP_PROVENANCE` seam for tests.
- A producer-owned aux namespace: `Builder.add_aux_member(name, bytes)` (the generic variant proposed in #566) restricted to, say, `aux/producer/<tool>/…`, outside the seal, with the ADR-0042 invariant that `content_hash` and `manifest_hash` are unchanged. #407 (signed aux appendix) would then cover these members once it lands.

## Linked

#566 (generic `add_aux_member`), #407 (signed `aux/`), ADR-0042, MorePET/anatomist#86.

