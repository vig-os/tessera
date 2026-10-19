---
type: issue
state: open
created: 2026-10-19T06:32:01Z
updated: 2026-10-19T06:32:01Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/650
comments: 0
labels: bug
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-19T07:17:38.909Z
---

# [Issue 650]: [release(macOS): C objects built for macOS 14.5 linked into a binary that targets 11.0 (MACOSX_DEPLOYMENT_TARGET unset)](https://github.com/vig-os/tessera/issues/650)

## What

Found while root-causing #647. On the v0.1.0-alpha.1 Release run ([35933703355](https://github.com/vig-os/tessera/actions/runs/35933703355)), the aarch64-apple-darwin link of the shipped `tessera` binary printed 66 linker warnings of this form:

```text
ld: warning: object file (.../libzstd_sys-….rlib[4](…-error_private.o)) was built for newer 'macOS' version (14.5) than being linked (11.0)
```

They come from C objects compiled by `cc`-based build scripts: `zstd-sys` and `blake3` (`blake3_neon.o`). The vendored HDF5 is likely affected too. rustc links for aarch64-apple-darwin's default deployment target of 11.0. The C objects, though, are compiled against the runner's SDK default (macOS 14.5 on `macos-14`), because `MACOSX_DEPLOYMENT_TARGET` is not set in the dist build.

## Why it matters

The shipped binary claims to support macOS 11.0. It may use APIs or ABI details from 14.5 in those objects, and so fail or misbehave on macOS 11–14.4. The x86_64 leg (`macos-15-intel`) probably has the same shape against its own SDK.

## Decide

Pick a minimum supported macOS for the broad channel and set `MACOSX_DEPLOYMENT_TARGET` to it for the dist build. Pass it through cargo-dist's documented mechanism, not by hand-editing the generated `release.yml`. Then make the `macOS build` gate (#647, `.github/workflows/macos-build.yml`) fail on this warning class. Today it is only a warning, so nothing catches it.

Refs: #647

