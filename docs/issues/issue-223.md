---
type: issue
state: closed
created: 2026-06-28T14:52:26Z
updated: 2026-09-28T17:54:24Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/223
comments: 2
labels: area:core, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:53:02.522Z
---

# [Issue 223]: [Collection/study model: flat products + by-reference nesting; 3 projections (RO-Crate · OCI-index · S3-prefix); raw→derived boundary (ADR-0033)](https://github.com/vig-os/tessera/issues/223)

## What
Define how a multi-table vendor acquisition (e.g. GE: singles/coin_2p/coin_3p/events_2p/events_3p → recon) maps onto Tessera products, and a **collection/study** descriptor.

## Decision (from design review)
- **Flat physical products**: each `.tsra` stays ONE content-addressed product = raw **or** one derived stage. No monolithic nested container.
- **Logical collections nest BY REFERENCE** (a collection may reference sub-collections → recursive studies), via the existing `study` label + provenance `sources` DAG.
- **Three projections of one logical collection (not opinionated — support all):**
  - **RO-Crate** `ro-crate-metadata.json` (FAIR collection descriptor, already the discovery export target)
  - **OCI image index** (native manifest-of-manifests referencing N `.tsra` artifacts)
  - **S3/MinIO prefix** (a prefix of N independently range-readable `.tsra` objects)
- **Raw→derived boundary** drives WORM: raw → Compliance-mode (immutable); derived → Governance-mode (regenerable).

## Why flat + by-reference (vs monolithic container)
Per-product WORM retention, range-read/partial-fetch, dedup/regen, and OCI's own index primitive all want flat products; nesting belongs at the logical (descriptor) layer.

## Deliverables
- **ADR-0033** (collection descriptor + raw/derived boundary + the 3 store projections); reconcile with ADR-0022 (versioning DAG) + ADR-0025 (provenance).
- Collection descriptor type + emit/read for the 3 projections.
- Ingest maps a multi-dataset `.h5` → raw + derived products + a study collection.

Context: branch `spike/tessera-core`. Blocks/relates to #222 (generic reader feeds the per-table products).
---

# [Comment #1]() by [gerchowl]()

_Posted on June 28, 2026 at 10:26 PM_

**Model + projections landed (cbb89ad) — staying open for ingest wiring.**

In: `tessera-core::collection` (`Collection`/`CollectionBuilder`/`CollectionMember`/`Role`) — content-addressed via the SAME MMR (`hash::merkle_root`) over members' `manifest_hash`es as the manifest uses for blocks, so a collection is itself verifiable + inherits inclusion/consistency proofs. `tessera-io::collection` — the three projections (`to_rocrate` reusing `export::dataset_entity`, `to_oci_index` reusing `oci` constants + Result-on-missing-descriptor, `prefix_layout`) all enumerate the same member set in order; `retention_mode` maps raw→Compliance / derived→Governance WORM. 8 tests, fresh-context reviewed (APPROVE), gate green.

**Remaining for full #223:** wire ingest so a multi-dataset GE `.h5` (singles/coin_2p/coin_3p/events_2p/events_3p) produces **raw + derived products + a study collection** with `derived_from` edges — composes the generic reader (#222) + this model. This is also the catalog leg #225's cohort-scale reads need (enumerate members → ADR-0028 stats prune → range-fetch survivors).

---

# [Comment #2]() by [gerchowl]()

_Posted on September 28, 2026 at 05:54 PM_

Closing as **done** — verified on `origin/dev` in the 2026-09-28 backlog triage.

Evidence: ADR-0033 accepted; Collection model landed cbb89ad ('#223 / ADR-0033 — content-addressed collection model + 3 projections'). Multi-block query landed (LogicalTableView, ingest engine).

https://claude.ai/code/session_01XdERKMVDAwfMJSKdTytNnK

