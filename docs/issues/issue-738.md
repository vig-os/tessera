---
type: issue
state: open
created: 2026-11-02T16:11:48Z
updated: 2026-11-02T16:11:48Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/738
comments: 0
labels: docs, priority:low, area:testing
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-03T08:59:44.697Z
---

# [Issue 738]: [corpus: keep a full-bitmap arrow-IPC fixture under test; record the arrow 58.4.1 decoder event in ADR-0056/0062](https://github.com/vig-os/tessera/issues/738)

Follow-up nits from the review of PR #737 (#736), head 3af42bf.

1. **Lost coverage.** arrow-ipc 58.4.1's writer (`writer.rs:~1828`) writes a 0-length validity buffer for columns with `null_count == 0`, where it used to write a full all-valid bitmap. The corpus IPC fixture is generated at build time, so it now covers only the new layout. Files written by older arrow writers have the old layout, and nothing tests reading them. Add a committed (or byte-constructed) IPC fixture with an explicit all-valid bitmap, and assert it ingests to the same `content_hash` as the 0-length form. Show it fails if the ingest path mishandled either form.
2. **Stale docs.** `docs/adr/0062-streaming-table-provider.md:18,204` still describes the pin as arrow `=58.3.0`.
3. **Missing ADR note.** No ADR-0056 note records the decoder-digest and source-digest move for 58.4.1. The zip precedent (#588) amended ADR-0056/0057. Add the same kind of amendment line, and say whether this should become a standing convention.

Refs: #736
