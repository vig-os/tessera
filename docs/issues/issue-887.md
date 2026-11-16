---
type: issue
state: open
created: 2026-11-16T07:18:44Z
updated: 2026-11-16T07:18:44Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/887
comments: 0
labels: bug
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-16T08:59:32.586Z
---

# [Issue 887]: [cli: `collection ls` and `provenance --collection` read a collection without verifying its seal](https://github.com/vig-os/tessera/issues/887)

Found by the #883 (#873) adversarial review.

`tessera collection ls` and `provenance --collection` (`crates/tessera-cli/src/provenance.rs` ~:211) parse a collection without `Collection::from_json_verified`. This predates #883 and affects only display and path resolution, but a tampered collection is shown and followed as if it were valid. Route both through the verified parse (and surface newer fields like `inspect` does).

Refs: #873
