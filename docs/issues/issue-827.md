---
type: issue
state: open
created: 2026-11-11T03:27:22Z
updated: 2026-11-11T03:27:22Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/827
comments: 0
labels: documentation
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-11T08:50:29.114Z
---

# [Issue 827]: [docs(spikes): fix amendment labels in the 801 gap analysis §4.4](https://github.com/vig-os/tessera/issues/827)

Nits from the fresh re-review of #820 at c1f6205. Not blocking, filed so they aren't lost.

- In the §4.4 intro, 'Items 2 and 7 are proposed amendments' should also name item 3, which is headed as an amendment too.
- Item 3 says it amends 'the steer's item 2: the partition'. The #800 steer has no partition item; its item 2 is concurrency. The #802 summary comment repeats the mislabel ('the partition item').
- Line 394 was reflowed to 159 characters, unlike its neighbours, and its 'For them' now follows a sentence about sim2listmode only.

Fold this into the next edit of docs/spikes/801-consumer-gap-analysis.md (for example when the owner's surveys land).

Refs: #801
