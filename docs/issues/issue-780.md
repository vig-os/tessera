---
type: issue
state: open
created: 2026-11-08T12:02:27Z
updated: 2026-11-18T14:58:20Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/780
comments: 1
labels: feature, effort:large, area:core, area:io, adr
assignees: none
milestone: none
projects: none
parent: 778
children: none
synced: 2026-11-19T09:52:21.677Z
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

---

# [Comment #1]() by [gerchowl]()

_Posted on November 18, 2026 at 08:40 AM_

**Input for the #780 ADR: the key lookup after the index hit, with measurements** (agent, from the kit benchmark [private-repo]#98 for [private-repo]#97, 2026-11-18). The split-rule and delta numbers from the same harness are on #781.

kit wants keyed reads of small rows (tasks, edges, settings, document blocks) to be about as cheap as a row store's. Three points for the ADR. None of them changes the proposal above; they are about what happens once the index has named a leaf and a row range.

**Setup.** A standalone harness on Vortex 0.93.0 from crates.io with default codecs, not tessera, the fork pin or the canonical profile. 10⁶ synthetic rows of about 350 bytes (`u64` key, three small integers, a 40-byte and a 300-byte string). SQLite in WAL mode as the reference. Timings are from a shared host with the data hot; byte counts and ratios within one run are the evidence.

**1. The pyramid plus a declared sorted key is already the navigation half of a B-tree.**
- Point 1 above records sortedness in the chunk statistics (ADR-0059) so a sorted key prunes by chunk. On a key-sorted table the `[min, max]` ranges of sibling nodes in ADR-0028's hierarchy do not overlap, so descending product → block → chunk → sub-chunk by key range is the same walk as through a B-tree's interior pages, and each step can be checked against the hash path.
- Question for the ADR: is the aux index of point 3 a separate map, or is it this descent over the statistics that already exist, with a separate map only across the products of a collection? The second needs no new derived block inside a product.
- The lowest level still needs the keys themselves (or a sparse sample of them) to turn a sub-chunk into a row position. In the harness, the leaf's sorted key column held in memory and a binary search cost well under a microsecond.

**2. Finding the row is not the cost. Fetching it through a scan is.** Per lookup, with the leaf open and the position known, on one 1,048,576-row leaf:

| Read | `scan().with_row_indices()` on the file | Same scan, file held in memory | Take on the encoded arrays held in memory | SQLite |
|---|---:|---:|---:|---:|
| One small integer column | 109 µs, 249 B read | 72 µs | 0.24 µs | 2.5 µs |
| The 300-byte string column | 1,000 µs, 1.9 MiB read | 518 µs | 3.7 µs | 2.1 µs |
| Full row, six columns | 1,182 µs, 4.3 MiB read | 751 µs | 5.7 µs | 2.3 µs |

- A scan call costs about 70 µs whatever it reads, also with the whole file in memory.
- It fetches one whole segment per projected column, and for the string column it appears to decode the whole 8,192-row chunk to return one row (about 400 µs; inferred from stage timers, since `perf` was not available).
- The format itself gives cheap random access: a take on the arrays in their Vortex encodings returns a full row in 5.7 µs. The values returned by that arm have not yet been compared with the oracle.
- So the acceptance item "resolves through the aux index without scanning other shards (measured: rows read)" can be met while a lookup still costs a millisecond and megabytes. Suggest measuring bytes read per lookup beside rows read.
- Option for the ADR: a keyed read path that takes directly from a fetched or cached sub-chunk and does not go through the scan. This is a reader-side path and changes nothing sealed.

**3. Sub-chunk size decides the cold cost, and it is a profile constant.**
- Vortex 0.93.0 writes 8,192-row blocks with a 1 MiB minimum per segment (`vortex-file/src/strategy.rs`, as read by the harness agent; the measured segment sizes match). A narrow column in a large leaf is therefore read whole: 375 KiB for one 3-bit value.
- With 1,024-row blocks and 16 KiB, a full-row read drops from 4.3 MiB to 1.3 MiB, a six-column scan gets 3.8 times slower and the file 11 % larger. Smaller blocks were not tried.
- A row store answers from one page of a few kilobytes that holds whole rows. To come close on a cold read, a small-row profile (#869, declared constants) would need sub-chunks of kilobytes, and a full row still costs one read per column unless a sub-chunk's columns sit next to each other.

**Not measured:** the fork pin and the canonical codec profile; cold reads and object storage; tables above 10⁶ rows for the in-memory arm; real data (the strings are random and do not compress); the cross-product index itself.


