---
type: issue
state: open
created: 2026-11-09T03:32:34Z
updated: 2026-11-09T03:32:34Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/783
comments: 0
labels: feature
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-09T09:04:19.903Z
---

# [Issue 783]: [feat(cli): optional Hugging Face publication helper for .tsra (dataset card, upload) — split from #777](https://github.com/vig-os/tessera/issues/783)

Split from #777 per the owner ruling of 2026-11-09. The original driver (scitadel-hub) withdrew. This is the optional helper for other tessera users: generate an HF dataset card from the product's RO-Crate/DataCite metadata and upload the `.tsra`. Remote open/verify from HF URLs is covered by #777's generic HTTP range-read. Backlog; needs a short design note before any code.

Refs: #777, #778
