---
type: issue
state: open
created: 2026-10-30T01:12:00Z
updated: 2026-11-08T02:43:12Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/715
comments: 3
labels: feature, area:testing, area:io
assignees: none
milestone: none
projects: none
parent: 677
children: none
synced: 2026-11-08T08:33:52.409Z
---

# [Issue 715]: [test(io): ksb RSS proof of push_table with the consumer reproducer (ADR-0064 PR G)](https://github.com/vig-os/tessera/issues/715)

PR G of ADR-0064's implementation plan (§8.1): the ksb RSS proof that settles the memory half of #677.

**Scope:** a ksb job (flake + sampler script) running the consumer reproducer (MorePET/visionx-rs PR #41,
`stream_memory.rs`, switched to `push_table`, exact nullable schema) against the PR's tessera:
`ksb run --cpu 4 --exclusive-cpus`, session dir on `--scratch`; `W ∈ {1, 4}` × rows `∈ {0, 4,194,304, 67,108,864, 268,435,456,
1,073,741,824, 2,400,000,000}`; samples `RssAnon` and the cgroup's `memory.current` + `memory.stat`
(`anon`/`file`/`shmem`) every 10 ms, and the input row-group at each peak; fails if `statfs` of the session dir is tmpfs. Results recorded in ADR-0026 and
FEATURE-MATRIX.

**Acceptance (all six ADR-0064 §8.1 criteria):** ceiling `R₀ + min(W, n)(R₁ − R₀) + s + s_rss`; plateau ≤ 5% from
16 W blocks; 2.4 G rows completes under `--mem 4Gi` with `shmem` < 16 MiB; peak `RssAnon` < 8 GiB at 2.4 G rows;
equal canonical manifests (minus `producer`, `timestamp`, `id`, `id_inputs`, `manifest_hash`) and equal
`content_hash` for `W = 1` and `4` at 268 M rows, so a `W`-dependent naming or marker difference fails; `W = 4` ≥ 1.5× faster than `W = 1` at 268 M rows. The job exits non-zero
on any failed criterion.

---
Design: ADR-0064 "Logical tables over numbered leaves" (`docs/adr/0064-logical-tables-over-numbered-leaves.md`,
Proposed, PR for #677). Implement only after the ADR is Accepted. Sub-issue of #677; order A → B → C1, C2 → D → E → F → G → H
(D lands only after B, C1 and C2, so no writer on `dev` writes a form a reader on `dev` can't read).


---

# [Comment #1]() by [gerchowl]()

_Posted on November 4, 2026 at 12:36 AM_

**Owner rulings on the r1 BLOCKED report (2026-11-04).** Diagnosis: `docs/spikes/715-rss-proof-diagnosis.md` @ 252af87.

1. **Criterion 2 (plateau):** fix #759 first. Then amend ADR-0064 §8 so the bound carries an explicit `c_leaf · n_leaves` term. `c_leaf` is derived from the serialized per-leaf manifest entry, not curve-fitted. The plateau criterion checks flatness beyond `c_leaf · Δn`.
2. **Criterion 6 (parallelism):** an isolated run with staging on `/dev/shm`, with `--mem` sized for it. Criteria 1–5 stay on disk-backed scratch. The shared-disk throughput is recorded as context, with the disk's measured %util.
3. **W=4 plateau noise:** each W=4 plateau point runs k=3 times, and the plateau compares medians. Criterion 1 (ceiling) uses the max over all runs.

**Order:** #759 and the ADR-0064 amendment (#760) run in parallel, then G reruns with the amended criteria, split into G1 (harness) and G2 (records).

Refs: #677

---

# [Comment #2]() by [gerchowl]()

_Posted on November 4, 2026 at 02:58 AM_

**Owner addendum (2026-11-04), cited by ADR-0064 amendment A.4 (#760/PR #764):** the ksb node runs on an overwhelmed shared HDD, a node-configuration problem tracked in exo-pet/exo-fleet#980 (measurements posted there). Every disk-throughput or wall-time number from ksb disk-backed scratch or the seat's `/mnt/HDD` is environment-dominated and is never pass/fail evidence. Allocation and RSS measurements stay valid. Criteria 1 and 3 remain on disk-backed scratch because a slow disk makes them harder, not easier. A criterion-3 failure is checked against the disk's measured %util before it is called a writer defect.

Refs: #677

---

# [Comment #3]() by [gerchowl]()

_Posted on November 8, 2026 at 02:43 AM_

**Owner ruling, 2026-11-08: criterion 2 (plateau) becomes a slope test.**

Evidence: G r3, PR #767 at 0b66160, ksb `tessera-715-r3-sweep-2`. At W=4 the medians of 3 runs are 189.5 / 202.9 / 195.9 MiB at 64 / 256 / 1024 leaves. Going from 64 to 256 leaves fails the pairwise check, 202.9 against the allowed 200.5 MiB. The medians are not monotone in n, and the spread within one point is 8.6% against τ = 5%. That is noise from concurrent encode peaks, not growth in n; the median of three runs did not suppress it.

Decision: fit every run's peak against n, and require the fitted slope to be ≤ `c_leaf` plus a margin derived from the measured noise. The pairwise median comparison goes. The slope test must still catch the zone-statistics regression it exists for (≈0.058 B/row, +47 MB across the sweep). Criterion 1 (the ceiling over every run) is unchanged.

Tracked as a second ADR-0064 amendment (new issue). Then G reruns with a fresh evaluator. Criterion 3's failure in r3 came from an evaluator baseline defect (the cgroup anon baseline was read before the binary ran), already fixed in 0b66160; the rerun covers it.


