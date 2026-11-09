---
type: issue
state: open
created: 2026-11-08T11:43:13Z
updated: 2026-11-09T03:32:40Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/778
comments: 2
labels: feature, epic, area:core, area:io
assignees: none
milestone: none
projects: none
parent: none
children: 758, 628, 654, 768, 769, 775, 776, 777, 779, 780, 781, 782
synced: 2026-11-09T09:04:22.341Z
---

# [Issue 778]: [use case: scitadel-hub — literature & patent corpus on tessera (strings, vectors, BM25, graph, streaming queries, Parquet export)](https://github.com/vig-os/tessera/issues/778)

**A consumer use case for planning.** This epic collects everything tessera needs for **scitadel-hub**: a lab-level literature and patent corpus stored as tessera products. It does not ask tessera to know about papers or patents. Every item below is a generic capability (strings, vectors, text search, graphs, streaming queries, publishing `.tsra` on Hugging Face), and the corpus is the first consumer that exercises them all at scale.

## What scitadel-hub is

[scitadel](https://github.com/vig-os/scitadel) is a Rust literature-review tool (CLI, MCP, TUI) that searches PubMed, OpenAlex, arXiv, Crossref, Europe PMC, patent offices and others through their live APIs. **scitadel-hub** is a planned server-side split: a shared, prefetched corpus for a lab or institute, so that search, scoring and reading run locally (CPU plus the anvil GPU) instead of through thousands of rate-limited API calls.

Two triggers:

- **PatentsView is gone.** The USPTO PatentsView PatentSearch API was paused on 2026-03-20 and `search.patentsview.org` no longer resolves, so patent search needs an owned corpus.
- **OpenAlex refreshes are paid.** OpenAlex's free API key covers $1/day, but its "changed since" sync is a paid Member+ feature. A quarterly snapshot plus targeted refresh is the free middle ground.

### Data (all sizes are **estimates** from upstream documentation, not measurements)

| Dataset | Rows | Raw size | Notes |
|---|---|---|---|
| OpenAlex works, topic subset | 10⁶–10⁷ | 5–50 GB | from the quarterly Parquet snapshot (CC0) |
| OpenAlex works, full | ~4.8×10⁸ works (~6.3×10⁸ records over all entities) | ~770 GB Parquet, compressed | quarterly; string-heavy |
| OpenAlex abstract embeddings, 384-d | ~10⁸ | ~77 GB at f16 | MiniLM-class; i8 + scale would be ~38 GB |
| USPTO grants 1976–present, full text | ~8×10⁶ | 50–100 GB | weekly full-text XML; public domain |
| PatentsView `PVGPATDIS` tables | 35 TSVs | multi-GB | inventors, assignees, CPC, citations; CC-BY-4.0 |
| Patent embeddings, PatentSBERTa 768-d | ~8×10⁶ | ~12 GB at f16 | |
| Citation / authorship / CPC edges | 10⁸–10¹⁰ | — | the graph half |

### Product model

- **Sealed, versioned products.** Each quarterly OpenAlex snapshot (or subset), each USPTO bulk release and each **daily delta** becomes a **new sealed product**. Daily deltas come from (a) topic/author/venue subscriptions filtered by publication date and (b) a rolling refresh of known works in 100-id batches. "Current" is a Collection or catalog pointer (ADR-0049 / ADR-0054 Decision 3), so nothing is rewritten.
- **Add-only storage.** Products live on anvil's research-data storage (`/srv/anvil/projects/<project>/`, add-only by fleet rule) and in an **S3-compatible object store (versitygw, gerchowl/g-fleet#1121)**, read through `object_store` range reads (the `cloud` feature, #225; #628 notes that whole-block GETs are the latency problem).
- **Everything is derived and rebuildable from upstream,** so tessera's pre-1.0 format is acceptable for this consumer. A format change means a rebuild, not data loss.
- **Provenance carries the useful facts:** which OpenAlex release, which USPTO week, which embedding model and version, which analyzer config.

### Query patterns

1. **Filter + projection over 10⁸ string-heavy rows** (year, topic, type, OA status, assignee, CPC) through the streaming DataFusion provider, across many products (Collection union).
2. **Hybrid retrieval:** BM25 top-k (text) ∪ vector top-k (semantics) ∪ graph neighbourhood (citations), fused (e.g. reciprocal-rank fusion), then filtered.
3. **Re-ranking outside tessera:** the top few thousand hits are scored by a System One model (Clef/Kev) and then read by an LLM. Tessera's job ends at "give me candidate row ids + columns fast".
4. **Publishing:** `.tsra` only (sealed, FAIR) on Hugging Face, read remotely by column via HTTP range requests; there is deliberately no Parquet copy, because Vortex gives better column access (owner decision 2026-11-08). The patent corpus with embeddings is, as far as we found, not available anywhere today.

## Dependency DAG and suggested waves

```
W0  #758 fork pin (dependents silently lose determinism fixes)
    #628 string-heavy size (≈3× Parquet; corpus is mostly strings)   ── format decision
W1  #654 streaming TableProvider → #666 → #668 (multi-product / Collection / object store)
    #780 declared keys + keyed references + key → product index   (schema; prerequisite of #781)
    #782 pipeline provenance, per-row provenance, external unheld inputs (before anything seals at scale)
W2  #768 declared vector columns → #769 vector search
    #775 BM25 / full-text aux cache          (needs #654 for composition)
    #777 .tsra on Hugging Face: card + remote range reads (needs #666; vectors via #768)
    #781 keyed delta products: upsert/tombstone/redirect, newest-wins read, compaction (needs #780, #668)
    #779 declared media columns for figure bytes (independent; small)
W3  #776 graph edge tables + CSR aux cache   (node embeddings via #768/#769)
    ADR-0054 acceptance (#365): all three new caches lean on Decision 2
```

**Why that order:**

- W0 decides how big and how deterministic the corpus is before anything is sealed at scale.
- W1 is a hard blocker: a 10⁸-row table cannot be materialised into a `MemTable` (`sql.rs:31`). #780 and #782 sit in W1 because they change what a sealed product says about its rows; sealing the corpus before them would mean re-sealing it.
- W2 delivers retrieval. W3 adds graph proximity.

## Acceptance (consumer view)

- [ ] A 10⁸-row abstracts table (titles + abstracts + metadata) seals at a size within an agreed factor of the equivalent Parquet (the target is set by #628's ADR).
- [ ] `SELECT … WHERE year >= 2020 AND topic IN (…)` over a Collection of quarterly + daily-delta products, local and over S3, runs in bounded RSS.
- [ ] One SQL statement does filter + `bm25(...)` + `vec_topk(...)` + RRF over 10⁸ rows and returns top-100 with the columns in seconds, in bounded RSS (exact target to be set when measured on ksb).
- [ ] A 768-d f16 embedding column for 8×10⁶ patents is stored natively (#768) and searchable (#769).
- [ ] A k-hop citation neighbourhood and PageRank run from the CSR cache (#776).
- [ ] A sealed corpus product published to an HF dataset repo can be verified and column-projected remotely (range reads, no full download), vectors included (#777).

## Related

- Sub-issues: #758, #628, #654, #768, #769, #775, #776, #777, #779, #780, #781, #782.
- Context: #299 (external indexes as rebuildable projections; this epic keeps the indexes in-format), #365 / ADR-0054, ADR-0062, ADR-0064, #580 (geometric graphs, different scope).
- Fleet: gerchowl/g-fleet#1121 (research-data S3 buckets, versitygw).
- Consumer: vig-os/scitadel (the scitadel-hub ADR is to be filed there).



---

# [Comment #1]() by [gerchowl]()

_Posted on November 8, 2026 at 12:03 PM_

**Consumer use case: the scitadel-hub document model.** This is what the corpus will put into tessera, and why #779–#782 were filed.

**Shard products.** One product per OpenAlex field (26) × publication-year bucket, about 1–10 GB each (estimate), grouped into collections. A per-paper `.tsra` would bury about 20 KB of text under manifest and footer overhead, starve the string codecs of volume, and turn every corpus scan into millions of opens.

```
collection  hub/openalex/base/2026-Q4
 └─ collection  field=<one of 26>
     └─ product  docs.<field>.<yearbucket>.tsra
          works/NNNN        work_id (key), doi, licence class, source kind, content_hash (per-work blake3), prov_id (#782)
          elements/NNNN     work_id, ord, parent_ord, kind, text, page, bbox[4]      key (work_id, ord)
          inline_refs/NNNN  work_id, ord, span, target_kind, target_id               → bib / figures / table_cells (#780)
          bib/NNNN          work_id, bib_id, parsed fields, doi, matched_work_id     → works.work_id across shards (#780, #776)
          table_cells/NNNN  work_id, table_id, row, col, text, num: f64?, unit       numbers kept apart from text
          figures/NNNN      work_id, fig_id, caption_ord, bbox, media_type            bytes in the fig-blobs product
          chunks/NNNN       work_id, chunk_id, ord span, tokens                       derived
 ├─ product  emb.<model>.<field>.<yb>.tsra   chunk and figure vectors, row-aligned with chunks/figures (#768, ADR-0062 RowAlignment)
 ├─ product  fig-blobs.<field>.<yb>.tsra     SVG / WebP bytes in a declared media column (#779), kept out of the text shard
 └─ aux/     bm25 (#775) · ann (#769) · csr over bib edges (#776) · work_id → shard index (#780)
delta/2026-11-08.tsra   same blocks + op column (upsert | delete | redirect) over the base (#781)
current  → collection{ base 2026-Q4, deltas[…] }
```

- **Every block is sorted by `(work_id, ord)`,** so one paper's rows are contiguous and "give me paper W" prunes by chunk statistics (ADR-0059) in every block.
- **PDFs are never stored in tessera.** Originals are referenced by content-addressed blake3 + URL + fetch date. They are either held in a separate cold artefact store (versitygw, gerchowl/g-fleet#1121) or discarded once processed, when they can be fetched again. #782 makes such unheld inputs first-class leaves, so they don't show up as provenance gaps.
- **Full provenance of every step:** acquire → parse (JATS | HTML | Grobid TEI | PDF extractor) → table/figure extraction → chunking → embedding → index. Typed steps go in a pipeline slot, and per-row `prov_id` handles shards that mix sources (#782).
- **Re-extraction and re-embedding are new derived products** (ADR-0033 roles). Bases and deltas are never rewritten in place.
- **Client-side `detach`:** a per-paper `.tsra` projection of one work's rows from every block (found through the #780 index), with a `derived_from` edge to the shard seal, so a user's local copy stays verifiable against the hub.


---

# [Comment #2]() by [gerchowl]()

_Posted on November 9, 2026 at 03:32 AM_

**Architect triage, 2026-11-09, with owner rulings.**

Sequence:
1. ADR-0054 accept (#365).
2. #720, the derived-block identity exclusion.
3. #719, views.
4. The #654 provider PRs (#666–#668).
5. #780 (keys, references).
6. Then #768 → #769, #775, #779, #781, #782, #777 (generic).
7. #776 last.

Constraints:
- **Must not land before #720:** #775, #776, #769 tier 2 and #782's derived per-row provenance. They add derived blocks, which would move `content_hash` today.
- **Ride the ADR-0059 format event:** #768 and #779, which add sealed column carriers, ship with it rather than as point PRs.

Owner rulings:
- #777 split (HF helper → separate backlog issue).
- #781 and ADR-0036 are decided in one ADR.
- Search caches are feature-gated and pure Rust (tantivy OK, no C++ HNSW).
- The visionx-rs pair #762 and #763 runs now, outside this epic.

