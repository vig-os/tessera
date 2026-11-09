---
type: issue
state: open
created: 2026-10-30T02:08:08Z
updated: 2026-11-09T03:32:41Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/720
comments: 2
labels: bug, priority:high
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-09T09:04:26.886Z
---

# [Issue 720]: [bug(core): derived-sidecar blocks are folded into content_hash, contradicting Accepted ADR-0028 §4](https://github.com/vig-os/tessera/issues/720)

## Defect
ADR-0028 §4 (**Accepted**, `docs/adr/0028-unified-hierarchy.md` ~:67-84): derived-sidecar blocks (stats, pyramids,
projections, the chunk index's stat columns) are "**NOT in the canonical identity** … regenerating a thumbnail must not
change `content_hash`", using a manifest `canonical | derived` tag.

The code does not do this. `ManifestBuilder::seal` (`tessera-core/src/product.rs` ~:172-182 on dev 4504efb) pushes
**every** block ref's digest into `merkle_root(&digests)`, with no class filter. There is no `canonical | derived`
class on the block type in `tessera-core` at all. ADR-0059 (Proposed) then records the as-built behaviour
("attaching a `.cidx` … `content_hash = merkle_root(&digests)` over *every* block ref, so default-on … moves
`content_hash`"). An Accepted ADR and the code therefore disagree about what identity is.

Reported by the visionx-rs architect in #719 (write-time aggregation views as derived blocks), which depends on it:
derived views cannot be added or regenerated without changing identity.

## Consequences
- Adding or regenerating any accelerator (`.cidx`, a pyramid level such as `volume/1…`, a stats block) changes
  `content_hash`. Two products with identical canonical data but different accelerators get different identities.
- Every new derived block kind is a corpus regeneration.

## Fix path (design first, sealed-format change)
This changes the sealed format and identity, so it needs an ADR decision before code. The proposal in #719 is to make
it part of ADR-0059's single format event. That ADR must decide:
- the manifest representation of the class;
- whether `manifest_hash` still commits to derived blocks, which it should: integrity of the accelerators remains;
- the version and migration of existing products (ADR-0052);
- the corpus regeneration.

Then implement, with a test that fails today: regenerating or adding a derived block leaves `content_hash` unchanged,
and changing a canonical block changes it. Until then, either ADR-0028 §4 or the code is wrong. Do not paper over it
in one place.

---

# [Comment #1]() by [gerchowl]()

_Posted on November 3, 2026 at 12:58 PM_

Scope for the ADR (owner discussion 2026-11-03): make the hash topology a single declared definition in the format, not code scattered per type and not part of the per-product schema.

**Proposed components** (one root each, in one format event with ADR-0059):
- `content_hash`: canonical blocks.
- `derived_hash`: derived blocks, each carrying `from` and `recipe`.
- `schema_hash`: the embedded schema, as JCS.
- `metadata_hash`: descriptive metadata and provenance, as JCS. A separate root allows selective proofs without disclosing PHI (ADR-0040).
- `manifest_hash`: a root over the four above plus `id` and the version fields. This is what the signature signs. Today it is a flat digest of the whole canonical JSON.

**State on dev (3ebb40a):**
- The primitives are DRY: one `hash::digest`, and one `merkle_root` / `inclusion_proof` with `0x00`/`0x01` domains.
- The composition is not DRY:
  - `Manifest::recompute_content_hash` hashes block digests in manifest order, with no class or kind tag on the leaf.
  - `Collection::recompute_content_hash` uses its own `tessera-collection-member-v1` prefix plus a kind byte.
  - `ChunkIndex::root` hashes in index order.
  - `compute_manifest_hash` is duplicated in `manifest.rs` and `collection.rs`.
  - Domain separation is inconsistent across the three.

**Decide in the ADR:**
1. **One declarative identity spec per format version** (components, leaf domain tags, ordering rule, root composition), evaluated by one generic composer used by product, collection and chunk index. Old specs stay in the reader so old products verify forever (ADR-0052 migration = a new entry).
2. **The product schema must not define identity.** It may only classify a block kind as derived within format rules (it must carry `from` and `recipe` and be recomputable); everything else is canonical. Otherwise a schema could remove real data from `content_hash`.
3. **Leaf ordering.** Today it is manifest order, and `merkle_is_order_sensitive` pins it, so the same blocks written in a different order give a different `content_hash`. Decide on a canonical order (e.g. by block name, which must stay consistent with partition leaf order `<table>/NNNN`) or keep writer order deliberately, with the reason.
4. **What each hash vouches for**, stated in one table in SPEC. In particular, `content_hash` does not vouch for accelerators; readers that use an index must verify `derived_hash` or `manifest_hash`.

---

# [Comment #2]() by [gerchowl]()

_Posted on November 9, 2026 at 03:32 AM_

Scheduling note (2026-11-09): the #778 epic's sealed column carriers, #768 (declared vector) and #779 (declared media), should be folded into this ADR-0059 format event, not shipped as separate point changes. Several #778 children (#775, #776, #769 tier 2, #782) depend on this issue's derived-block identity exclusion.

