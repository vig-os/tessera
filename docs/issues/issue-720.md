---
type: issue
state: open
created: 2026-10-30T02:08:08Z
updated: 2026-10-30T02:08:08Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/720
comments: 0
labels: bug, priority:high
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-30T08:02:13.255Z
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

