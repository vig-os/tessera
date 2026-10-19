---
type: issue
state: open
created: 2026-10-18T12:15:06Z
updated: 2026-10-18T12:15:06Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/608
comments: 0
labels: question, priority:medium, area:ci
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-19T07:17:49.838Z
---

# [Issue 608]: [ci(release): format-milestone tags v0.1.0 / v1.0.0-rc1 collide with the software release tags](https://github.com/vig-os/tessera/issues/608)

## Summary

The `v*` tag namespace holds two FORMAT-milestone tags that predate the software releases:

| Tag | Commit | Annotation |
|---|---|---|
| `v0.1.0` | 331328c (2026-06-26) | "Tessera v0.1.0 — format frozen" |
| `v1.0.0-rc1` | b1b2a0f (2026-06-26) | "Tessera v1.0.0-rc1 — format v1.0-ready, independently validated" |

Software releases use the same unified `v<version>` tags (`v0.1.0-alpha.1`, cut 2026-09-24). Two
consequences:

1. **The 0.1.0 software release cannot be tagged.** After its release PR merges, the operator must push
   `v0.1.0`, which already exists on a June commit. The same happens at `v1.0.0-rc1`.
2. **Baseline selection needs a workaround.** #606's `scripts/last-release-tag.sh` caps the candidate
   tags at the workspace version, so these milestones are not mistaken for the last release. That holds
   until the workspace reaches `0.1.0`. From then on, the June `v0.1.0` qualifies as a baseline.

The release train also fires on these tags: cargo-dist's `release.yml` matches `**[0-9]+.[0-9]+.[0-9]+*`.

## Decision needed (owner)

- Rename the format milestones into their own namespace (e.g. `format-v0.1.0`, `format-v1.0.0-rc1`). That
  rewrites public tags, and anything that references them must be updated. Or
- Keep them, and move the software tags to another scheme (e.g. per-crate `tessera-cli-v…`, or a
  `release-` prefix). That changes cargo-dist's trigger and the baseline lookup.

Must be decided before the workspace version reaches `0.1.0`.

