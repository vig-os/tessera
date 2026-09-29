---
type: issue
state: open
created: 2026-09-24T01:00:11Z
updated: 2026-09-28T17:59:32Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/438
comments: 0
labels: none
assignees: none
milestone: devkit release adoption
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:35.809Z
---

# [Issue 438]: [ci(release): release-nix never fires — cargo-dist's announce creates the Release with GITHUB_TOKEN, which cannot trigger `release: published`; chain via `workflow_run`](https://github.com/vig-os/tessera/issues/438)

## Problem

`release-nix.yml` triggers on `release: types: [published]`. cargo-dist's generated `release.yml` creates the GitHub Release in its `announce` job using `GITHUB_TOKEN`. GitHub deliberately does **not** fire workflow triggers for events caused by `GITHUB_TOKEN` (anti-recursion), so `release-nix` never runs after a cargo-dist release. Observed on **v0.1.0-alpha.1** (2026-09-24): the Release published with 13 cargo-dist assets, no `release-nix` run appeared in 90 min; it had to be `workflow_dispatch`ed by hand with the tag.

## Fix (durable)

Chain it the sanctioned way — `workflow_run` events **do** fire for `GITHUB_TOKEN`-driven workflows:

```yaml
on:
  workflow_run:
    workflows: ["Release"]
    types: [completed]
  workflow_dispatch: { inputs: { tag: … } }   # keep the manual path
jobs:
  build:
    if: ${{ github.event_name == 'workflow_dispatch' || github.event.workflow_run.conclusion == 'success' }}
```
and derive the tag from `github.event.workflow_run.head_branch` (cargo-dist runs on the tag ref, so `head_branch` is the tag) when not dispatched. Alternative: cargo-dist `publish-jobs = ["./release-nix"]` (`workflow_call`) — heavier refactor of release-nix.yml.

Keep `release: published` too? No — it can only fire for manually-created releases, which this repo doesn't do; drop it to avoid a false sense of coverage.

Refs: #337
