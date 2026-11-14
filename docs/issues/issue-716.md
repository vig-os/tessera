---
type: issue
state: closed
created: 2026-10-30T01:12:02Z
updated: 2026-11-13T13:01:55Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/716
comments: 2
labels: feature, area:docs
assignees: none
milestone: none
projects: none
parent: 677
children: none
synced: 2026-11-14T08:34:37.519Z
---

# [Issue 716]: [docs: tell MorePET/visionx-rs about push_table and pairs/NNNN (ADR-0064 step H)](https://github.com/vig-os/tessera/issues/716)

Step H of ADR-0064's implementation plan: tell the #677 consumer.

**Scope:** after PR G passes, comment on #677 and open an issue on MorePET/visionx-rs describing the switch from
`table_job_streaming` to `StreamWriter::push_table`, the new leaf names `pairs/NNNN` (and that `pairs` stays the one
logical name for `logical_table`, `read`, `sql` and the Python `table`/`logical_column`), that `table_job_streaming`
refuses tables above `BLOCK_ROWS`, the measured RSS bound, and the tessera release that carries it.

**Acceptance:** the visionx-rs issue is linked from #677 and names the release; #677 closes after G and H.

---
Design: ADR-0064 "Logical tables over numbered leaves" (`docs/adr/0064-logical-tables-over-numbered-leaves.md`,
Proposed, PR for #677). Implement only after the ADR is Accepted. Sub-issue of #677; order A → B → C1, C2 → D → E → F → G → H
(D lands only after B, C1 and C2, so no writer on `dev` writes a form a reader on `dev` can't read).

---

# [Comment #1]() by [gerchowl]()

_Posted on November 13, 2026 at 11:55 AM_

From the MorePET/visionx-rs architect, 2026-11-13.

- **#677:** `StreamWriter::push_table` in v0.1.0-alpha.5 (ADR-0064, with the ksb RSS proof in #767 and #839) looks like it satisfies #677. Can #677 be closed and this step H note posted? Our streaming scan writer (MorePET/visionx-rs#39) builds on it.
- **Pin:** we are bumping visionx-rs to the `v0.1.0-alpha.5` tag, pinning the tag rather than `branch = "dev"`, and adding `partition: None` to every `TableSpec`. Say so if you'd prefer downstream to track `dev`.
- **Question for #39:** within one `WriteSession`, can several `push_table` streams be open at the same time (`pairs`, `markers`, `ms_stats`, plus QC blocks fed by the same decode pass), or must the tables be pushed one after another? What pattern do you intend for interleaved producers, and does the W+1 staging bound hold per table or per session?

---

# [Comment #2]() by [gerchowl]()

_Posted on November 13, 2026 at 01:01 PM_

Follow-up from the MorePET/visionx-rs architect, 2026-11-13. We read the alpha.5 source to design visionx-rs#39 on `push_table`. Three points don't fit a single-pass producer of several tables, which is the shape of our decoder:

1. `push_table` needs `spec.rows` exactly before the first row (`tessera-io/src/stream.rs:862`, `:1001-1026`). A list-mode decode only knows its pair, marker and ms_stats counts at the end.
2. It is one table per call, holding `&mut StreamWriter` (`stream.rs:927-934`). So `pairs`, `markers` and `ms_stats` cannot be fed in parallel from one pass.
3. The spine is fixed before `StreamWriter::new` (`write.rs:638-686`). Fields derived from the payload, such as mode evidence and per-file decode summaries, therefore need a separate earlier pass.

Our plan, pending owner sign-off: a two-pass writer. Pass 1 counts the rows and builds the spine evidence. Pass 2 streams `pairs` through `push_table` and spills the small side tables, which are then pushed in turn. This works on alpha.5 as it stands.

Feature request, not blocking us: an unknown-length `push_table` (ADR-0064 §1/§2 mention "a future unknown-length partitioning"), several open table handles per session, or both. Either would let us drop pass 1's second read of a 19 GB file. Should we file that as a separate issue?

