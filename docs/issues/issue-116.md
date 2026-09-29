---
type: issue
state: closed
created: 2026-02-25T20:33:56Z
updated: 2026-09-28T17:53:39Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/116
comments: 1
labels: feature, effort:medium, area:core
assignees: gerchowl
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:53:11.087Z
---

# [Issue 116]: [[FEATURE] fd5.ingest.csv — CSV/TSV tabular data loader](https://github.com/vig-os/tessera/issues/116)

## Parent

Epic: #108 (Phase 6: Ingest Layer)

## Summary

Implement `src/fd5/ingest/csv.py` — a loader that reads CSV/TSV files and produces sealed fd5 files. Targets tabular scientific data: spectra, calibration curves, time series, device logs.

## Scope

### Product types supported

| Product type | CSV layout | Notes |
|-------------|-----------|-------|
| `spectrum` | columns: energy/channel + counts | Histogrammed data |
| `calibration` | columns: input + output + uncertainty | Detector calibration curves |
| `device_data` | columns: timestamp + signal channels | Device logs, slow control |
| generic | any columnar data | User specifies product type + column mapping |

### Key functionality

1. **Read CSV/TSV** — detect delimiter, header row, comment lines
2. **Column mapping** — user specifies which columns map to which fd5 fields (or auto-detect from header names)
3. **Type inference** — numeric columns → numpy arrays, string columns → attrs
4. **Metadata from header comments** — common pattern: `# units: keV`, `# detector: HPGe`
5. **Provenance** — record source CSV file with SHA-256 hash

### No additional heavy dependencies

Uses `numpy.loadtxt` / `numpy.genfromtxt` or stdlib `csv`. No pandas required (optional for complex cases).

## Proposed API

```python
def ingest_csv(
    csv_path: Path,
    output_dir: Path,
    *,
    product: str,
    name: str,
    description: str,
    column_map: dict[str, str] | None = None,
    delimiter: str = ",",
    header_row: int = 0,
    comment: str = "#",
    timestamp: str | None = None,
    **kwargs,
) -> Path:
    """Read a CSV/TSV file and produce a sealed fd5 file."""
```

## Acceptance criteria

- [ ] Implements `Loader` protocol from `fd5.ingest._base`
- [ ] Produces valid fd5 files that pass `fd5 validate`
- [ ] CSV and TSV (tab-delimited) supported
- [ ] Column mapping configurable; sensible auto-detection from headers
- [ ] Comment-line metadata extraction (e.g. `# units: keV`)
- [ ] Provenance records source file SHA-256
- [ ] Tests with synthetic CSV data
- [ ] ≥ 90% coverage

## Dependencies

Depends on: `fd5.ingest._base` (#109)
---

# [Comment #1]() by [gerchowl]()

_Posted on September 28, 2026 at 05:53 PM_

Closing as **obsolete** (2026-09-28 backlog triage).

Evidence: Superseded by #386 'tessera ingest table <CSV|PARQUET>' (ADR-0056 P1).

https://claude.ai/code/session_01XdERKMVDAwfMJSKdTytNnK

