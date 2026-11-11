---
type: issue
state: open
created: 2026-11-11T01:18:50Z
updated: 2026-11-11T01:32:26Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/816
comments: 0
labels: feature, area:bindings
assignees: none
milestone: none
projects: none
parent: 799
children: none
synced: 2026-11-11T08:50:33.736Z
---

# [Issue 816]: [feature(capi): cargo-dist library artifacts and the abidiff ABI gate (ADR-0065 §10-§11)](https://github.com/vig-os/tessera/issues/816)

Part of epic #799. Design: ADR-0065 (docs/adr/0065-c-abi-bridge.md, PR for #800). **Blocked until the owner accepts ADR-0065**; do not start before. Toward #800.

## What
Release channel and ABI gate (ADR-0065 §10, §11):

- probe first: does cargo-dist 0.32 package `tessera-capi`'s cdylib/staticlib with the header and the config files as included files, alongside the `tessera-cli` binaries, with `precise-builds = true`? Record the probe in the PR;
- the `[dist]` table sets `features = ["static-hdf5"]` workspace-wide (`dist-workspace.toml:27`) and `tessera-capi` has no such feature: scope it to `tessera-cli` (per-package dist config), do not add an inert feature to the bridge;
- add the bridge to the dist build for the four targets in `dist-workspace.toml`;
- an `abidiff` (libabigail) check comparing the built `.so` with the last released one, failing on an ABI change without the matching `TSRA_ABI_*` bump; active from the second release that carries the bridge.

## Acceptance
- A release dry run attaches the library archives; the abidiff check fails on a deliberate signature change (shown once).

Size: ≤ ~300 lines. Depends on: #815. The glibc floor and the cluster package channel are pending the owner's consumer surveys.



