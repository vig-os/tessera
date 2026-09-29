---
type: issue
state: closed
created: 2026-02-25T20:25:27Z
updated: 2026-09-28T17:53:37Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/114
comments: 1
labels: feature, effort:large, area:imaging
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:53:11.449Z
---

# [Issue 114]: [[FEATURE] fd5.ingest.midas — MIDAS event data loader](https://github.com/vig-os/tessera/issues/114)

## Parent

Epic: #108 (Phase 6: Ingest Layer)

## Summary

Implement `src/fd5/ingest/midas.py` — a loader that reads MIDAS `.mid` event files (PSI/TRIUMF DAQ system) and produces sealed fd5 files.

## Context

MIDAS (Maximum Integrated Data Acquisition System) is used in nuclear/particle physics experiments. It produces binary event files with bank-structured data. See [RFC-001 § Prior Art](docs/rfcs/RFC-001-2026-02-25-fd5-core-implementation.md) for context.

## Scope

### Product types supported

| Product type | MIDAS input | Notes |
|-------------|-------------|-------|
| `listmode` | Coincidence event banks | Event-by-event data with timestamps |
| `spectrum` | Histogrammed banks | Pre-binned energy/time spectra |
| `device_data` | Slow control banks | Temperature, pressure, HV readings |

### Key functionality

1. **Event bank parsing** — read MIDAS binary format (16-byte event headers, 4-char bank IDs)
2. **Bank type mapping** — map known bank types to fd5 product types
3. **Timestamp extraction** — from MIDAS event headers
4. **Run metadata** — extract ODB (Online DataBase) settings if available
5. **Provenance** — record source `.mid` file with SHA-256 hash

### Dependency

This may require a custom MIDAS reader or `midas` Python package (if available). Research needed during implementation.

```toml
[project.optional-dependencies]
midas = ["midas>=0.1"]  # TBD — may need custom reader
```

## Acceptance criteria

- [ ] Implements `Loader` protocol from `fd5.ingest._base`
- [ ] Produces valid fd5 files that pass `fd5 validate`
- [ ] At least `listmode` product type supported
- [ ] Binary event format correctly parsed
- [ ] `ImportError` with clear message if deps not installed
- [ ] Provenance records source file SHA-256
- [ ] Tests with synthetic MIDAS-format data
- [ ] ≥ 90% coverage

## Dependencies

Depends on: `fd5.ingest._base` (#109)

## Notes

This is the most complex loader due to the binary format and vendor-specific bank structures. May require a spike issue for MIDAS format research if no suitable Python library exists.
---

# [Comment #1]() by [gerchowl]()

_Posted on September 28, 2026 at 05:53 PM_

Closing as **obsolete** (2026-09-28 backlog triage).

Evidence: fd5-scoped; no MIDAS spike proposed for Tessera. Refile if a real MIDAS need arrives.

https://claude.ai/code/session_01XdERKMVDAwfMJSKdTytNnK

