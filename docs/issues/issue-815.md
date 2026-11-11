---
type: issue
state: open
created: 2026-11-11T01:18:49Z
updated: 2026-11-11T02:15:04Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/815
comments: 0
labels: feature, area:bindings
assignees: none
milestone: none
projects: none
parent: 799
children: none
synced: 2026-11-11T08:50:34.305Z
---

# [Issue 815]: [feature(capi): pkg-config, CMake config, nix package and a CMake C++ consumer check (ADR-0065 §11)](https://github.com/vig-os/tessera/issues/815)

Part of epic #799. Design: ADR-0065 (docs/adr/0065-c-abi-bridge.md, PR for #800). **Blocked until the owner accepts ADR-0065**; do not start before. Toward #800.

## What
Packaging (ADR-0065 §11):

- `lib/pkgconfig/tessera.pc`, `lib/cmake/Tessera/TesseraConfig.cmake` + version file, imported target `Tessera::tessera`, soname `libtessera.so.<ABI major>`;
- `packages.tessera-capi` in `flake.nix` (lib, header, config files);
- a flake check that builds a small CMake C++ consumer against the package with nanoarrow and runs: the 8-thread determinism case through the real ABI (ADR-0065 §7.4), and a schema-refusal case;
- the static archive's localisation as a build step (`ld -r --whole-archive` + `objcopy --keep-global-symbols=<tsra_ list>`, measured in PR #821's spike §6.2: ~90,400 globals → 8) and `nm -g` as a check that only `tsra_*` symbols are global; plus a link test beside a consumer's own `libzstd.a`, which fails before localisation with `multiple definition of 'FSE_versionNumber'`;
- `Libs.private` from rustc's `--print native-static-libs`.

## Acceptance
- The consumer check fails when the header, the `.pc` or the CMake file is broken (shown once).
- Measured in the PR: exported symbols of both artifacts.

Size: ≤ ~400 lines. Depends on: #814.



