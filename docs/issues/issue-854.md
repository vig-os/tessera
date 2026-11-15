---
type: issue
state: open
created: 2026-11-14T10:20:04Z
updated: 2026-11-15T00:50:40Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/854
comments: 1
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-15T08:48:30.082Z
---

# [Issue 854]: [chore(release): stale repo description ("on HDF5"), main stuck at alpha.1 (262 behind dev), legacy v0.1.0 / v1.0.0-rc1 tags outrank the alphas](https://github.com/vig-os/tessera/issues/854)

Consumers that read the repository front page or `main` get the wrong picture of tessera. Found while routing an event-browser request (#853, MorePET/visionx-rs#237): the repo's own front page told us `.tsra` was HDF5.

## 1. Stale repository description
The GitHub "About" text still reads **"FAIR Data on HDF5 — self-describing, FAIR-principled data format for scientific data products"**. Since then:
- tables are **Vortex** and arrays are **Zarr v3 + pcodec** (README, docs/book `tables-sql.md`);
- HDF5 is used only to *read* vendor acquisitions at ingest.

Suggested wording: "Self-describing, sealed FAIR container (`.tsra`): Vortex tables + Zarr v3 arrays, with provenance and versioning."

## 2. `main` lags every published release
| ref | commit | date |
|---|---|---|
| `main` | ee277ad3 "release: v0.1.0-alpha.1" | 2026-09-23 |
| `v0.1.0-alpha.5` (latest GitHub release) | 5373f61a | 2026-11-11 |
| `dev` (default branch) | e2ecaa09 | 2026-11-13 |

`main` is 262 commits behind `dev`. The alpha.3, alpha.4 and alpha.5 releases were cut without `main` being moved forward. Downstream pins such as visionx-rs `crates/visionx-tsra` → `tag = "v0.1.0-alpha.5"` are fine, but anything that tracks or reads `main` sees alpha.1.

**Ask:** promote `v0.1.0-alpha.5` (or the next alpha) to `main` through the repo's normal release flow. Keep `main` = latest release from now on, e.g. as a step in the release checklist or workflow.

## 3. Legacy tags outrank the alpha series
`v0.1.0` (331328c5, 2026-06-25) and `v1.0.0-rc1` (b1b2a0f9, 2026-06-26) predate `v0.1.0-alpha.1`. By semver, both sort **above** `v0.1.0-alpha.5` (`v0.1.0` > `v0.1.0-alpha.5`, and `v1.0.0-rc1` is the highest tag). Tooling that picks the "latest tag" (renovate, `cargo` git-tag resolution, `git describe`, release-please) can choose them. Neither has a GitHub release.

**Ask:** decide whether these are legacy pre-reset tags. If so, document them (e.g. in CHANGELOG or a release note) and decide whether to retire or rename them. That's the maintainers' call, since deleting published tags breaks anyone who pinned them.

---

# [Comment #1]() by [gerchowl]()

_Posted on November 15, 2026 at 12:50 AM_

Owner decision 2026-11-15 on §3: **delete both legacy tags outright**. They are pre-reset fd5-era format milestones: no GitHub release, and nothing in vig-os/MorePET pins them (code search, 2026-11-15). For the record, before deletion:
- `v0.1.0`: tag object `3f7b495e7f14cf31c2cdf8b03ede6b0d31327c61` → commit `331328c51d0fa49c3cab3083d474db76d52a7c77` (2026-06-26): Tessera v0.1.0 — format frozen
- `v1.0.0-rc1`: tag object `b8e85d2d1daef026ff39635938cfc61b260ef7d6` → commit `b1b2a0f9d04941db0244485dd6c294eb42776ff9` (2026-06-26): Tessera v1.0.0-rc1 — format v1.0-ready, independently validated
Recreate one, if ever needed: `git tag <name> <commit>` (annotated messages above).

§1 (repo description) is done: it now reads 'Self-describing, sealed FAIR container (.tsra): Vortex tables + Zarr v3 arrays, with provenance and versioning.'
§2: owner-approved promotion PR #855 (main → v0.1.0-alpha.5). A follow-up PR adds the release step that keeps main equal to the latest release, and closes this issue.

