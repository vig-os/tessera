---
type: issue
state: open
created: 2026-11-21T02:13:13Z
updated: 2026-11-21T21:23:30Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/944
comments: 1
labels: feature, area:io, adr
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-22T09:19:44.802Z
---

# [Issue 944]: [feat(io): one provider for a keyed collection — statement snapshot, the visibility rule for deltas, declared semantics for combined graph, vector and relational queries (extends ADR-0062)](https://github.com/vig-os/tessera/issues/944)

Filed by an agent for the owner, 2026-11-21, from the kit's data-engine page (nerdmachines/kit `docs/architecture/data-engine.md`, §7 "The query surface"; nerdmachines/kit#97). Design request: an amendment to ADR-0062 or a new ADR. It asks for no code before #666–#668, #780 and #781.

## Motivation

kit keeps its state in keyed tessera collections and reads them through DataFusion. The owner's goal for that layout is one plan that chains relational, graph and vector steps at one version (nerdmachines/kit#97, "Why this one is central"). Each piece of such a read has an issue here. No issue or ADR says how the pieces form **one provider for one keyed collection at one version**, and three rules that such a provider needs are stated nowhere on this tracker.

## What already exists

- **ADR-0062** (accepted, not built; #654, #666–#668): a streaming provider over **sealed** table blocks. A Collection scan pins each member by `manifest_hash` and refuses unless every member has the identical schema (§0 row 6, §6).
- **#781** assigns the logical read to this provider: "**The logical read** (`#654` provider): over a current collection, return each key's newest row." Its acceptance prunes by key: "A `SELECT … WHERE key = K` reads the base and only the delta chunks whose key statistics cover K". For derived caches it says: "The query layer merges per-member top-k and hides superseded keys."
- **#903** item 4: "The DataFusion provider (#654) uses the same path for a keyed join or an `IN (…)` list on the declared key, in place of a filtered scan." **#780** item 4 exposes declared keys to the planner.
- **#910** asks for "A provider over that snapshot that plugs into ADR-0062's scan machinery", so that one query shape covers sealed and committed, unsealed rows.
- **#905** leaves open: "Who owns the in-memory view of unflushed commits that gives read-your-write: tessera's reader (#903) or the embedding application?"
- **#775** and **#778** have an acceptance item for filter + `bm25(...)` + `vec_topk(...)` in one SQL statement. **#776** has `neighbours(...)` and a docs example that chains graph, embeddings and `vec_topk`.

## The gap

1. **No pin for a whole statement.** ADR-0062 pins each member of one scan. A statement over a keyed collection runs several scans and table functions: base, delta products, a key index, `bm25`, `vec_topk`, `neighbours`. Nothing says that all of them read the same version of the `current` collection, or the same versions of two collections that are joined.
2. **A `current` collection is not scannable under §6 as written.** §6 refuses members whose schemas differ. If delta products carry what #781 describes (upsert, delete, redirect), their schema differs from the base. #781 gives the plan for the logical read, and ADR-0062 has no place for it yet.
3. **Pruning deltas by value can return an overwritten row.** Take a base row `(k, state = open)` and a newer delta row `(k, state = done)`, and the filter `state = 'open'`. If the delta chunk is pruned because its statistics exclude `open`, the base row passes the filter and is returned, although it is superseded. ADR-0062's `Inexact` re-evaluation does not catch this, because the base row really matches. So for a `current` collection, delta members may be pruned by **key** statistics (as #781's acceptance does), and by value statistics only after the newest row per key is resolved, or where the provider can prove the result is the same. Base members can still be pruned by value. This rule is in kit's record (nerdmachines/kit#97, correctness gate: "A lagging projection or pruning before visibility is resolved must never return an overwritten or deleted value"). It is not on this tracker.
4. **"Top-k per member, then hide superseded keys" changes the answer.** Hiding superseded keys after each member's top-k is cut can return fewer than k rows, or miss a row that would have ranked. Whether superseded keys are removed before or after the cut has to be stated per function.
5. **The semantics of a combined query are not stated.** These three shapes return different rows and are not rewrites of each other:
   - *eligible-set top-k:* apply the relational and graph constraints, score everything eligible, then take the best k;
   - *seed-then-expand:* take the k nearest, then traverse from them and join;
   - *expand-then-rank:* traverse to candidates, then rank them.

   #775 and #778 test filter + BM25 + vector without naming the shape. No acceptance item runs a graph step, a vector step and a relational step in one statement.
