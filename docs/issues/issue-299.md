---
type: issue
state: open
created: 2026-07-02T14:14:24Z
updated: 2026-09-28T18:00:01Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/299
comments: 0
labels: none
assignees: none
milestone: backlog / research
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:53.552Z
---

# [Issue 299]: [T5: tessera as the unit in an unbundled data platform — catalog/index integrations](https://github.com/vig-os/tessera/issues/299)

**Tier 5** of the compute topology (EPIC #295). Reach TileDB-Cloud/Arraylake-equivalent platform capability by **composing open services over portable sealed products** (`tsra → S3 → OCI → index DB`), not a vertically-integrated platform.

**Principle: the index is a rebuildable *projection*; sealed products are the source of truth.** No lock-in; multiple catalogs coexist; integrity/provenance travel with the data; index the **PHI-safe** discovery metadata while PHI stays sealed.

**Integrations:**
- **InvenioRDM / Zenodo** — FAIR discovery, DOIs, metadata search (tessera already exports DataCite/RO-Crate → near-turnkey)
- **vector DB** (LanceDB/Qdrant/pgvector/Milvus) — similarity / 'find similar scan' / AI, keyed by product id
- **relational + search** (Postgres/OpenSearch/DuckDB over manifests + collections #223) — cohort query

Storage (S3 cloud reads) + distribution (OCI push/pull, cache-node) already exist; the index layer is the new integration work. Refs: #286
