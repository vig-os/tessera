---
type: issue
state: open
created: 2026-10-16T21:39:10Z
updated: 2026-10-16T21:39:10Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/571
comments: 0
labels: feature, area:ingest
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-17T06:32:46.345Z
---

# [Issue 571]: [feat(ingest): spec derived_from accepts only in-spec names — no pinned edge to a parent from an earlier run](https://github.com/vig-os/tessera/issues/571)

## Context

Dogfooding from MorePET/anatomist#86 at tessera `dev` `fa581f5`, with synthetic data only. `tessera ingest --spec` is the one write path that seals `producer` and `generation` today, so it is a candidate interim route around #562 for Python producers that can emit files plus a spec (verified: exit 0, both blocks sealed, `provenance --require-complete` exits 0 within one spec).

## Gap

`[[product]] derived_from` accepts **only names of other products in the same spec** (`tessera-ingest/src/spec.rs:26`: "`derived_from` references resolve to spec-local product names (in-spec only, v1)"). A pipeline step that derives from a product written by an **earlier run** (anatomist: trees in #30 from the ingest product of #87, written by a separate batch job) cannot record a pinned `derived_from` edge to it through the spec. So the spec route cannot carry lineage across runs, and lineage across runs is the whole point for a multi-step pipeline.

## Ask

Allow an external parent in the spec, pinned by its seal, e.g.:

```toml
[[product]]
name = "trees"
derived_from_external = [{ id = "blake3:…", manifest_hash = "blake3:…" }]
```

or `derived_from = ["blake3:…"]` with an optional `--search <dir>` so that ingest resolves and verifies the parent file and pins its `manifest_hash`. The edge should be identical to what #563 would let the Python `Builder` write.

## Linked

#563 (the same edge from Python), ADR-0035 (declarative spec), ADR-0048, MorePET/anatomist#86.

