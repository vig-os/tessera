---
type: issue
state: open
created: 2026-09-29T15:28:14Z
updated: 2026-09-29T15:28:14Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/530
comments: 0
labels: none
assignees: none
milestone: 0.1.0-beta
projects: none
parent: none
children: none
synced: 2026-09-30T07:57:01.675Z
---

# [Issue 530]: [feat(schema): archive conformance profiles (BagIt-Profiles analogue) on top of ADR-0050](https://github.com/vig-os/tessera/issues/530)

ADR-0050's CollectionSchema/MemberRule profiles collection membership and ADR-0003 profiles product typing. Neither asserts archive-level shape (required aux members, allowed digest algorithms, required info fields), the role bagit-profiles JSON plays for deposit gatekeeping. Add an `archive_profile` clause plus a `tessera verify --profile` check. Additive.
