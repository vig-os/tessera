---
type: issue
state: open
created: 2026-11-11T01:35:24Z
updated: 2026-11-11T01:35:24Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/824
comments: 0
labels: bug, priority:medium, area:integrity
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-11T08:50:30.783Z
---

# [Issue 824]: [bug(format): .tsra zip bytes differ per run — aux/provenance.json ingested_at is wall-clock, ignores --timestamp](https://github.com/vig-os/tessera/issues/824)

## Description

Repeated `tessera ingest dicom-series` runs over the same input give the **same `content_hash` and `manifest_hash`**: tested over 6 runs with `RAYON_NUM_THREADS ∈ {1, 4, 8}` plus repeats. But the **outer `.tsra` zip differs on every run**, so the file's sha256, rsync and dedup tools, and WORM or content-addressed archive stores see a new file each time.

## Root cause

- `aux/provenance.json` (unsealed) records `ingested_at` as the **wall-clock** time, not the value of the `--timestamp` flag.
- That is the **only** differing zip entry. `mimetype`, `manifest.json` and every block entry are byte-identical, with fixed 1980-01-01 zip timestamps.

## Expected

With `--timestamp` given, or with `SOURCE_DATE_EPOCH` set, the whole container is byte-reproducible. Wall-clock fields either take that value or are left out. The documentation should say which bytes are reproducible: seal only, or the whole container.

## Reproduce

Ingest any DICOM series twice, e.g. a public TCIA ACRIN-NSCLC-FDG-PET CT (DOI 10.7937/tcia.2019.30ilqfcl). Then `unzip -p` both outputs and diff the entries. Only `aux/provenance.json` differs.

## Related

#568 (Python pack writes no `aux/provenance.json`), #259 (self-contained `.tsra` with non-sealed members), #407 (signed `aux/` appendix).

## Changelog Category

Fixed

