---
type: issue
state: open
created: 2026-11-03T06:26:41Z
updated: 2026-11-03T06:26:41Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/746
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-03T08:59:42.520Z
---

# [Issue 746]: [io writers: follow-up nits from #745 (ADR-0064 D)](https://github.com/vig-os/tessera/issues/746)

Review nits from PR #745 (ADR-0064 PR D, #712; merged 3ebb40a). None blocked the merge.

- [ ] `tessera-io/src/table.rs` test (~3564): `block_name("", 0, 2).is_err()` only checks that some error comes back. Match `Error::InvalidBlockName` to pin the typed variant the rustdoc promises.
- [ ] `tessera-io/src/accumulate.rs` (~382): an edited rustdoc line was not re-wrapped and is longer than the surrounding doc lines (cosmetic).
- [ ] `ge_hdf5.rs` `PRE_SWITCH_CONTENT_HASH`: the literal's capture at `11280ba` rests on the PR body. Add a one-line comment stating how to recapture it (the commit and the command), so a future reader can re-derive it.

Refs #712

