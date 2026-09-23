---
type: pull_request
state: closed (merged)
branch: chore/update-devcontainer-config → dev
created: 2026-02-24T22:43:08Z
updated: 2026-02-24T22:47:25Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/pull/9
comments: 0
labels: none
assignees: none
milestone: none
projects: none
relationship: none
merged: 2026-02-24T22:47:25Z
synced: 2026-08-04T10:47:22.845Z
---

# [PR 9](https://github.com/vig-os/tessera/pull/9) chore: update devcontainer config and devc-remote script

## Summary

- Updated `justfile.base` devc-remote recipe to accept variadic args and improved usage comments to reflect auto-clone and `--repo` flag support
- Improved `devc-remote.sh` with proper error handling for `docker compose up`, added progress logging throughout the `main()` flow

## Test plan

- [ ] Run `just devc-remote myserver` against a remote host and verify it connects and opens the editor
- [ ] Verify error messaging when compose up fails on the remote


---
---

## Commits

### Commit 1: [5295618](https://github.com/vig-os/tessera/commit/52956183ebe9844358ade5d2f19c6e4c88ec9c16) by [gerchowl](https://github.com/gerchowl) on February 24, 2026 at 09:51 PM
chore: resolve merge conflict in devc-remote.sh, 12 files modified (scripts/devc-remote.sh)

### Commit 2: [be1aee5](https://github.com/vig-os/tessera/commit/be1aee563d88695b50de79f45623ac62619cee79) by [gerchowl](https://github.com/gerchowl) on February 24, 2026 at 10:40 PM
chore: Update project configuration and documentation, 19 files modified (.devcontainer/justfile.base, scripts/devc-remote.sh)

### Commit 3: [16bab38](https://github.com/vig-os/tessera/commit/16bab38f0e1c684779624752a730e39d3db9721c) by [gerchowl](https://github.com/gerchowl) on February 24, 2026 at 10:47 PM
chore: merge dev into update-devcontainer-config, 117 files modified (scripts/devc-remote.sh)
