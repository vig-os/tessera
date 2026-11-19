---
type: issue
state: open
created: 2026-11-19T05:25:30Z
updated: 2026-11-19T05:25:30Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/918
comments: 0
labels: docs, priority:low, area:docs
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-19T09:52:02.143Z
---

# [Issue 918]: [docs(adr): ADR-0066 version-bump wording + same-layout future breaks (review nits from #900)](https://github.com/vig-os/tessera/issues/918)

Follow-up nits from the final review of #900 (ADR-0066 at 9483b64).

1. ADR-0066 lines 106–107 say an optional format-version bump "belongs in step 8's regeneration". But step 8 counts any difference not traced to step 1, 5 or 7 as a defect. A bump would be a fourth named cause, and the text should say so.
2. Rule 10's reason (c), that the reader tells layouts apart by layout rather than by the version string, works only because this break changes the layout. A future pre-1.0 break that keeps the layout would leave nothing to tell old from new. Add a sentence: such a break must bump the version string, or must itself change the layout.

