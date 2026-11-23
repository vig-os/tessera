---
type: issue
state: open
created: 2026-11-22T20:46:07Z
updated: 2026-11-22T20:46:07Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/957
comments: 0
labels: chore, area:testing
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-23T10:16:27.250Z
---

# [Issue 957]: [chore(ksb): a per-crate test job so agents can build and test off the seat](https://github.com/vig-os/tessera/issues/957)

## Why
Every agent build slot (`/mnt/HDD/larsgerchow/tessera-target-slot{1,2,3}`) lives on the seat's /mnt/HDD FUSE mount. It has stalled five times in about 30 h (gerchowl/g-fleet#1676), and each stall freezes every delegate's build with processes stuck in D state. A full CI cycle takes 45–90 min, which is too slow for test iteration. Owner direction, 2026-11-22: run agents' builds and tests as small ksb jobs sized like a local run.

## What
One reusable ksb job (`bench/ksb/` style; see 677-rss-proof, 905-commit-cost and #828's m4b harness):
- **Input:** a git SHA (pushed, or a local commit snapshotted the way ksb run snapshots sources), a crate, an optional nextest filter, and an optional flag set (`--bins`, `--features sql`).
- **Output:** the nextest result (JUnit plus a summary line), the exit code, and the failing tests' logs in `$KSB_JOB_OUT`.
- **Build:** compile in a crane derivation, not in the job pod, because pod cargo builds get SIGTERM 143. Reuse the flake's existing crane deps so that warm runs take minutes.
- **Size:** 4 CPU and 8–16 GiB by default, with a timeout under 30 min.
- **Wrapper:** one command for agents, e.g. `scripts/ksb-test.sh <sha> <crate> [filter]`, which submits, waits and prints the summary with the real exit code.
- **Docs:** CLAUDE.md and the agent traps document it as the default way to test until #1676 is fixed.

## Acceptance
- A run on dev passes for tessera-core, tessera-io and `tessera-cli --bins`, with cold and warm wall times reported.
- A deliberately failing test on a scratch branch makes the wrapper exit non-zero and shows that test's output.
- **Known gaps, documented:** private DUPLET fixtures are not a ksb dataset (`dataset_refused`), and pods cannot count instructions (seccomp has no `perf_event_open`).

Refs: #828
