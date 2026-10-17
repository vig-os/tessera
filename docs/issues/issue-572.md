---
type: issue
state: open
created: 2026-10-16T21:39:12Z
updated: 2026-10-16T21:39:12Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/572
comments: 0
labels: bug, security, area:ingest
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-17T06:32:45.896Z
---

# [Issue 572]: [fix(ingest): spec seals the spec path in ingested_via_spec with no label, and the input path unless source_label is set](https://github.com/vig-os/tessera/issues/572)

## Context

Dogfooding from MorePET/anatomist#86 at tessera `dev` `fa581f5`, with synthetic data only.

## Gap

`tessera ingest --spec` seals two path-derived references:

- `ingested_from.reference` is the **input path, verbatim**, unless the product sets `source_label` (ADR-0040 `--source-label`). This is opt-in, so a spec that forgets it seals the path. Synthetic repro: `input = "series-fictus-001/t.csv"` without `source_label` seals `"reference": "series-fictus-001/t.csv"`.
- `ingested_via_spec.reference` is **the spec path as given on the command line** (`spec.rs:604`), and **there is no label option** for it. A spec stored under a patient- or series-named directory puts that name into every member's sealed manifest.

On clinical data, paths routinely carry identifiers (ADR-0040's own example: a DICOM series embedding the patient name 890 times).

## Ask

1. A `[collection] spec_label` (or a CLI `--spec-label`) used instead of the spec path in `ingested_via_spec.reference`.
2. Consider making a missing `source_label` a **warning or a refusal** under a `--phi-safe` flag (or by default for schemas with `identifying` fields), so the path-sealing default is not silent.

## Linked

ADR-0040 (PHI hygiene, `--source-label`), ADR-0048, MorePET/anatomist#86.

