---
type: issue
state: closed
created: 2026-02-25T20:34:18Z
updated: 2026-09-28T17:53:44Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/118
comments: 1
labels: feature, effort:large, spike, area:imaging
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:53:10.344Z
---

# [Issue 118]: [[SPIKE] fd5.ingest.root — ROOT TTree loader feasibility](https://github.com/vig-os/tessera/issues/118)

## Parent

Epic: #108 (Phase 6: Ingest Layer)

## Summary

**Spike/research issue** to assess feasibility and design for a ROOT TTree → fd5 loader using `uproot`.

ROOT is the dominant data format in particle/nuclear physics. TTrees are columnar event stores that map to fd5 `listmode`, `spectrum`, and `sim` product types. However, the complexity is significant: jagged arrays, friend trees, custom classes, and vendor-specific branch naming conventions.

## Questions to answer

1. **Can `uproot` read the target ROOT files?** — Test with real-world examples from the target experiments
2. **Jagged array handling** — ROOT TTrees often have variable-length arrays per event. How do these map to HDF5 datasets? (Options: vlen datasets, padding, separate datasets per branch)
3. **Branch → fd5 mapping** — What's the right heuristic for mapping TTree branches to fd5 datasets/attrs? Is a user-provided mapping always required?
4. **Performance** — For large TTrees (millions of events), what's the read throughput via uproot? Does chunked reading work?
5. **Metadata** — Where does ROOT store run metadata? (TNamed objects, TParameter, user info in TFile) How to extract it?
6. **Friend trees** — Can the loader handle friend-tree joins, or should it require a single merged TTree?

## Proposed investigation

1. Install `uproot` and `awkward-array`
2. Create synthetic ROOT files with representative structures
3. Prototype a minimal loader for `listmode` product type
4. Benchmark read performance
5. Document findings and propose API

### Dependency

```toml
[project.optional-dependencies]
root = ["uproot>=5.0", "awkward>=2.0"]
```

## Deliverable

A comment on this issue with:
- Feasibility assessment (go / no-go / conditional)
- Proposed API sketch
- Known limitations
- Performance benchmarks
- Recommended implementation issue(s) if go

## Dependencies

Depends on: `fd5.ingest._base` (#109)
---

# [Comment #1]() by [gerchowl]()

_Posted on September 28, 2026 at 05:53 PM_

Closing as **obsolete** (2026-09-28 backlog triage).

Evidence: fd5-scoped; not on any Tessera roadmap. Refile as a Tessera spike if particle-physics use arises.

https://claude.ai/code/session_01XdERKMVDAwfMJSKdTytNnK

