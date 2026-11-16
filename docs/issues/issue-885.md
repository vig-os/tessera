---
type: issue
state: open
created: 2026-11-16T06:01:50Z
updated: 2026-11-16T06:01:50Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/885
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-16T08:59:33.691Z
---

# [Issue 885]: [docs(io): #884 nits — codec_profile rustdoc omits sequenced leaves; drop the duplicate profile assert](https://github.com/vig-os/tessera/issues/885)

Follow-up nits from the #884 (#869) review.

- `TableSpec::codec_profile` rustdoc (`tessera-core/src/block/table.rs` ~:203) says "every table block and partition leaf" but leaves out sequenced leaves, which the CHANGELOG and ADR-0024 amendment include.
- The drift test's second assert (`TABLE_CODEC_PROFILE == PINNED_PROFILE`) duplicates `table_edition_constant_is_the_recorded_name`; the real guard is the compressor-Debug fingerprint assert. Drop or fold it.

Refs: #869
