---
type: issue
state: open
created: 2026-11-08T11:42:37Z
updated: 2026-11-08T11:43:42Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/776
comments: 0
labels: feature, effort:large, area:core, area:io, adr
assignees: none
milestone: none
projects: none
parent: 778
children: none
synced: 2026-11-09T09:04:23.181Z
---

# [Issue 776]: [feat(schema): graph edge tables + a CSR adjacency aux cache — traversals, algorithms and node embeddings without a graph DB as source of truth](https://github.com/vig-os/tessera/issues/776)

## Motivation

Many datasets are a graph as much as a table: citations, authorship, patent families, patent classifications, patent-to-paper references, dataset lineage. Consumers need:

- **traversals:** k-hop neighbourhoods, citation chains, co-citation;
- **algorithms:** PageRank, connected components, community detection;
- **node embeddings** for similarity ("works that sit in the same part of the citation graph").

The first consumer is the scitadel-hub corpus (epic #778). Its rough sizes, estimates only:

- OpenAlex has on the order of 10⁹–10¹⁰ citation edges in full, and a topic subset of 10⁷–10⁸.
- USPTO grants from 1976 have on the order of 10⁸ citation edges.

The usual answer is a graph database such as Neo4j. This issue proposes **no graph DB as the source of truth**:

- **Sealed products stay authoritative.** A graph DB would be a second, mutable store, which is exactly what ADR-0054 and #299 rule out ("the index is a rebuildable projection").
- **The Rust embedded graph-DB landscape is young after Kùzu was archived (2025):** KyuGraph (a pure-Rust Kùzu-lineage port, created 2026-02), NervusDB v2 (a Cypher subset, binaries "coming"), CypherLite (an openCypher subset) and Grafeo (GQL-focused, vendor-only benchmarks). None of them is a dependency we should seal against today.

## What already exists

- Table blocks are deterministic Vortex files with chunk statistics (ADR-0059) and an accepted streaming DataFusion provider design (ADR-0062, #654). DataFusion 54 (`tessera/crates/tessera-cli/Cargo.toml:58`) supports `WITH RECURSIVE`.
- Derived caches follow ADR-0054 Decision 2 (`docs/adr/0054-derived-caches-and-the-spine-block-boundary.md:74`).
- Provenance already is a typed DAG (ADR-0048 edge shape, `tessera provenance` from #452). That covers **product-level** edges, not row-level data edges.
- #580 covers anatomical trees and graphs (vessel trees, polylines, meshes) as geometric schema objects. That is a **different scope**: geometry, not relational edge sets. Cross-reference both, and don't merge them.
- Vector columns and vector search are planned in #768 and #769.

## The gap

There is no convention for storing edges, no adjacency structure for algorithms, and no traversal surface beyond hand-written self-joins over a full materialisation.

## Proposal

1. **An edge-table convention (schema, no new primitive).** An edge set is an ordinary table block with typed columns `src`, `dst`, `kind` (dictionary-encoded), plus optional `weight`, `t` and attributes.
   - **Endpoints are either** row ids into a named node table in the same product, **or** `(product content_hash, row id)` / stable external ids for cross-product edges.
   - **The schema declares** which node tables an edge block references, so the reader can validate referential integrity. Validation is optional, and a missing endpoint is reported, never silently dropped.
2. **A CSR / adjacency ADR-0054 aux cache:** offsets + targets (and a reverse CSR for in-edges), built by streaming the edge block, stamped `built_from` the edge block digest. It feeds the algorithms: a plain CSR in Rust, or `petgraph` views over it, with PageRank, connected components and BFS to start. Results (e.g. a pagerank score per node) are written as a **derived product** with a provenance edge, not into the source.
3. **Traversals in SQL:** `WITH RECURSIVE` over the edge table through the streaming provider for small k, plus a table function `neighbours(edges, seeds, hops, kind)` backed by the CSR cache for large fan-out.
4. **Node embeddings:** node2vec or GraphSAGE embeddings (trained outside tessera, e.g. a GPU job) stored as a **declared vector column** (#768) on the node table, or in a derived product, and searched with #769. This is the "graph embeddings" half of hybrid retrieval: BM25 (text) + vectors (semantics) + graph proximity.
5. **A trait seam for a query language later.** Keep traversal and algorithm entry points behind a small trait so a Cypher/GQL layer (one of the engines above, once mature) can be plugged over the same edge tables and CSR cache **as another derived projection**. Out of scope now.

## Acceptance

- [ ] An ADR records the edge-table convention (endpoint addressing, cross-product edges, integrity checking) and the CSR cache layout and stamp.
- [ ] The CSR cache builds in bounded memory from a streamed edge block, measured at 10⁸ edges on ksb (estimate: 8–16 bytes/edge per direction).
- [ ] A k-hop `neighbours(...)` table function and PageRank over the cache, cross-checked against a reference implementation on a fixture graph.
- [ ] A derived-results product (pagerank or component id) carries a provenance edge to the edge block it came from.
- [ ] A docs example: citation graph → co-citation → node embeddings (#768) → `vec_topk` (#769).

## Related

- Epic: #778 (scitadel-hub corpus, parent).
- #580 (geometric trees/graphs, different scope), #768 / #769 (vectors), #654 / ADR-0062 (streaming provider), ADR-0054 / #365 (derived caches), ADR-0048 (provenance edges), #299 (external indexes as projections).


