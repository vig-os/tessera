---
type: issue
state: open
created: 2026-10-16T21:45:19Z
updated: 2026-10-16T21:45:19Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/574
comments: 0
labels: bug, area:core, audit-trail
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-17T06:32:45.064Z
---

# [Issue 574]: [bug(cli): publish and commit drop sealed producer, generation and ingest_transform via from_manifest](https://github.com/vig-os/tessera/issues/574)

## The defect

`ProductBuilder::from_manifest` (`tessera-core/src/product.rs:38-63`) rebuilds a manifest from the
parent's fields, but it carries only `study`, `schema`, `metadata`, `extra` and the non-`supersedes`
sources. It **drops** `producer`, `generation` and `ingest_transform`. Two verbs reseal through it:

- `tessera publish` (`tessera-cli/src/version.rs:377`) is documented as a history-free snapshot of
  the *same* version. The published artifact loses its generation recipe and its recorded transforms,
  and its producer is restamped as `tessera/0.0.0`, which is a false attribution under the original
  seal's author.
- `tessera commit --set/--add-block/--remove-block` (`version.rs:75`) carries the parent's blocks
  unchanged but drops the `ingest_transform` that says what those blocks' values *mean*
  (ADR-0056 §6: "a reader who cannot see it cannot reconstruct the source's semantics").
  ADR-0058 intends `producer` to be restamped on commit (`product.rs:49-51`). Dropping
  `generation` while the blocks it produced are kept is the same loss.

## Repro (probe against tessera-core at dev `fa581f5`)

```text
parent producer=Some("anatomist/0.3.0")
child  producer=Some("tessera/0.0.0")
parent generation keys=Some(["seed"]) child generation=None
parent ingest_transform=1 child ingest_transform=0
same id: true
```

(`ProductBuilder::new` → `with_producer`/`with_generation`/`with_ingest_transform` → `seal`, then
`ProductBuilder::from_manifest(&parent).seal()`.)

## Expected

`publish` reproduces the version's sealed provenance slots verbatim (it adds only the `snapshot_of`
edge). `commit` carries every slot that describes carried blocks (`ingest_transform`, and
`generation` unless the caller replaces it), and restamps only `producer`. Each slot has a test that
fails without the fix. ADR-0060 (#564) adds a fourth slot, `actors`, with the same carry rule, so the
rule should be stated once, next to `from_manifest`.

