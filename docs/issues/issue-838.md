---
type: issue
state: open
created: 2026-11-11T04:21:36Z
updated: 2026-11-11T04:21:36Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/838
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-11T08:50:23.804Z
---

# [Issue 838]: [ci(dist): darwin builds don't set MACOSX_DEPLOYMENT_TARGET, so vendored HDF5 targets the runner's SDK, not the binary's declared minimum macOS](https://github.com/vig-os/tessera/issues/838)

## Defect

The darwin `dist build` legs never set `MACOSX_DEPLOYMENT_TARGET`. The vendored HDF5 (`static-hdf5` →
`hdf5-metno-src` 0.10.3, a CMake build through cmake 0.1.58) is therefore compiled for the **build host's SDK
version**, not for the binary's declared minimum macOS.

- cc 1.2.65 `src/lib.rs` (`apple_deployment_target`, around lines 4126-4223) resolves the macOS deployment target
  in this order: `MACOSX_DEPLOYMENT_TARGET`, then `xcrun --show-sdk-version --sdk macosx` (the host's SDK), then
  the 11.0/10.7 fallback. The source comment says it "doesn't use rustc's `--print deployment-target`".
- cmake 0.1.58 forwards cc's compiler args, including that `-mmacosx-version-min=<SDK version>`, into
  `CMAKE_C_FLAGS`.
- rustc links the final binary with its own default (11.0 for aarch64-apple-darwin, 10.12 for x86_64). So the
  Mach-O header claims a floor the HDF5 objects inside it don't honor.

So the shipped binaries' effective minimum macOS follows whatever Xcode the runner image defaults to.
`x86_64-apple-darwin` already builds on `macos-15-intel`. `aarch64-apple-darwin` moved from `macos-14` to
`macos-15` in #837 (#790).

## Not yet verified

I haven't inspected the built objects' `LC_BUILD_VERSION minos` (`vtool -show-build` / `otool -l`), and the
build logs show no `ld: warning: object file ... was built for newer macOS version`. The practical effect on
older macOS is therefore unproven: plain-C HDF5 may never call a newer API.

## Fix (proposed)

Choose and document a supported macOS floor for the broad channel. Set `MACOSX_DEPLOYMENT_TARGET` to it for the
darwin build legs (cargo-dist config, or the build env if dist 0.32 has no knob). Then add a check in
`macos-build.yml` that every object/binary's `minos` is ≤ the floor, so the floor is asserted rather than
assumed.

