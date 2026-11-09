---
type: issue
state: open
created: 2026-11-09T08:35:14Z
updated: 2026-11-09T08:35:14Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/789
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-09T09:04:17.462Z
---

# [Issue 789]: [ci(nix-check): x86_64-linux leg hits the 2h job limit with workspace-test starved (tests 100x slower), on three unrelated branches](https://github.com/vig-os/tessera/issues/789)

## Observed (2026-11-09)

The `nix flake check (x86_64-linux)` leg was cancelled at the 2h job limit (`The job has exceeded the maximum execution time
of 2h0m0s`) on three unrelated branches:

| run | branch | change |
| --- | --- | --- |
| 42569492335 (job 129003331386, runner `ksb-vig-os-large-runner-sr8lt`) | `refactor/762-identity-envelope-crate` (#786) | crate move, no `tessera-io` change |
| 42564071349 | `chore/release-plz-2026-10-29T04-53-10Z` | version bump only |
| 42558901451 | `test/715-ksb-rss-proof` | |

`docs/770-adr0064-plateau-slope` (run 42556966454) passed on the same base shortly before.

In #786's leg, `workspace-test` (`tessera-nextest`) started 759 tests at 07:19:57, and 750 had passed when the job
was killed at 08:28. Tests that normally take seconds ran for 700–1070 s, for example:

- `stream::tests::byte_budget_bounds_encoded_bytes_while_the_first_job_is_held` 1072.8 s
- `stream::tests::held_workers_pin_w_plus_one_staged_input_blocks` 717.7 s
- `multiblock::tests::multi_block_identity_is_independent_of_values_under_nulls` 708.6 s

Four tests were still running at the cutoff:

- `tessera-io stream::tests::content_hash_is_identical_across_workers_and_windows` (>3240 s)
- `tessera-io::block_names recover_refuses_a_journal_line_with_a_bad_name` (>600 s)
- `tessera-io::session_leaf_memory session_leaf_memory`
- `tessera-io::staged_job_memory staged_job_memory`

## Reading

The slowdown is general (a `multiblock` read test is as slow as the stream tests), and a version-bump-only branch is
affected. That points to runner CPU or thread starvation rather than a deadlock in one test. It could be the
pod-CPU-vs-`available_parallelism()` mismatch on ksb pods, or several heavy legs sharing a node. This hasn't been
confirmed: comparing this run's test timings with 42556966454's, and the node's load at the time, would settle it.

## Ask

Find why the x86 leg's tests are starved, and fix the cause rather than raising the 2h limit.
