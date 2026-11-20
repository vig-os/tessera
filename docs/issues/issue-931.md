---
type: issue
state: open
created: 2026-11-19T21:13:25Z
updated: 2026-11-19T21:13:25Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/931
comments: 0
labels: chore, priority:low, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-20T09:39:08.081Z
---

# [Issue 931]: [chore(io): #929 follow-ups — stale to_sealed_json doc, aux-member door, one JCS-form test helper, derived corpus list](https://github.com/vig-os/tessera/issues/931)

Nits from the review of PR #929 (#912, merged). None block anything. Fold them into ADR-0066 step 4 (#914) where it's natural.

1. **`Manifest::to_sealed_json` docs are stale and the function is a trap.** Its rustdoc says "Every writer … goes through this", which is no longer true, and it still returns pretty JSON. A future writer that calls it directly gets the old non-JCS form back. Fix in #914: make it emit JCS, or remove it in favour of the stored-bytes API from #913.
2. **`add_aux_members` (`sign`, ingest provenance) keeps a pre-#912 product's pretty manifest bytes.** This is deliberate and documented, but it is a door that still writes non-JCS bytes. #914's cutover refuses such products anyway; make sure #914 covers this door.
3. **The same JCS-form test assertion exists three times** (cli `test_support`, `tessera-io/tests/stored_form.rs`, an inline closure in `tessera-ingest` `engine.rs`), with a fourth oracle in `smoke.py`. Share one helper.
4. **`every_tracked_tsra_stores_the_jcs_form` hard-codes its 13 files.** Derive the list from `git ls-files '*.tsra'`, or from corpus.json, so a fixture added later is covered too.

Refs: #897

