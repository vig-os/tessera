---
type: issue
state: open
created: 2026-11-21T20:38:54Z
updated: 2026-11-26T20:41:18Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/947
comments: 2
labels: feature, area:core, area:io, adr
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-27T10:31:09.564Z
---

# [Issue 947]: [feat(repo): refs to collections, a version edge on collections, and gc that marks through members — so a current collection pins its base and a second lineage can share it](https://github.com/vig-os/tessera/issues/947)

Filed by an agent for the owner, 2026-11-21, from the kit's storage review (nerdmachines/kit#214, nerdmachines/kit#97). Feature request; it belongs with the #781 / ADR-0036 ADR. From reading `dev` at `4863a78`; nothing was built or run.

## Motivation

kit needs workspaces: a person or an agent works on a private copy of a keyed collection and the result is merged back. The owner's direction of 2026-11-21 is that branches live in the kit ("id guess branches live in the kit, file the tsra requests"). tessera's rule stays as it is: no branches and no content merge (ADR-0036).

With that split, kit can build a workspace from parts tessera already plans. A workspace is a **second lineage of `current` collections** (#781) whose first version pins the same base and deltas as the main lineage at the fork. Fork, conflict check and merge are kit code; a merge writes one new delta on the main lineage. tessera only has to make a `current` collection a first-class citizen of the repository, which it is not today.

## What already exists

- **#781:** a `current` collection pins `{base, deltas[...]}` by `manifest_hash`; "Advancing it seals a new tiny collection; nothing else changes."
- **ADR-0036 (Proposed):** `refs/<lineage>` is the only mutable state; a commit advances it by compare-and-set; objects are shared "across every version *and* every product in the repo". "Branch / merge are explicitly out of scope. … `refs` are single-lineage pointers, not branches."
- **`tessera-io/src/repo.rs`:** `commit` takes a product `Manifest`; `get_manifest` parses a product manifest; `gc` marks a manifest's `blocks` and `sources` (`repo.rs:395-406`). The word "collection" does not occur in the module.
- **`tessera-core/src/collection.rs`:** a `Collection` has `id`, `id_inputs`, `members` (each pinned by `manifest_hash`, with `derived_from` between members), `content_hash` and `manifest_hash`. It has no edge to an earlier version of itself.
- **#292** lists as child work a "Collection ref-model + custodian verbs" (`collection outdated`, `rebind` giving "a new collection version with a diff/consistency proof").
- **#946** reports that the ref compare-and-set is not atomic; item 6 there is this gap in one sentence.

## The gap

1. A ref cannot point at a collection, so `current` has no mutable pointer and no compare-and-set.
2. A collection has no version chain: nothing in a sealed collection names the collection version it replaces.
3. `gc` does not mark through collection members. A base and its deltas that only a `current` collection pins are unreachable and would be reclaimed.
4. Nothing lets a collection version name a version in **another** lineage as its origin, so two lineages that share a base have no recorded common ancestor.

## Proposal

1. **Collections are repository objects.** A sealed collection is stored at its `manifest_hash`, and `refs/<lineage>` may name it. `commit` for a collection takes the expected parent and follows the rules of #946.
2. **A version edge on collections.** A collection version names the version it replaces (`supersedes`, pinned by `manifest_hash`), as products do under ADR-0022.
3. **An origin edge across lineages.** A collection version may name a version of another lineage as `derived_from`. tessera records it and `gc` and provenance walks follow it; tessera gives it no merge meaning.
4. **`gc` marks through members,** recursively for sub-collections (ADR-0049), then through each member's blocks and sources. Reachability is repository-wide: an object is kept while any ref of any lineage reaches it. Dropping a workspace is `forget` on its lineage.
5. **A member diff as a pure function:** for two collection versions, the members added, removed and re-pinned. kit's merge and #292's `rebind` both need it.
6. **Unchanged:** no branch, no merge, no staging in tessera. Two lineages that share objects are two lineages.

## Acceptance

- [ ] A `current` collection is committed, advanced with compare-and-set, and read back through its ref.
- [ ] After `gc`, a base and deltas pinned only by a `current` collection are still there and read back by hash.
- [ ] Two lineages pin the same base. `forget` on one, then `gc`: the base stays. `forget` on both, then `gc`: it is reclaimed (subject to retention, #906, and WORM holds).
- [ ] The first version of the second lineage carries the origin edge, and a walk from it reaches the first lineage's version at the fork.
- [ ] `diff` of two collection versions lists exactly the members that differ.
- [ ] ADR-0036's sentence on branches is restated with this case: shared objects and an origin edge are allowed, a merge is not tessera's.

## Open

- **The `id` of a forked lineage.** A collection's `id` comes from `id_inputs` (by default name and timestamp). What a workspace lineage uses is to be decided; it must not collide with the main lineage.
- **Retention across a fork.** A retention class is declared per collection (#906), reachability is per repository. A workspace that pins an old base keeps it alive past the main lineage's `latest-only`. That is intended; a class may want an upper bound on how long a workspace may hold a generation.
- **Delta order across lineages:** see the comment on #781 (the sealed `seq`).
- Whether `supersedes` on a collection is part of `id_inputs` or only of the seal.

## Related

#781, #946, #906, #292, #288, #945; ADR-0036, ADR-0022, ADR-0033, ADR-0049, ADR-0050. nerdmachines/kit#214 (where branches live), nerdmachines/kit#97.

---

# [Comment #1]() by [gerchowl]()

_Posted on November 21, 2026 at 09:23 PM_

**Three additions from a kit consumer** (agent for the owner, 2026-11-21; from nerdmachines/kit#214, the document slice's requirements: tens of short-lived workspaces per document, long-lived variants merged in both directions, offline work as a workspace on another node).

1. **Durable pins: a ref that is not a lineage tip.** A three-way merge needs the common ancestor's rows. For long-lived variants that ancestor is an old version of either lineage, and it must stay readable across compactions. Request: a pin namespace beside `refs/<lineage>` (for example `refs/pins/<name>`) that names any collection version; `gc` treats a pin as a root; removing a pin is explicit. Fork points, merge bases, long-running readers and release points all use it. Without it the kit would fake a pin with an empty lineage.

2. **Lineage ids that are unique across nodes.** A workspace made offline on another node arrives later by sync. Its `id` must not collide with a lineage made elsewhere, and must not depend on a clock alone (the default `id_inputs` are name and timestamp). This sharpens the first Open item above.

3. **The cost of dropping many short-lived lineages.** `gc` walks every ref and lists every object in the repository. With tens of workspaces per document created and dropped per day, discard needs a cost that follows what the workspace wrote, not the size of the repository. Two ways to get there: `forget` returns the objects that only this lineage reached (reclaim on forget), or a workspace writes into its own object area that falls back to the main one for reads and is dropped as a whole. A measurement should choose.

Acceptance to add:
- [ ] A pinned version reads back after its lineage has compacted and `gc` has run; after the pin is removed and `gc` runs, it is reclaimed.
- [ ] Creating and dropping 1,000 workspaces of about 200 rows each in a repository of 10⁷ rows: the time and bytes of discard are reported and do not grow with the repository.


---

# [Comment #2]() by [gerchowl]()

_Posted on November 26, 2026 at 08:41 PM_

ADR-0067 is Accepted (owner, 2026-11-26; PR #988 merged as 4f569cf8). Each implementation step has its own issue:
- #996: layout 2 with typed refs
- #997: `sources` on Collection
- #998: kind discriminator and member diff
- #999: collections in the repo, with gc through members
- #1000: `commit_collection`, log and history
- #1001: pins
- #1002: CLI
- #1003: ADR amendments

Steps 1–3 can land in any order, 4 needs all of them, 5 and 6 need 4, 7 needs 5 and 6, and 8 comes last. #994 (collection member file names are not injective) should land before step 4. #947 stays open until step 8 lands.

