---
type: pull_request
state: closed (merged)
branch: chore/update-devcontainer-config → dev
created: 2026-02-24T19:22:51Z
updated: 2026-02-24T19:24:24Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/pull/7
comments: 0
labels: none
assignees: none
milestone: none
projects: none
relationship: none
merged: 2026-02-24T19:24:24Z
synced: 2026-08-04T10:47:25.680Z
---

# [PR 7](https://github.com/vig-os/tessera/pull/7) chore: update devcontainer config and project tooling

## Description

Update devcontainer configuration, project tooling scripts, and pre-commit hooks. This also aligns with the rename of the default branch from `master` to `main` and creation of the `dev` integration branch.

## Type of Change

- [x] `chore` -- Maintenance task (deps, config, etc.)

### Modifiers

- [ ] Breaking change (`!`) -- This change breaks backward compatibility

## Changes Made

- `.cursor/skills/pr_create/SKILL.md` — Updated PR creation skill
- `.cursor/skills/pr_solve/SKILL.md` — Updated PR solve skill
- `.cursor/skills/worktree_pr/SKILL.md` — Updated worktree PR skill
- `.devcontainer/justfile.base` — Updated base justfile
- `.devcontainer/justfile.gh` — Updated GitHub justfile
- `.devcontainer/justfile.worktree` — Updated worktree justfile
- `.devcontainer/scripts/check-skill-names.sh` — Added skill name validation script
- `.devcontainer/scripts/derive-branch-summary.sh` — Added branch summary derivation script
- `.devcontainer/scripts/gh_issues.py` — Updated GitHub issues script
- `.devcontainer/scripts/resolve-branch.sh` — Added branch resolution script
- `.pre-commit-config.yaml` — Updated pre-commit hooks configuration
- `pyproject.toml` — Updated project configuration
- `scripts/check-skill-names.sh` — Added skill name check script
- `src/fd5/template_project/__init__.py` — Removed template project init
- `uv.lock` — Updated dependency lock file

## Changelog Entry

No changelog needed — internal maintenance and configuration changes only.

## Testing

- [ ] Tests pass locally (`just test`)
- [x] Manual testing performed (describe below)

### Manual Testing Details

- Verified `master` branch renamed to `main` on local and remote
- Verified `dev` branch created and pushed
- Verified GitHub default branch set to `main`

## Checklist

- [x] My code follows the project's style guidelines
- [x] I have performed a self-review of my code
- [ ] I have commented my code, particularly in hard-to-understand areas
- [ ] I have updated the documentation accordingly (edit `docs/templates/`, then run `just docs`)
- [x] I have updated `CHANGELOG.md` in the `[Unreleased]` section (and pasted the entry above)
- [x] My changes generate no new warnings or errors
- [ ] I have added tests that prove my fix is effective or that my feature works
- [ ] New and existing unit tests pass locally with my changes
- [ ] Any dependent changes have been merged and published

## Additional Notes

N/A

Refs: #6


---
---

## Commits

### Commit 1: [fdc4176](https://github.com/vig-os/tessera/commit/fdc41765cce8c7136d7c8399cfe706400c32daaf) by [gerchowl](https://github.com/gerchowl) on February 24, 2026 at 07:22 PM
chore: update devcontainer config and project tooling, 753 files modified
