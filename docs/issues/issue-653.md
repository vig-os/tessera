---
type: issue
state: open
created: 2026-10-19T09:26:13Z
updated: 2026-10-19T09:26:13Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/653
comments: 0
labels: bug, priority:low, area:ci
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-20T07:08:35.086Z
---

# [Issue 653]: [ci(release): changelog compare links use per-package tags (tessera-cli-v…) that are never created; repo uses unified v* tags](https://github.com/vig-os/tessera/issues/653)

The per-crate CHANGELOG headers that release-plz generates link to per-package tags that don't exist, for example:

```
## [0.1.0-alpha.3](https://github.com/vig-os/tessera/compare/tessera-cli-v0.1.0-alpha.2...tessera-cli-v0.1.0-alpha.3)
```

The repo cuts **unified** tags (`v0.1.0-alpha.1`, `v0.1.0-alpha.2`, `v0.1.0-alpha.3`), and the operator pushes them by hand after the release PR merges. `release-plz.toml` doesn't set `git_tag_name` or the changelog link template, so release-plz falls back to its per-package `<crate>-v<version>` scheme. As a result, every compare link in the alpha.2 and alpha.3 sections (#639, #652) returns a 404.

Fix: configure release-plz's tag name or changelog template to match the unified `v{{ version }}` tag. Check this doesn't let `release-plz release` create tags itself, since the operator owns the cut. Then regenerate or correct the existing headers in a normal PR. Found in the fresh review of #652.

