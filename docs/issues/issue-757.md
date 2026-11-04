---
type: issue
state: open
created: 2026-11-03T15:04:51Z
updated: 2026-11-03T15:04:51Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/757
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-04T08:35:53.003Z
---

# [Issue 757]: [docs: FEATURE-MATRIX lists push_table (#713) as open; narrow the read-back rustdoc wording](https://github.com/vig-os/tessera/issues/757)

Review nits from the #751 re-review (PR merged as a3df683).

1. `tessera/docs/FEATURE-MATRIX.md:112`: the logical-tables row still says "**Open:** `StreamWriter::push_table` (#713)". #713 was closed by #750 (fb39323). Move it to Landed.
2. `stream.rs` rustdoc says "so a worker reads its staged block back a row-group at a time". `staged_job_memory.rs` proves only that a worker holds at most `s` above the encoder, which rules out a whole-block read back. Reword the rustdoc to claim what is proved, or add a test that pins the row-group granularity.

Refs: #714
