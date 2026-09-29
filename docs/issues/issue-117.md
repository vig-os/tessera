---
type: issue
state: closed
created: 2026-02-25T20:34:05Z
updated: 2026-09-28T17:53:41Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/117
comments: 1
labels: feature, effort:medium, area:core
assignees: gerchowl
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:53:10.716Z
---

# [Issue 117]: [[FEATURE] fd5.ingest.parquet — Parquet columnar data loader](https://github.com/vig-os/tessera/issues/117)

## Parent

Epic: #108 (Phase 6: Ingest Layer)

## Summary

Implement `src/fd5/ingest/parquet.py` — a loader that reads Apache Parquet files and produces sealed fd5 files. Parquet's columnar layout and embedded schema map naturally to fd5's typed datasets and attrs.

## Scope

### Product types supported

Same as CSV but with richer type information from Parquet schema:

| Product type | Parquet content | Notes |
|-------------|----------------|-------|
| `spectrum` | columns: bins + counts | Histogrammed data |
| `listmode` | columns: event fields (time, energy, detector, ...) | Event-by-event data |
| `device_data` | columns: timestamp + channels | Time series |
| generic | any columnar data | Column metadata preserved |

### Key functionality

1. **Read Parquet** — via `pyarrow.parquet` or `polars`
2. **Schema extraction** — Parquet column types → numpy dtypes; Parquet metadata → fd5 attrs
3. **Column-to-dataset mapping** — each column becomes a dataset or attr depending on cardinality
4. **Row group handling** — large Parquet files read in chunks (streaming)
5. **Key-value metadata** — Parquet footer metadata mapped to fd5 root attrs
6. **Provenance** — record source `.parquet` file with SHA-256 hash

### Dependency

```toml
[project.optional-dependencies]
parquet = ["pyarrow>=14.0"]
```

## Proposed API

```python
def ingest_parquet(
    parquet_path: Path,
    output_dir: Path,
    *,
    product: str,
    name: str,
    description: str,
    column_map: dict[str, str] | None = None,
    timestamp: str | None = None,
    **kwargs,
) -> Path:
    """Read a Parquet file and produce a sealed fd5 file."""
```

## Acceptance criteria

- [ ] Implements `Loader` protocol from `fd5.ingest._base`
- [ ] Produces valid fd5 files that pass `fd5 validate`
- [ ] Parquet schema metadata preserved as fd5 attrs
- [ ] Column mapping configurable
- [ ] `ImportError` with clear message when pyarrow not installed
- [ ] Provenance records source file SHA-256
- [ ] Tests with synthetic Parquet data
- [ ] ≥ 90% coverage

## Dependencies

Depends on: `fd5.ingest._base` (#109)
---

# [Comment #1]() by [gerchowl]()

_Posted on September 28, 2026 at 05:53 PM_

Closing as **obsolete** (2026-09-28 backlog triage).

Evidence: Superseded by #386 (ADR-0056 P1 includes Parquet in the table-ingest ladder).

https://claude.ai/code/session_01XdERKMVDAwfMJSKdTytNnK

