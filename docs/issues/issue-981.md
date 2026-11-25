---
type: issue
state: open
created: 2026-11-25T01:53:41Z
updated: 2026-11-25T01:53:41Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/981
comments: 0
labels: chore, area:testing
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-25T10:51:45.121Z
---

# [Issue 981]: [test(corpus): a whole-archive golden that carries a derived block, and an exhaustive cross-check table](https://github.com/vig-os/tessera/issues/981)

From the fresh review of PR #980 (#916, merged):
1. No tracked `.tsra` in `corpus/files` carries a derived block (`"class":"derived"`, e.g. `<name>.cidx`), at either alpha.6 or dev. The #720 `content_hash` rule (primary blocks only) is therefore covered only by seal cases and the reference reader's in-memory checks, never by a whole sealed archive golden. Add one indexed product to the corpus (writer + `.cidx`) with its pinned `content_hash` and `manifest_hash`. Rust, the reference reader and wasm must agree on it.
2. The #916 cross-check table leaves out 5 fixtures present at both tags: `scan.tsra`, the two `study.tsra`, `legacy_flat_events.tsra` and `partitioned_pairs.tsra`. The review paired them and found no movement. Add them so the release-to-release claim is exhaustive.

Refs: #916
