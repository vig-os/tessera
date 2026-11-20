---
type: issue
state: open
created: 2026-11-20T08:07:11Z
updated: 2026-11-20T08:07:11Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/937
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-20T09:39:07.007Z
---

# [Issue 937]: [nits from the #934 review (schema number rules)](https://github.com/vig-os/tessera/issues/937)

Follow-ups from the round-2 review of PR #934 (#895):
- `dicom.rs` scout test: also check that a very large `slice_thickness` (e.g. 1e6) is accepted. The maximum has been removed, but the test covers only ~1300.
- `docs/adr/0020-…`: one new line runs to 170 characters (the file already has others over 120).

Refs: #895
