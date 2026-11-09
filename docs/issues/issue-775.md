---
type: issue
state: open
created: 2026-11-08T11:42:36Z
updated: 2026-11-09T03:32:37Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/775
comments: 1
labels: feature, effort:large, area:core, area:io, adr
assignees: none
milestone: none
projects: none
parent: 778
children: none
synced: 2026-11-09T09:04:23.678Z
---

# [Issue 775]: [feat(io): full-text BM25 search as an ADR-0054 derived cache — tantivy index over a declared text column, composable with filters and #769 vectors](https://github.com/vig-os/tessera/issues/775)

## Motivation

Text retrieval ("papers whose abstract mentions X", "reports that say Y", agent retrieval over free-text annotations) needs **ranked keyword search** over a text column, with the same filters, policy and provenance as every other tessera query. Vector search (#769) does not replace it. Embeddings miss exact terms, identifiers, chemical names and rare tokens, so the standard retrieval stack is **hybrid**: BM25 plus vectors, fused.

The first consumer is the scitadel-hub corpus (epic #778): 100M+ OpenAlex abstracts and about 8M US patents with full text, queried as filter + BM25 + vector and then re-ranked. The motivation is generic: #769 already names "semantic lookup over annotations/reports", and the same products will hold free-text report and annotation columns.

## What already exists

- **The derived-cache rule.** ADR-0054 Decision 2 (`docs/adr/0054-derived-caches-and-the-spine-block-boundary.md:74-98`): every columnar, joined or indexed form is an unsealed, rebuildable cache stamped with the `built_from` seal it mirrors; stale when `built_from ≠` the source seal. A full-text index is exactly such a cache. ADR-0054 is still **Proposed**, which is tracked in spike #365.
- **The query surface.** DataFusion 54 behind the CLI `sql` feature (`tessera/crates/tessera-cli/Cargo.toml:58`). ADR-0062 (Accepted) designs the streaming `TableProvider` (#654 → #666–#668). Today `tessera sql` materialises the block into a `MemTable` (`tessera/crates/tessera-cli/src/sql.rs:31`, ADR-0062 `:34`).
- **The precedent:** #769 plans HNSW as an ADR-0054 aux cache next to a strict-Vortex source of truth. This issue is the text counterpart.

## The gap

There is no text index of any kind, and no way to rank rows by relevance to a query string. The only option today is `LIKE` / regex over a full scan, which neither ranks nor scales to 100M rows.

## Proposal

1. **A declared text-index on a table column.** A product (or a derived product) declares `text_index{column, analyzer}`. The source of truth stays the Vortex string column. The index is **derived**.
2. **A tantivy index as an ADR-0054 aux cache.**
   - It is unsealed and rebuildable. Its stamp is `built_from = <source block digest / manifest_hash>` **plus the analyzer config hash**: tokenizer, language or stemmer, stop-words and the tantivy version. Changing any of these makes the cache stale, exactly like a source change, so a stale-is-never-fresh test covers both.
   - Rows map by **row id** (the logical-table row ordinal, ADR-0064 leaves included), not by a copied key. The cache never duplicates source columns beyond what tantivy's postings need (stored fields off).
   - **Location:** `aux/` beside the product (ADR-0042), or in a delegated cache store for read-only and object-store products. Keep the same options as ADR-0054's catalog table.
3. **A query surface as a DataFusion table function,** e.g. `bm25(block, column, 'query', k) → (row_id, score)`. It is composable:
   - joined back positionally or by row id to the table (projection and filter pushdown from #666);
   - fused with #769's `vec_topk(...)` via reciprocal-rank fusion (a small `rrf(...)` helper or documented SQL);
   - spanning **many products / a Collection**: one index per product, merged top-k (BM25 statistics are per-index; document the IDF caveat, or offer a collection-level merged index as a second cache kind).
4. **Bounds:**
   - **Build:** streams the column in row groups with a bounded writer heap (tantivy `IndexWriter` memory budget), so it never materialises the column.
   - **Query:** reads mmap'd segments, with RSS bounded by the segments touched.
   - Record build time, size and peak RSS for 1M, 10M and 100M rows as a ksb job (estimate only, to be measured: about 10–30% of the raw text size on disk).
5. **CLI:** `tessera index text <file> <block> --column abstract [--analyzer en_stem]`, `tessera search <file> <block> 'query' [-k 20]`, and `tessera index verify` (freshness check). Behind a feature (ADR-0057), so the lean build does not pull tantivy.

## Acceptance

- [ ] An ADR (or an amendment to ADR-0054) fixes the cache layout, the stamp (including the analyzer hash), row-id mapping and the multi-product semantics.
- [ ] A 10M-row text column indexes and queries in bounded RSS (measured), on a ksb job, not vm-dev.
- [ ] `bm25(...)` composes with a filter and with `vec_topk` (#769) in one SQL statement, and the fused top-k equals a reference implementation on a fixture.
- [ ] A changed source **or** a changed analyzer config makes the cache stale (test).
- [ ] The default build is unchanged: no tantivy in the dependency graph without the feature.

## Related

- Epic: #778 (scitadel-hub corpus, parent).
- #769 (vector search, the hybrid partner), #768 (vector columns), #654 / ADR-0062 (streaming provider), #365 / ADR-0054 (derived caches), #299 (external-index integrations: this is the in-format counterpart), #628 (string column size).


---

# [Comment #1]() by [gerchowl]()

_Posted on November 9, 2026 at 03:32 AM_

**Owner ruling, 2026-11-09 (search dependencies):** derived search caches are **optional, feature-gated, and pure Rust**.
- tantivy (#775) is allowed, being pure Rust. An HNSW tier (#769) must be pure Rust: no C++ such as usearch.
- The lean, WASM and determinism builds never pull them.
- A cache never enters `content_hash`; that depends on #720 (derived blocks out of identity), so these land after #720.

