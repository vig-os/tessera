---
type: issue
state: open
created: 2026-11-11T12:07:04Z
updated: 2026-11-11T12:07:04Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/841
comments: 0
labels: documentation
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-12T09:15:34.114Z
---

# [Issue 841]: [release: version the root CHANGELOG and mark breaking changes in release-plz crate logs](https://github.com/vig-os/tessera/issues/841)

From the fresh review of the alpha.5 release PR (#694 at 574b949).

- The hand-written root `CHANGELOG.md` has the consumer-facing breaking-change notes, but it has no version headings. Everything stays under `## Unreleased`, so the notes are never tied to a release. Give it a release step, for example release-plz `changelog_path` or a pre-tag step that renames Unreleased to the version.
- The crate CHANGELOGs list `TableSpec.partition`, a new public field that breaks struct literals (#728), under 'Added', not as breaking. `Reader::logical_table` changing `Error::Container` to `Error::LogicalTable` is unmarked too. Fix this at the source: mark the commits (`!` / `BREAKING CHANGE:`) or add a review step for the release-plz PR.
- The core log lists io APIs (`deprecate table_job_streaming`, `push_table`), and one entry is a meta entry ('note the legacy-path spec error change in the CHANGELOG').

For alpha.5, the GitHub release notes call these out by hand.

Refs: #694
