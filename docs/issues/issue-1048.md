---
type: issue
state: open
created: 2026-11-27T22:58:01Z
updated: 2026-11-27T22:58:01Z
author: github-actions[bot]
author_url: https://github.com/github-actions[bot]
url: https://github.com/vig-os/tessera/issues/1048
comments: 0
labels: bug, area:ci
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-28T09:46:57.422Z
---

# [Issue 1048]: [nix flake check: the run on release tag v0.1.0-alpha.8 failed](https://github.com/vig-os/tessera/issues/1048)

The `nix flake check` run https://github.com/vig-os/tessera/actions/runs/46072383822 (push, `v0.1.0-alpha.8` at e2db1c06397aba83c6f0d57db252216456903f1a) failed.

Its `nix flake check` job names the leg that failed, and that leg's summary names the
checks. On a nightly, the cause is among the day's merges to v0.1.0-alpha.8. Close this issue
once a run passes again.
