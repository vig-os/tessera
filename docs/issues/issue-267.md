---
type: issue
state: closed
created: 2026-07-01T14:23:27Z
updated: 2026-09-28T17:54:33Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/267
comments: 1
labels: bug
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:53:00.324Z
---

# [Issue 267]: [bug(export): RO-Crate/DataCite leak PHI + emit invalid JSON-LD (provenance stored as one comma-joined string)](https://github.com/vig-os/tessera/issues/267)

**Found by 4 of 5 usability reviewers (analyst, explorer, auditor, ingest).** Highest-priority: a "FAIR discovery record" that leaks PHI and isn't consumable.

## Symptoms
- `export ro-crate <ct-series>.tsra` → `isBasedOn[0].@id` is a **256,319-character comma-joined string of 890 DICOM paths**, not a JSON-LD node. `jq '.["@graph"][]|select(.["@type"]=="Dataset").isBasedOn[0]["@id"]|length'` → `256319`.
- Patient name (`CHERICO`), MRN (`2393105__2201488`), and full acquisition dates flow **verbatim** into `export ro-crate`, `export datacite`, and `inspect --full`.
- Absolute host paths (`/home/.../sdsc_dump/...`) baked into the record → doxxes the ingesting workstation + leaks study/date identifiers.

## Root cause (explorer + auditor B5)
Multi-file provenance (a DICOM series' `ingested_from`) is stored as **one comma-joined scalar `String`**, not a list. `ls` re-splits on `,` (so any path containing a comma silently corrupts the file list), and the exporters emit it as a single `@id`.

## Fix
1. Model provenance sources as a first-class list: `Vec<SourceFile { reference, digest, size }>` (or keep the string but carry a structured sidecar), replacing the comma-join. This also fixes the `inspect --full` 250KB one-liner and the comma-in-path hazard.
2. RO-Crate exporter: emit one `{"@type":"File","@id":<opaque-id>,"sha256":…,"contentSize":…}` per source in `@graph`, referenced from `isBasedOn`.
3. **PHI-safe by default:** emit opaque source identifiers (`urn:tessera:source:<blake3>`) instead of absolute filesystem paths; add `--paths-verbatim` for trusted internal repos.

Relates to the `--source-label`/`--deidentify` PHI issues (filed separately) and the provenance-integrity work (#250, which already added per-edge `content_hash` — reuse it here).
---

# [Comment #1]() by [gerchowl]()

_Posted on September 28, 2026 at 05:54 PM_

Closing as **done** — verified on `origin/dev` in the 2026-09-28 backlog triage.

Evidence: commit 6b4b55d 'fix(export): PHI-safe + valid JSON-LD FAIR records — opaque source URNs, not raw paths (#267)' + ADR-0048 (provenance edge shape, 5333f5f).

https://claude.ai/code/session_01XdERKMVDAwfMJSKdTytNnK

