---
type: issue
state: open
created: 2026-11-27T11:23:47Z
updated: 2026-11-27T11:23:47Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/1032
comments: 0
labels: chore, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-28T09:47:00.386Z
---

# [Issue 1032]: [nits from the #1028 review (commit_collection, history)](https://github.com/vig-os/tessera/issues/1032)

From the review of PR #1028 (#1000, ADR-0067 step 5). The review found no blockers, and all four injected mutants were caught.
1. The `put_collection` doc line in `tessera-io/src/repo.rs` is 153 characters long; wrap it.
2. Supplied members that the collection doesn't reach are still checked and stored, so they stay orphaned until the next gc. gc does reclaim them, but neither ADR-0067 nor the rustdoc says so. Document it, or refuse unreached supplied members.
3. `MissingMember` paths don't shorten member hops, so the path grows with nesting depth. gc's paths grow the same way, but it is still worth bounding.
4. The ADR-0067 step-5 test names don't match the tests. `commit_collection_refuses_a_product_lineage` is covered by `collection_commit_checks_cas_then_kind_before_version_edges`, and the moved-tip test is folded into `commit_collection_advances_by_cas_and_reads_back`. Rename the tests or update the plan.
5. Add a test that `log` and `history` agree on a linear chain.

Refs: #1000