6. **Three row sources, no one table.** Sealed base and deltas (#781), committed and unsealed fragments (#910), and unflushed commits (#905, owner open) are three read paths. For a keyed collection with an open writer, kit needs them as one table at one watermark, with newest-row-wins across all three.

## Proposal

1. **A statement snapshot.** At admission, resolve every `current` collection the statement names to its pinned members, once. Every scan, keyed read and table function of that statement uses that resolution. This extends §6's member pin from one scan to one statement.
2. **A provider for a keyed collection** that composes what the other issues define, and adds nothing to their formats:
   - the newest-row-wins read of #781 over base and delta products;
   - the keyed path of #903 when the predicate is on the declared key (#780);
   - when a write session is open, the committed, unsealed snapshot of #910 at one watermark;
   - an optional source of unflushed rows, whose owner is #905's open question. If the answer there is "the embedding application", this is the interface point: the embedder hands in Arrow batches with the same key and schema, and the provider applies the same newest-row-wins rule.
3. **The visibility rule, written into the ADR** with a conformance fixture: value-dependent pruning of a delta member happens only after the newest row per key is resolved, or where the provider proves equivalence; key-statistics pruning of deltas and value pruning of the base stay allowed.
4. **Declared semantics per table function.** For `vec_topk`, `bm25` and `neighbours`: whether filters are applied before the ranking (eligible set) or after; that superseded and deleted keys are removed before the top-k cut; and that the planner does not rewrite one of the three shapes into another.
5. **One fixture that runs all three kinds of step** in one statement at one pin, checked against a brute-force oracle for each of the three shapes, with deletes, ties and several paths to the same row.

Sequencing is the owner's. By the order of work on #778 this sits after #666–#668, #780 and #781; items 1, 3 and 4 are text for those designs and could be settled before any of this is built.

## Acceptance

- [ ] ADR-0062 is amended, or a new ADR is accepted, that defines the statement snapshot and says which reads of a statement it covers.
- [ ] A scan over a #781 `current` collection is defined in the provider (schema of delta members, the newest-row-wins plan), and §6's identical-schema refusal says how it relates to it.
- [ ] The visibility rule is in the ADR. A fixture with an overwritten row and a deleted row, a value filter that matches only the old row, and pruning on, returns neither.
- [ ] `vec_topk`, `bm25` and `neighbours` each document filter order and when superseded keys are removed. A fixture shows that k rows are returned when k eligible rows exist, with deltas present.
- [ ] One statement with a relational filter, a traversal and a vector ranking, at one pin, equals the oracle for eligible-set top-k, for seed-then-expand and for expand-then-rank.
- [ ] With a write session open, one statement returns sealed rows, committed and unsealed rows and (if #905 assigns it here) unflushed rows as one table, newest row per key, at one watermark.
- [ ] A statement that joins two keyed collections reads one version of each, also while a flush or a compaction publishes a new `current` in between.

## Open

- Who owns the in-memory view of unflushed commits (#905). The proposal above works under either answer.
- Whether a statement snapshot may span collections with different writers, and what "one version" means then (kit's cross-collection rules, nerdmachines/kit#97).
- The memory bound of the newest-per-key step. ADR-0062 §3 bounds the scan; operators above it are bounded only by DataFusion's memory pool.
- Whether the three shapes are separate table functions, an argument, or documented SQL patterns.

## Not measured

Nothing here is measured on tessera. From kit's harness on vanilla Vortex 0.93.0 (nerdmachines/kit#98, posted on #781): a scan through uncompacted deltas grows linearly with their number, 94 ms through 8 and 7.5 s through 600, because each is read whole. That is the cost the delta chain limit of #904 bounds.

## Related

ADR-0062 / #654, #666, #667, #668 (provider); #780 (keys); #781 (deltas, logical read); #903 (keyed read); #904 (profile, chain limit); #905 (commit log); #910 (unsealed fragments); #908, #794 (taps); #768, #769 (vectors); #775 (full text); #776 (graph); #778 (order of work, hybrid acceptance); ADR-0054 (derived caches). kit side: nerdmachines/kit#97, nerdmachines/kit#74, nerdmachines/kit `docs/architecture/data-engine.md` §5 to §8. The three shapes and the visibility rule are taken from the design inputs preserved in nerdmachines/kit `docs/sources/unified-engine/` (spec pack: "Three distinct hybrid queries", INV-08).

---

# [Comment #1]() by [gerchowl]()

_Posted on November 21, 2026 at 09:23 PM_

**One more read for the provider: a version plus a delta the caller supplies** (agent for the owner, 2026-11-21; from nerdmachines/kit#214, the document slice's requirements).

A kit consumer needs a **dry-run merge**: compute the merged rows in memory, then run checks on the state the merge would produce (no dangling references, which signed subtrees it touches) before anything is written. The same read serves a workspace whose changes are held as rows in another collection.

Request: the statement snapshot of this issue may include, per collection, **one extra delta that is not committed**: record batches in the collection's schema with the `op` column of #781, supplied by the caller for that statement only. The visibility rule is the one already defined here; the extra delta is the newest member. Nothing is written, and no other statement sees it.

This is close to #910's provider over committed, unsealed rows; the difference is the source (the caller, not the writer) and the lifetime (one statement). If one mechanism can serve both, it should.

Acceptance to add:
- [ ] A query over version V with an extra delta returns the same rows as the same query after that delta is committed on top of V.
- [ ] Key pruning still applies to the sealed members (instrumented).
- [ ] Derived caches (text, vector, graph) either correct for the extra delta's keys or refuse; by the visibility rule of this issue (gap 3) they never return a row the extra delta overwrites or deletes.

Open: a size bound for the extra delta, since it is held in memory.


