---
type: issue
state: open
created: 2026-10-30T01:12:00Z
updated: 2026-10-30T01:23:07Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/715
comments: 0
labels: feature, area:testing, area:io
assignees: none
milestone: none
projects: none
parent: 677
children: none
synced: 2026-10-30T08:02:15.501Z
---

# [Issue 715]: [test(io): ksb RSS proof of push_table with the consumer reproducer (ADR-0064 PR G)](https://github.com/vig-os/tessera/issues/715)

PR G of ADR-0064's implementation plan (§8.1): the ksb RSS proof that closes #677's memory half.

**Scope:** a ksb job (flake + sampler script) running the consumer reproducer (MorePET/visionx-rs PR #41,
`stream_memory.rs`, switched to `push_table`, exact nullable schema) against the PR's tessera:
`ksb run --cpu 4 --exclusive-cpus`, session dir on `--scratch`; `W ∈ {1, 4}` × rows `∈ {0, 4,194,304, 67,108,864, 268,435,456,
1,073,741,824, 2,400,000,000}`; samples `RssAnon` and the cgroup's `memory.current` + `memory.stat`
(`anon`/`file`/`shmem`) every 10 ms, and the input row-group at each peak; fails if `statfs` of the session dir is tmpfs. Results recorded in ADR-0026 and
FEATURE-MATRIX.

**Acceptance (all six ADR-0064 §8.1 criteria):** ceiling `R₀ + min(W, n)(R₁ − R₀) + s + s_rss`; plateau ≤ 5% from
16 W blocks; 2.4 G rows completes under `--mem 4Gi` with `shmem` < 16 MiB; peak `RssAnon` < 8 GiB at 2.4 G rows;
same `content_hash` for `W = 1` and `4`; `W = 4` ≥ 1.5× faster than `W = 1` at 268 M rows. The job exits non-zero
on any failed criterion.

---
Design: ADR-0064 "Logical tables over numbered leaves" (`docs/adr/0064-logical-tables-over-numbered-leaves.md`,
Proposed, PR for #677). Implement only after the ADR is Accepted. Sub-issue of #677; order A → B → C1, C2 → D → E → F → G → H
(D lands only after B, C1 and C2, so no writer on `dev` writes a form a reader on `dev` can't read).

