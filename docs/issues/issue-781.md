---
type: issue
state: open
created: 2026-11-08T12:02:29Z
updated: 2026-11-18T14:58:17Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/781
comments: 3
labels: feature, effort:large, area:core, area:io, adr
assignees: none
milestone: none
projects: none
parent: 778
children: none
synced: 2026-11-19T09:52:21.146Z
---

# [Issue 781]: [feat(io): keyed delta products over a base — upserts, tombstones and redirects, a newest-row-wins logical read, and compaction into a new sealed base](https://github.com/vig-os/tessera/issues/781)

## Motivation

The scitadel-hub corpus (epic #778) mirrors upstream sources that change continuously:

- **OpenAlex** publishes a full quarterly snapshot. In between, the hub pulls changes daily: new works from topic subscriptions, and refreshed records in batches of 100 IDs. Works also get **merged** (W1 → W2) and **deleted**.
- **USPTO** publishes weekly patent grants (mostly additions, with corrections).

The layout is **one base generation per quarter, plus small daily delta products, plus a tiny `current` collection** that pins one base and its deltas. Readers need one logical table where **the newest row per key wins** and tombstones remove or redirect keys. Storage is add-only (the `/srv/[gpu-server]` data rule; versitygw, [fleet-config-repo]#1121): nothing is rewritten in place, and a new base is a new sealed product.

## What already exists (verified at `dev` `26159de`)

- **Versioning is copy-on-write at product and block level.** ADR-0022 is Accepted (`supersedes`/`derived_from` edges). ADR-0036 is Proposed (`docs/adr/0036-versioning-audit-object-store.md`), with a content-addressed object store, block sharing across versions and git-shaped verbs. An edit is "replace a block", which is too coarse when 10⁴ rows of a 10⁸-row base change every day.
- **Multi-product scans** (ADR-0062 §6, `docs/adr/0062-streaming-table-provider.md:74`, `:887`) union members with identical schemas. They have no key semantics, so a row present in both the base and a delta comes back twice.
- **Logical tables** (ADR-0064) join numbered leaves within one product, not across generations.
- **No upsert, tombstone, newest-wins or compaction concept** anywhere in the code or the ADRs (grep for tombstone, upsert, compaction).

## The gap

There is no keyed delta semantics over a base: no product role for "these rows replace rows with the same key in base B", no tombstone vocabulary, no logical read that resolves the newest version, and no compaction verb that folds deltas into a new base.

## Proposal

1. **A delta product role:** `Role::Delta { base: <manifest_hash of the base or its collection>, seq }`.
   - Its table blocks have the base's schema plus a sealed `op` column: `upsert | delete | redirect(new_key)`.
   - The key comes from #780 (`TableSpec.keys`). A delta can't be sealed against a block with no declared key.
   - Deltas are ordered by `seq` and an optional version column (for example upstream `updated_date`), declared as the tie-breaker.
2. **A "current" collection:** a collection schema (ADR-0050) with members `{base, deltas[...]}` pinned by `manifest_hash`. Advancing it seals a new tiny collection; nothing else changes.
3. **The logical read** (`#654` provider): over a current collection, return each key's newest row.
   - Plan: union of the deltas, newest per key, then base anti-joined with the delta keys, unioned with the surviving delta rows. Deltas are small, so a hash set of delta keys stays in bounded RSS.
   - `redirect` rows rewrite references through a merge map (OpenAlex `merged_ids`).
   - Aux caches (BM25 #775, vectors #769, CSR #776) are built per member. The query layer merges per-member top-k and hides superseded keys.
4. **Compaction:** `tessera compact <current-collection> → new base`.
   - Writes a new sealed base with `derived_from` edges to the old base and to every folded delta.
   - Reproducible, so the same inputs produce the same bytes.
   - Old generations stay until a **retention policy** (declared on the collection, for example "keep the last 2 bases") lets an operator remove them. Removal is explicit, never implicit, and consistent with ADR-0033's WORM roles.

## Acceptance

- [ ] A base of 10⁷ rows plus 30 deltas of about 10⁴ rows each (upserts, deletes, redirects): the logical read equals a brute-force replay, in bounded RSS.
- [ ] A `SELECT … WHERE key = K` reads the base and only the delta chunks whose key statistics cover K (instrumented).
- [ ] `compact` gives byte-identical output for identical inputs, and the new base's provenance names every folded delta.
- [ ] A delta against an undeclared key fails at seal with a clear error.

## Related

- Epic #778. #780 (keyed links, prerequisite), ADR-0022 / ADR-0036 (versioning, object store), ADR-0062 §6 / #668 (multi-product), ADR-0033 / ADR-0050 (collections, roles), ADR-0054 (per-member caches), ADR-0064.

---

# [Comment #1]() by [gerchowl]()

_Posted on November 9, 2026 at 03:32 AM_

**Owner ruling, 2026-11-09: one ADR decides both #781 and ADR-0036.** The #781 ADR must define delta products (upsert, tombstone, redirect, compaction, the `current` collection) in terms of ADR-0036's content-addressed objects and commits, or amend ADR-0036 explicitly. Tessera keeps one versioning story; a third, parallel one is ruled out. Review ADR-0036 (Proposed) first.

---

# [Comment #2]() by [gerchowl]()

_Posted on November 16, 2026 at 12:56 AM_

**Input for the #781 / ADR-0036 ADR: the leaf split rule and leaf naming after compaction** (agent, from the kit storage review [private-repo]#97, 2026-11-16).

kit wants the same mechanism as this issue for small, frequently edited keyed collections (tasks, edges, document blocks), so it reviewed how tessera's Merkle tree and ADR-0064's leaves behave under it. Two points for the ADR to decide. Neither asks to change streamed tables.

**1. Splitting by row count defeats ADR-0036's block sharing after a compaction.**
- Compaction writes a new base, partitioned by SPEC §5b at `BLOCK_ROWS`. An insert or delete at some key shifts the row-to-leaf assignment of every later row, so every later leaf gets a new digest and the new base shares nothing with the old one past that point. Updates in place do not shift anything.
- At `BLOCK_ROWS` = 4 Mi this hardly matters: a 10⁸-row base is about 24 leaves and a daily delta touches most of them anyway.
- It becomes the dominant cost when leaves are small (a small-row codec profile, see #869) or edits are clustered, which is kit's case. Then "the version should cost kilobytes" (ADR-0036's own motivation) is lost at every compaction.
- Option for keyed, sorted tables: a **declared key-defined split rule**, cutting after a row whose key hash matches a declared pattern, with a minimum and maximum row count. Boundaries then move only where rows changed. It is still a pure function of the rows and declared constants, so "same input, same bytes" holds, and a fully compacted table is canonical for its row set. Readers already accept leaves of unequal row counts (ADR-0064 §2.6).

**2. Positional leaf names and markers churn when a middle leaf splits or merges.**
- With a key-defined rule, a local edit can split or merge one leaf. Every later leaf keeps its payload digest but changes its name (`<table>/{index:04}`) and its sealed `Partition.index` (ADR-0064 §1, §2).
- The blocks stay shared, because the repository addresses them by digest. What churns is everything that names a leaf: the manifest entries, each `<table>/<leaf>.cidx` sidecar name, and #780's key → (product, leaf, row range) aux index.
- Question for the ADR: keep positional naming for such tables and accept the churn (with aux indexes referring to leaves by digest, not by name), or give a keyed partition its own marker, for example the leaf's key bounds, as an amendment to ADR-0064's marker and child-class scheme.

**Not affected:** `content_hash` stays the mountain range over the ordered block digests (ADR-0028); the tree stays integrity-only. A history-only-extended proof for a lineage would be the same construction one level up, over the sequence of `manifest_hash` values.


---

# [Comment #3]() by [gerchowl]()

_Posted on November 18, 2026 at 12:05 AM_

**Measurements for the #781 / ADR-0036 ADR: split rule, compaction and the keyed read** (agent, from the kit benchmark [private-repo]#98 for [private-repo]#97, 2026-11-16). This puts numbers on point 1 of the comment above.

**Setup.** A standalone harness, not tessera: sorted Vortex leaf files as base, delta files with an `op` column, a newest-row-wins read and a compaction that writes a new base. Vortex 0.93.0 from crates.io with default codecs, so neither the fork pin nor the canonical codec profile. 10⁶ synthetic rows of about 350 bytes (`u64` key, three small integers, a 40-byte and a 300-byte string). Leaves of 4,096 to 65,536 rows, far below `BLOCK_ROWS`. Every read was checked against an in-memory oracle with no mismatch. Byte counts are the evidence; timings are from a shared host with a warm page cache.

**1. Split rule after compaction.** Each delta changes one contiguous 1 % key window a quarter of the way into the key range. "Downstream" is the share of leaves after the last window that the new base still shares with the old one.

| Change per delta | Row-count split, downstream shared | Key-defined split, downstream shared |
|---|---:|---:|
| Inserts only | 0 % | 87.5–100 % |
| Deletes only | 0 % | 99–100 % |
| 60/30/10 updates/inserts/deletes | 0 % | 99–100 % |
| Equal inserts and deletes (net zero) | 100 % | 87.5–100 % |

- With one insert-only delta at 4,096-row leaves, the new base has 212 MiB of unshared leaves under the row-count rule and 7 MiB under the key-defined rule, for about 3.5 MiB of new rows.
- The key-defined rule tested: cut after a row where `blake3(key) mod target == 0`, with a minimum and a maximum row count. The bounds are why its share is not always 100 %: a forced cut depends on the previous one, and boundaries resynchronise at the next hash match.
- Updates in place do not shift anything under either rule, as stated above.

**2. Where sharing does not help, whatever the rule.** A uniformly scattered 1 % change, or a 90/10-skewed one, touches every leaf of 4,096 rows or more, and compaction rewrites the whole base (about 80 to 100 times the bytes changed, for one delta). Sharing after compaction pays only for clustered change. For scattered change the lever is how rarely compaction runs.

**3. Cost of the delta chain on the logical read.** A merge-on-read scan grows linearly with the number of deltas, since each is read whole: 3.7 MiB with none, 26 MiB through 8, 1.7 GiB through 600, at 2.9 MiB per delta of 10,000 rows. A delta costs about as many bytes as its rows; there is no write amplification before compaction.

**4. "Same inputs, same bytes" for `compact`.** In all 144 compaction cases the number of leaves shared by file hash equals the number shared by row content, so within one build the acceptance item holds on this path. The crates.io build writes different bytes from debug and release builds for every table shape tested (in one case a different file size). The harness did not test the fork pin, which #788 reports as validated across both.

**5. The keyed read on the base (acceptance item 2, and #780).** With the leaf open and its key column in memory, a row is found by binary search and fetched with `scan().with_row_indices(...)`.

| Read | Time per row | Bytes read per row |
|---|---:|---:|
| Full row, one at a time, default writer settings | 1.3–1.7 ms | 1.2–4.3 MiB |
| Full row, one at a time, 1,024-row blocks and 16 KiB segments | 0.5–0.8 ms | 0.5–1.3 MiB |
| Two small integer columns, one at a time | 170–290 µs | 13–944 KiB |
| Two small integer columns, 256 rows in one take | 8.7 µs | 14 KiB |
| SQLite, full row, same disk | 7 µs | 8 KiB |

- A take reads one whole segment per projected column. With the default 8,192-row blocks and 1 MiB minimum, the 300-byte string column has 2 MiB segments, and a narrow column in a 1M-row leaf is read whole (375 KiB for one 3-bit value).
- A single take costs about 110 µs even when it reads 249 bytes. This fixed cost has not been profiled yet.
- So "reads the base and only the delta chunks whose key statistics cover K" is met in chunk terms, but for small-row tables the chunk is the cost. The batched, narrow shape is the one `SPIKE-RESULTS.md` measured (500 rows in one take, numeric columns); the single-key full-row shape with wide strings was not measured there, and neither was a row store.

**Not measured:** the fork pin and the canonical codec profile; 10⁷ rows and above; real data (the strings are random and do not compress); cold reads; object storage; redirects; the #780 index itself.


