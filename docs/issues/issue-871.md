---
type: issue
state: open
created: 2026-11-15T15:39:56Z
updated: 2026-11-15T15:39:56Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/871
comments: 0
labels: feature, effort:small, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-16T08:59:38.570Z
---

# [Issue 871]: [Reader: name the Vortex edition when a table block can't be decoded](https://github.com/vig-os/tessera/issues/871)

Found while documenting the forward-read limit of the vortex 0.93 format event (#635, PR #788).

A tessera reader built before #635 (vortex 0.75 fork; dev before it, 0.1.0-alpha.1 to alpha.5) verifies a product whose table blocks were written at `TABLE_EDITION = core2026.08.3`, then fails the table decode with an opaque error:

```
$ tessera-0.1.0-alpha.5 read corpus/files/listmode_events.tsra events
error: codec: Other error: Encoding not found in registry: 1   # exit 1
```

That limit is now documented (SPEC §5b, ADR-0024, CHANGELOG). Old readers can't be changed. But the next edition move hits the same failure in today's reader: a block sealed with an edition this reader's Vortex doesn't know fails with the same registry error, even though the manifest names the edition (`TableSpec.edition`).

Proposal: before it decodes a table block, `tessera-io` compares `spec.edition` with the editions its pinned Vortex knows. For an edition it doesn't know, it returns a typed error that names the edition and says a newer tessera reader is needed. A block without an edition (pre-#635) is unaffected. `verify` stays as it is (integrity doesn't need decoding).

Acceptance: a test with a spec naming an unknown edition gets the typed error, not `Encoding not found in registry`.
