---
type: issue
state: open
created: 2026-07-02T12:13:52Z
updated: 2026-09-28T17:59:47Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/288
comments: 0
labels: none
assignees: none
milestone: backlog / research
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:57.816Z
---

# [Issue 288]: [Study Icechunk's multi-writer transaction/concurrency model for concurrent CoW commits](https://github.com/vig-os/tessera/issues/288)

tessera's CoW versioning (ADR-0036) is single-writer. **Icechunk** (Earthmover; Apache-2.0 Rust crate) does **serializable multi-writer** commits over object storage with no external DB — conditional-write on the `$ROOT/repo` info file + immutable snapshots and branches/tags.

Evaluate borrowing that protocol for tessera versioning-at-scale (e.g. multiple ingest agents committing to one lineage concurrently). Note the key divergence to preserve: Icechunk uses **random 12-byte snapshot IDs**, whereas tessera identity is **content-addressed** — study the concurrency mechanism, keep content-addressing.

Spec: https://github.com/earth-mover/icechunk · Refs: #286
