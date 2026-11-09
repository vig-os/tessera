---
type: issue
state: open
created: 2026-11-09T08:35:33Z
updated: 2026-11-09T08:35:33Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/790
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-09T09:04:16.914Z
---

# [Issue 790]: [ci(macos-build): aarch64-apple-darwin dist leg fails to start: 'The configuration macos-14-arm64-us-default is not supported'](https://github.com/vig-os/tessera/issues/790)

## Observed (2026-11-09, PR #786, run 42569492387)

`dist build (aarch64-apple-darwin)` failed in under a minute, twice: the first run and a `--failed` rerun (jobs
129018481628 and 129053741187). The check-run annotation:

```text
The configuration 'macos-14-arm64-us-default' is not supported
```

`macOS build` then fails with `the macOS dist build did not pass on every darwin target`. `dist build
(x86_64-apple-darwin)` passed in the same run. The last `macOS build` run on `dev`, on 2026-11-04, passed.

## Reading

The aarch64 leg's runner comes from cargo-dist's plan, which defaults to `macos-14` for `aarch64-apple-darwin`
(`dist-workspace.toml:45` also assumes `macos-14`). GitHub no longer accepts that hosted configuration, so the
release's macOS aarch64 binary is blocked too, not just PR CI.

## Ask

Pin a supported arm64 runner for the aarch64-apple-darwin target in `dist-workspace.toml`, for example
`[dist.github-custom-runners]`. Then regenerate `release.yml` with `dist generate`, because the workflow is generated.
Related: #787.
