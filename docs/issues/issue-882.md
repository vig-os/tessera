---
type: issue
state: open
created: 2026-11-16T02:35:19Z
updated: 2026-11-16T02:35:19Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/882
comments: 0
labels: bug
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-16T08:59:34.218Z
---

# [Issue 882]: [test: pin that two distinct block names may share one digest (#879 nit)](https://github.com/vig-os/tessera/issues/882)

Follow-up nit from the #879 (#734) re-review.

ADR-0063 §2a and SPEC §5 now state normatively that two distinct block names may share one content digest (only duplicate NAMES are refused). No test pins that, so a future change could refuse it by mistake.

Add:
- seal → pack → `Reader::open` → read both names, for a product where two names share one digest;
- `Repository::commit` with two names on one digest succeeds and stores the payload once.

Refs: #734
