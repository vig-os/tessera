---
type: issue
state: open
created: 2026-11-11T01:18:43Z
updated: 2026-11-11T01:18:43Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/811
comments: 0
labels: feature, area:bindings
assignees: none
milestone: none
projects: none
parent: 799
children: none
synced: 2026-11-11T08:50:36.500Z
---

# [Issue 811]: [feature(capi): tessera-capi skeleton: handles, status codes, last error, catch_unwind, ABI version, cbindgen header (ADR-0065)](https://github.com/vig-os/tessera/issues/811)

Part of epic #799. Design: ADR-0065 (docs/adr/0065-c-abi-bridge.md, PR for #800). **Blocked until the owner accepts ADR-0065**; do not start before. Toward #800.

## What
The `tessera-capi` crate skeleton (ADR-0065 §1, §3, §5, §10, §11):

- workspace member, `crate-type = ["cdylib", "staticlib"]`, lib name `tessera`, release-plz CHANGELOG;
- opaque session handle, `tsra_session_open` / `finish` (empty product) / `free`, `tsra_string_free`, `tsra_abi_version`, `tsra_version_string`, `TSRA_ABI_CHECK()`;
- status codes and the `tessera_core::Error` → status mapping with a `_ => TSRA_E_FORMAT` arm; thread-local `tsra_last_error`;
- `catch_unwind` at every entry; a caught panic poisons the handle; `build.rs` refuses `panic = "abort"`;
- cbindgen config, the committed `tessera.h`, and a header-drift check;
- the `capi-deps` flake check: no `hdf5*`/`dicom*` crate in `cargo tree -p tessera-capi`, and `arrow-array` present (anti-vacuity), modelled on PR #786's `identity-deps`;
- Gate B feature snapshots regenerated, with the delta shown to be only `tessera-capi` edges.

## Acceptance
- A test per status mapping; a test that a panicking entry returns `TSRA_E_PANIC` and poisons (via a test-only panic seam); a test that `tsra_last_error` is per-thread.
- The header-drift and `capi-deps` checks fail on a deliberate violation (shown once in the PR).

Size: ≤ ~400 lines (excluding the generated header). Depends on: #454 (the arrow lane must be HDF5-free).

