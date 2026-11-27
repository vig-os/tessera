---
type: issue
state: open
created: 2026-11-26T16:33:39Z
updated: 2026-11-26T16:33:39Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/992
comments: 0
labels: chore, docs, area:ingest
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-27T10:31:05.046Z
---

# [Issue 992]: [nits from the #991 review (linked decoder version)](https://github.com/vig-os/tessera/issues/992)

From the fresh review of PR #991 (#979, merged):
1. `tessera/crates/tessera-ingest/src/decoder.rs:44` adds a 158-character doc line, over the 120 limit.
2. `linked.rs`, the ADR-0056 amendment and `build.rs` say that a packaged build records only the decoder's name "exactly as before #979". That is not accurate. `cargo package --list -p tessera-ingest` includes `Cargo.lock`, and the old `find_lockfile` walked up and found it, so packaged builds used to seal versions. The crate is not published, so this affects nothing today; fix the wording.
3. `docs/adr/0056-…md:489` wraps raggedly ("tracked in #989. The lockfile").
4. Documented limit, for the record: `features` still comes from lockfile and toml text (the lockfile closure filtered by `IN_DIGEST`), not from the linked crate's enabled features. It is now keyed in the right compile unit. Decide whether the linked features are needed for ADR-0056 §6a truthfulness.

Refs: #979
