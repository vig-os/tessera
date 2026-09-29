---
type: issue
state: closed
created: 2026-02-25T22:29:23Z
updated: 2026-09-28T17:54:02Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/137
comments: 1
labels: chore, effort:medium, area:core, area:imaging
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:53:07.868Z
---

# [Issue 137]: [[CHORE] Create end-to-end DICOM-to-fd5 demo notebook with real data](https://github.com/vig-os/tessera/issues/137)

### Description

Create a Jupyter notebook that demonstrates the full fd5 workflow from raw DICOM data to a sealed fd5 file and back. This serves as the primary onboarding artifact — the "before and after" that shows what fd5 does and why it matters.

The notebook should use real (anonymized) or realistic synthetic DICOM data and walk through:

1. **The problem**: show the scattered metadata, repeated parsing, and fragility of working with raw DICOM files directly
2. **Ingest**: use `fd5 ingest dicom` (or the Python API) to produce an fd5 file
3. **Inspect**: use `fd5 info` and/or the read API to show the self-describing metadata, embedded schema, units, and provenance
4. **Visualize**: load the volume as a numpy array, display a slice, show the precomputed MIP
5. **Export**: generate the RO-Crate JSON-LD and/or TOML manifest from the fd5 file
6. **Validate**: run `fd5 validate` and show what a passing (and failing) validation looks like

### Acceptance Criteria

- [ ] Notebook runs end-to-end without errors in the devcontainer
- [ ] Uses anonymized/synthetic DICOM data (no real patient data committed)
- [ ] Demonstrates ingest, inspection, visualization, export, and validation
- [ ] Includes narrative markdown cells explaining each step
- [ ] Sample data is either generated programmatically or downloaded from a public source (e.g., TCIA)
- [ ] Notebook is in `examples/` or `docs/notebooks/`

### Implementation Notes

Consider using `pydicom`'s built-in test datasets or generating synthetic DICOM files with `pydicom.dataset.Dataset`. The DICOM ingest loader (#110) must be functional for this demo. If the read API (#136) is not yet available, raw h5py access is acceptable as a stopgap.

### Related Issues

Depends on #110 (DICOM ingest loader). Benefits from #136 (read API) if available.

### Priority

High
---

# [Comment #1]() by [gerchowl]()

_Posted on September 28, 2026 at 05:54 PM_

Closing as **obsolete** (2026-09-28 backlog triage).

Evidence: fd5-scoped. Tessera has real-data DUPLET DICOM ingests + Python bindings; demo would live under Tessera docs (see #389 for cookbook).

https://claude.ai/code/session_01XdERKMVDAwfMJSKdTytNnK

