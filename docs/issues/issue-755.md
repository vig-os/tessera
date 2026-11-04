---
type: issue
state: open
created: 2026-11-03T13:13:28Z
updated: 2026-11-03T13:13:28Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/755
comments: 0
labels: refactor, priority:low, area:core
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-04T08:35:53.982Z
---

# [Issue 755]: [refactor: three single-owner fixes (rescale rule, spec-only Block::digest via public add_block, .cidx name)](https://github.com/vig-os/tessera/issues/755)

Found by an independent SOLID/DRY review of dev 74fc409 (astra + Fable, cross-checked by a third verifier). Confirmed by the verifiers; low severity, filed so they don't drift.

1. **The rescale rule is owned three times.**
   - Core defaults each missing field: `core/block/array.rs` ~201, `slope.unwrap_or(1.0)`, `intercept.unwrap_or(0.0)`.
   - The CLI demands both: `nav.rs` ~2212 and ~2422 refuse with "carries no rescale_slope/intercept".
   - `nav.rs` ~1916-1917 `.zip()`s them and silently drops the physical stats.
   - No producer on dev writes only one of the two (all go through `with_rescale`), but a foreign or hand-edited manifest, or the public fields, can. Fix: one `ArraySpec::physical(&self) -> Option<Rescale>` that all three use, with a decided rule for a lone field.
2. **`Block::digest` is spec-only behind a public `add_block` (LSP/ISP).**
   - `core/block/array.rs` ~420-427 and `table.rs` ~175-178 (with a stale "Spike" comment) digest the spec, not the payload.
   - `ProductBuilder::add_block` (`product.rs` ~66) seals a manifest that no packed payload verifies against. Every caller is test-only.
   - Fix: remove `add_block` from the public API (tests use `add_block_ref`), or make it take the payload digest. This is a public API change, and should be made before 1.0.
3. **The `.cidx` sidecar name is spelled in three places:** `core/block/partition.rs` ~60 (`leaf_suffix`), `io/chunk_index.rs` ~21-23 `cidx_name`, and `cli/nav.rs` ~1816 `format!("{block}.cidx")` (`version.rs` uses `cidx_name`). Fix: `CIDX_SUFFIX` and `cidx_name()` in core beside `PARTITION_CHILD_CLASSES`, reused by io and nav. Do this before #522/#675 change the sidecar class.

