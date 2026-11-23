---
type: issue
state: open
created: 2026-11-22T23:15:29Z
updated: 2026-11-22T23:15:37Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/961
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-23T10:16:26.663Z
---

# [Issue 961]: [chore(ksb): bound the ksb test job's memory, not only its threads](https://github.com/vig-os/tessera/issues/961)

## Why
`scripts/ksb-test.sh` (#957, PR #960) runs a crate's tests inside a crane derivation on the ksb builder, because ksb builds are sandboxed at `/build` and test binaries bake `env!("CARGO_MANIFEST_DIR")` paths there, which the job pod cannot recreate (no `/build`, no user namespaces). The job's `--cpu`/`--mem` therefore bound only the reporting pod. The test run's threads are bounded (`--threads`, default 4), its memory is not: the builder has ~1.3 TB, so a test whose RSS would OOM a 16 GiB laptop passes there.

## What
Bound the test run's memory to a configurable limit (default 16 GiB) and record an overrun as a distinct, documented exit code. Options to evaluate: a cgroup the build can create on the ksb builder, `systemd-run --scope` if the builder allows it, or running the recorded binaries in the pod at a path both sides share (the build relocated to a fixed `/tmp/...` path, which both the sandbox and the pod can create).

## Acceptance
- A test allocating past the limit fails the run with the documented code, shown by the wrapper.
- CONTRIBUTING.md's "Known gaps" entry is removed.

Refs: #957

