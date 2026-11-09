---
type: issue
state: open
created: 2026-11-08T12:02:27Z
updated: 2026-11-08T12:02:38Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/780
comments: 0
labels: feature, effort:large, area:core, area:io, adr
assignees: none
milestone: none
projects: none
parent: 778
children: none
synced: 2026-11-09T09:04:21.418Z
---

# [Issue 780]: [feat(schema): declared keys and keyed references between blocks and across a collection — plus a key → product aux index](https://github.com/vig-os/tessera/issues/780)

## Motivation

The scitadel-hub corpus (epic #778) is a set of shard products, one per field × publication-year bucket, each holding several table blocks keyed by `work_id`: `works`, `elements`, `inline_refs`, `bib`, `table_cells`, `figures`, `chunks`. Rows reference each other **by key, not by position**:

- `elements.work_id → works.work_id` (same product);
- `inline_refs.target_id → bib.bib_id | figures.fig_id | table_cells.table_id` (same product);
- `bib.matched_work_id → works.work_id`, **across products** (a citation into another shard);
- delta products (#781) reference base rows by key.

Today the meaning of a key column lives only in the consumer's code. A reader, an agent, `tessera inspect` or a DataFusion session can't tell that `matched_work_id` joins to `works.work_id`, so it can't plan the join, validate referential integrity, or resolve a reference to the shard that holds it.

## What already exists (verified at `dev` `26159de`)

- **Positional links only.** ADR-0062 declares `RowAlignment` (`docs/adr/0062-streaming-table-provider.md:630-649`): a sealed `aligned_to` for a block that is **row-aligned** with a block in this product or in a `sources`-edge product. That is positional zip, not a keyed join.
- **Multi-product scans** (ADR-0062 §6, `:74`, `:887`) need identical schemas and give no join semantics.
- **No keyed-reference vocabulary anywhere in the code or the ADRs.** A grep for foreign/primary/join key finds only signing-key text.
- **Product-level links** are `Source` edges (`tessera/crates/tessera-core/src/provenance.rs:15`), which are provenance (`derived_from`, `ingested_from`), not row references.

## The gap

There is no sealed, schema-level declaration that a column is a **key** of its block, or a **reference** to a key in another block or in another product of a collection. Nor is there a way to find which product holds key K across a sharded collection.

## Proposal

1. **`TableSpec.keys`:** declare a block's key, for example `works: key(work_id)` or `elements: key(work_id, ord)`.
   - Optional uniqueness and sortedness claims, checked at seal and recorded in the chunk statistics (ADR-0059), so a sorted key prunes by chunk.
2. **`TableSpec.references`:** for example `bib.matched_work_id → works.work_id`.
   - Scope: `block` (same product), `source(role)` (a `sources`-edge product, like `RowAlignment`), or `collection(<schema>)` (any member of a declared collection).
   - Nullable references are allowed, since an unmatched citation is normal.
   - Sealed, so they're part of the product's identity.
3. **A key → product index as an ADR-0054 aux cache:** for a collection, a rebuildable `work_id → (product manifest_hash, leaf, row range)` map, stamped with the collection seal. It resolves cross-shard references in one lookup, and `detach` (a per-paper projection) uses it to collect one work's rows from every block.
4. **DataFusion:** the provider (#654) exposes declared keys and references as table and field metadata. A small helper registers the join relationships, so `JOIN` is planned with the sorted-key knowledge (merge join when both sides are sorted on the key).
5. **Validation:** `tessera verify --references` reports dangling references within scope. It's off by default, because cross-collection checks are expensive.

## Acceptance

- [ ] A product declaring `key(work_id, ord)` fails to seal if the rows aren't sorted or unique as declared.
- [ ] `inspect` and `tree` show keys and references. `verify --references` finds an injected dangling reference.
- [ ] A cross-shard reference resolves through the aux index without scanning other shards (measured: rows read).
- [ ] A DataFusion join of `bib` to `works` across a 10-shard collection uses the declared key (plan shows a merge or keyed join), in bounded RSS.

## Related

- Epic #778. ADR-0062 (`RowAlignment`, §6), ADR-0054 (aux caches, #365), ADR-0059 (chunk statistics), ADR-0033 / ADR-0049 / ADR-0050 (collections), #776 (graph edges are a special case: `src`/`dst` would be declared references), #781 (delta products).

