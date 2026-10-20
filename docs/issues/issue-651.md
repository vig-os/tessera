---
type: issue
state: open
created: 2026-10-19T09:24:02Z
updated: 2026-10-19T09:24:02Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/651
comments: 0
labels: chore, priority:low, area:ci
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-20T07:08:35.473Z
---

# [Issue 651]: [follow-ups from #648 review: --cold error category, unverified dist installer, macOS-build required-check shape](https://github.com/vig-os/tessera/issues/651)

Review nits from #648 (#647). None blocks the release; each needs a decided fix.

1. **Wrong error category.** `tessera-cli/src/bench_compare.rs:1567` reports the `--cold` platform refusal through `err()`. The user therefore sees `error: invalid product: bench compare: --cold is not supported…`. A platform limit is not an invalid product. It needs an "unsupported" error kind, or a usage-error path with its own exit code.
2. **Unverified dist installer.** `.github/workflows/macos-build.yml:69-75` installs cargo-dist via `curl … | sh` with no checksum. The generated `release.yml` does the same, so the fix belongs in the dist config (a pinned installer plus hash verification, if cargo-dist supports it), and both workflows must agree. Never hand-edit `release.yml`.
3. **Required-check semantics.** The `macOS build` workflow is path-filtered. As a required check, it would block PRs that don't touch those paths. Before the owner makes it required, it needs an always-reporting shape: a no-op success leg, or a `CI Summary`-style aggregation.

