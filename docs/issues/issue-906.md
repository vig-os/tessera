---
type: issue
state: open
created: 2026-11-18T20:04:31Z
updated: 2026-11-18T21:19:20Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/906
comments: 0
labels: feature, area:core, adr
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-19T09:52:08.708Z
---

# [Issue 906]: [feat(repo): declared retention classes for keyed collections — make superseded bases and deltas reclaimable at compaction](https://github.com/vig-os/tessera/issues/906)

Filed by an agent for the owner, 2026-11-18, from the kit storage review (nerdmachines/kit#97). Feature request. It belongs with the #781 / ADR-0036 ADR.

## Motivation

kit keeps app state in keyed tessera collections. Some of it changes often and only its newest value matters: window sizes, the last open panel, an agent's cursor. Keeping every superseded generation forever makes storage grow with the number of changes, not with the size of the state. In the kit harness one delta of 10,000 rows of about 350 bytes was 2.9 MiB.

## What already exists

- #781, point 4: old generations stay until a retention policy declared on the collection ("keep the last 2 bases") lets an operator remove them. Removal is explicit and consistent with ADR-0033's WORM roles.
- ADR-0036 lists garbage collection of unreachable objects as still open.
- `commit` pins the previous version's `content_hash` in its `supersedes` edge, and `gc` treats every such hash as reachable. So a superseded base never becomes unreachable. `forget` drops a whole lineage.
- #591 proposes an edge that keeps a `manifest_hash` but drops the `content_hash` pin, for erasure.

## The gap

A retention policy can be declared (#781) but nothing can act on it: no verb makes one superseded generation reclaimable while the lineage lives on.

## Proposal

1. **A retention class declared on the collection and sealed in its schema:**
   - `keep-all`, the default and today's behaviour;
   - `keep-last(n)`, the newest n bases with their deltas;
   - `latest-only`, the current base and the deltas not yet folded.
2. **Compaction applies the class.** For a generation outside the class, the new base's edge to it keeps the `manifest_hash` and does not pin the content. This looks like the same edge as #591; one mechanism should serve both.
3. **`gc` reclaims what is no longer pinned.** Removal stays an explicit verb, which an embedding application may schedule. It refuses under a WORM hold and for `Role::Raw`.
4. **The chain of `manifest_hash` values stays verifiable** after removal, so "this state descends from that one" can still be shown without the old bytes.

## Acceptance

- [ ] After N compactions of a `latest-only` collection followed by `gc`, the store holds the current base and its live deltas only (measured bytes), and the logical read is unchanged.
- [ ] A `keep-all` collection is untouched by the same sequence.
- [ ] `gc` refuses to reclaim an object under a WORM hold, with a clear error.
- [ ] The lineage's sequence of manifest hashes verifies after removal.
- [ ] A class cannot be weakened on an existing collection without a new sealed collection version that says so.

## Open

- Is the edge the same as #591's, or do erasure and retention need different audit records?
- Do signed checkpoints (#795) or counter-signatures have to survive for a removed generation?
- Does `latest-only` conflict with any consumer of #781 that relies on reading an old base by hash?
- Two inputs from the kit's record that a class has to respect: a generation may only be reclaimed once an upstream copy has acknowledged it (the kit's DAQ pruning rule), and some collections need a minimum retention and a data class (audit rows that name patients: 7 years in one consumer). So a class is a lower bound as well as an upper one.
- The kit builds undo as a parent version, so a class also bounds undo depth. Should the class say how many versions stay readable, not only how many bases?

## Related

#781 (retention policy line, compaction), #904 (profile), #905 (commit log), ADR-0036 (gc open), ADR-0033 (WORM roles), #591 (redacting commit), #795.

