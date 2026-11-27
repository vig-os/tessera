---
type: issue
state: closed
created: 2026-11-26T17:44:01Z
updated: 2026-11-27T04:44:40Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/994
comments: 0
labels: bug, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-27T10:31:04.516Z
---

# [Issue 994]: [fix: collection member files still use the non-injective ':'→'_' name mapping](https://github.com/vig-os/tessera/issues/994)

Found in the review of PR #990 (#975). #975 makes lineage → ref file names injective (percent-encoding, never `:`). Collections still map member names to file names with the same colliding `:`/`/`/`\\` → `_` replacement: `tessera-io/src/collection.rs:97` and `tessera-ingest/src/engine.rs:1801`. Two members such as `a:b` and `a_b` (or ids containing `:`, which real ids do) therefore get the same file, and one overwrites the other.

Expected: one injective encoder shared with #975's `lineage_file_name`, never two mappings. Start with a failing test that writes two members whose names collide under the old mapping and reads both back. ADR-0067 (collection refs, gc through members) depends on members being addressable, so this should land before ADR-0067's implementation.

Refs: #975
