---
type: issue
state: closed
created: 2026-02-24T19:21:54Z
updated: 2026-09-28T17:53:17Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/6
comments: 1
labels: chore
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:53:14.294Z
---

# [Issue 6]: [[CHORE] Update devcontainer version and rename default branch to main](https://github.com/vig-os/tessera/issues/6)

### Chore Type

Configuration change

### Description

Update the devcontainer image version and rename the default branch from `master` to `main` (standard convention). Create the `dev` integration branch.

### Acceptance Criteria

- [x] Default branch renamed from `master` to `main` on local and origin
- [x] `dev` branch created from `main` on local and origin
- [ ] All local uncommitted changes committed on a chore branch
- [ ] PR merged into `dev`

### Implementation Notes

The repository was using `master` as the default branch. The pre-commit hook already expected `main` (the `no-commit-to-branch` regex allows `main` and `dev`). This change aligns the actual branch name with the hook configuration.

### Priority

Medium

### Changelog Category

No changelog needed
---

# [Comment #1]() by [gerchowl]()

_Posted on September 28, 2026 at 05:53 PM_

Closing as **obsolete** (2026-09-28 backlog triage).

Evidence: Predates Tessera trunk migration; default branch is now 'dev' per ADR-0052 (release-only main); no devcontainer in tessera/ tree.

https://claude.ai/code/session_01XdERKMVDAwfMJSKdTytNnK

