---
type: issue
state: open
created: 2026-11-11T01:18:46Z
updated: 2026-11-11T04:29:44Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/813
comments: 0
labels: feature, area:bindings
assignees: none
milestone: none
projects: none
parent: 799
children: none
synced: 2026-11-11T08:50:35.433Z
---

# [Issue 813]: [feature(capi): table lanes over the Arrow C Data Interface: tsra_table_open/push/mark_done/close, schema checks, canonicalisation (ADR-0065 §2.1, §7, §8)](https://github.com/vig-os/tessera/issues/813)

Part of epic #799. Design: ADR-0065 (docs/adr/0065-c-abi-bridge.md, PR #819). **Blocked until the owner accepts ADR-0065**; do not start before. Toward #800, #801, #802.

## What
Tables over the ABI (ADR-0065 §2.1, §3, §7, §8):
- `tsra_table_open(session, role, const tsra_table_options*, tsra_table**)`: one lane per table role declared by the session's schema (#809), with its own `seq` stream: `first_seq`, `seq_block` (`B`; `0` = no block cut; `B ≥ 1` only on an integer first key component, else `TSRA_E_ARG`) and `seq_key_base`, all per lane in `tsra_table_options` and sealed per role as `capi_seq`; and the set of optional columns it writes;
- `tsra_push(table, seq, ArrowArray*, ArrowSchema*)` (thread-safe; the batch's borrowed `ArrowSchema` compared with the declaration **before** import, a mismatch incl. a different child count or width is `TSRA_E_SCHEMA`, never a panic or an out-of-bounds read; the `ArrowArray` always moved via `FFI_ArrowArray::from_raw`, imported with `from_ffi(array, &schema)`, then `validate_full`), `tsra_mark_done(table, seq)`, `tsra_table_close(table, expected_count)` onto #812/#833;
- canonicalisation through `tessera-ingest`'s arrow lane: `canonicalise_floats` per the owner's H4 decision (ADR-0065 §1), its transform recorded (#810); not `normalise_null_slots` (the encoder already normalises masked slots); declared nullability by wrapping;
- `tsra_session_abort`, and the window / `max_ahead` / `refuse_instead_of_block` / `stall_timeout_ms` options passed to #812.

## Acceptance
- A bridge-written table equals the batch `ProductBuilder` product of the same rows (byte-identical) where the declared nullability matches the data and the table is NaN-free.
- A two-table product written concurrently seals the same bytes for any interleaving (through #832).
- An `i32` batch where `i64` is declared, and a batch with a missing child, are refused with `TSRA_E_SCHEMA`.
- The producer's `release` runs exactly once, inside `tsra_push`, on the pushing thread (counting release callback, success and every refusal path).
- Refusals: undeclared role, wrong column order/dtype, recorded-transform Arrow types, a null in a non-nullable column, an optional column outside the opened set.
- The 16-thread #834 case through the `extern "C"` functions.

Prototype to start from: PR #821. Size: ≤ ~400 lines. Depends on: #809, #810, #811, #812, #833.

