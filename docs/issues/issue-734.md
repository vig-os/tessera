---
type: issue
state: open
created: 2026-11-02T12:27:20Z
updated: 2026-11-02T12:27:46Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/734
comments: 0
labels: bug
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-03T08:59:45.756Z
---

# [Issue 734]: [core(name): duplicate block names are not refused at seal; ADR-0063 does not state uniqueness](https://github.com/vig-os/tessera/issues/734)

Follow-up from #733 item 3.

**Finding.** ADR-0063 does not state that block names are unique within a product, and `ProductBuilder::seal`
does not refuse duplicate names (`tessera-core/src/product.rs:175-177` runs the leaf rule and ADR-0064's
`check_partitions`, neither of which looks at duplicates; `check_leaf_rule`'s doc says "Duplicates are not this
rule's concern", `tessera-core/src/block/name.rs:164`). The write session refuses a duplicate (`write.rs:780`,
"already committed", and `recover`'s "committed a second time"), so only the batch builder and hand-sealed
manifests can carry one.

**Why it matters.**
- ADR-0063 §2's premise is that zip members, the exploded directory and an S3 prefix are one-to-one with the
  names. A duplicate breaks that premise, and today it surfaces late and untyped: the seal succeeds, then `pack`
  fails inside the zip writer (zip 9.0.4 refuses a second `blocks/<name>` entry with "Duplicate filename"), after
  the manifest has been sealed and returned. A manifest carrying a duplicate (hand-built, or packed by a tool
  that writes zips differently) makes `Reader::read_block` check whichever entry the zip lookup finds against the
  manifest's *first* matching digest (`container.rs`, `find(|b| b.name == name)`).
- ADR-0064's `LogicalTableError::DuplicateIndex` is reachable only through two blocks of the same name (a
  marker's index fixes its leaf's canonical name), so it guards a state the seal could refuse at its source.

**Proposed.** An ADR-0063 amendment stating that block names are unique in a product, enforced where a set of
names is finalised (ADR-0063 §4: `ProductBuilder::seal`, a write session's seal, `unpack`, `pack_dir`) with a
typed error, plus a negative test per door. Readers keep accepting what they accept today, as for the leaf rule.
Whether `DuplicateIndex` then stays as a defensive variant for hand-sealed products is part of the amendment.

Refs: #733

