---
type: issue
state: closed
created: 2026-11-04T00:36:57Z
updated: 2026-11-04T02:58:53Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/760
comments: 0
labels: documentation
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-04T08:35:50.964Z
---

# [Issue 760]: [docs(adr): amend ADR-0064 §8/§8.1 with c_leaf·n, the tmpfs parallelism run and median-of-3 plateau (#715 rulings)](https://github.com/vig-os/tessera/issues/760)

Amend Accepted ADR-0064 (`docs/adr/0064-logical-tables-over-numbered-leaves.md`) §8 and §8.1 per the owner rulings on #715 (comment of 2026-11-04) and the diagnosis `docs/spikes/715-rss-proof-diagnosis.md` (branch `test/715-ksb-rss-proof` @ 252af87):

1. **§8 model:** the bound gains `c_leaf · n_leaves`. `c_leaf` is defined from the serialized per-leaf manifest entry (measured ~3.1 KB for the consumer's 12-column schema), with an explicit allowance for allocator retention, derived rather than fitted. Update `push_table`'s rustdoc bound to match.
2. **§8.1 criterion 2:** the plateau is checked beyond `c_leaf · Δn`. W=4 plateau points run k=3 times and compare medians. Criterion 1 uses the max over all runs.
3. **§8.1 criterion 6:** parallelism is measured in an isolated run with staging on `/dev/shm`, with `--mem` sized for it. Criteria 1–5 stay on disk-backed scratch, and the shared-disk throughput is recorded as context with its measured %util. Keep the rule that a writer-caused failure stops the PR as a design question.

Follow the repo's ADR-amendment convention (`docs/adr/README.md`).

Refs: #715
