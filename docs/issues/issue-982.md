---
type: issue
state: closed
created: 2026-11-25T01:57:17Z
updated: 2026-11-25T03:02:43Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/982
comments: 0
labels: chore, docs
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-25T10:51:44.669Z
---

# [Issue 982]: [docs(changelog): the root CHANGELOG has never been versioned, so every release's notes dump all history](https://github.com/vig-os/tessera/issues/982)

## What
At every tag from `v0.1.0-alpha.1` to `v0.1.0-alpha.6`, the root `CHANGELOG.md` has exactly one heading: `## Unreleased`. The GitHub release body is built from that section, so alpha.6's notes list items that shipped in alpha.1–alpha.5 (for example #709, ADR-0064 PR A). That breaks Keep a Changelog, which the file's own header claims to follow. A consumer can't tell what changed between two alphas.

It also blocks #917 (the format-break alpha.7). Its acceptance requires the notes to name the format break, the refused alpha layout (no migration verb), which hashes moved, and that signatures made before the break no longer verify.

## Fix
1. Reconstruct one `## [0.1.0-alpha.N] - <tag date>` section per past release from the file at each tag. Release N's entries are those present at tag N and absent at tag N-1. Keep `## Unreleased` for what is new since alpha.6.
2. Add the `[0.1.0-alpha.N]` compare links.
3. Check how the release notes are extracted (cargo-dist's changelog lookup in the `Release` workflow, and any awk step in the workflows). Make sure the next release takes exactly its own section. Document the release-time step (rename `Unreleased` to the version) in the release runbook or CONTRIBUTING.

Refs: #917
