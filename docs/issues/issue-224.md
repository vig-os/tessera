---
type: issue
state: closed
created: 2026-06-28T14:52:27Z
updated: 2026-09-28T17:54:26Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/224
comments: 2
labels: area:core, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:53:02.164Z
---

# [Issue 224]: [Runtime, parallelism & wasm boundary (ADR-0034): reconcile ADR-0002 tokio drift · native std-thread vs wasm single-thread · feature-gate zstd](https://github.com/vig-os/tessera/issues/224)

## What
Define and document Tessera's execution model across native + wasm, and reconcile as-built drift.

## As-built drifts to fix
- **ADR-0002 says "no tokio in the tree"** — but tokio IS compiled via `vortex-io`'s default features (rt/sync/io-util/bytes). We never *instantiate* a tokio runtime (smol `CurrentThreadRuntime` is the bridge). Fix the wording: tokio is a transitive compile dep; no tokio runtime is created; `rt-multi-thread` is NOT enabled.
- **flake `wasm-core` comment + memory say "Vortex/zarrs are non-wasm"** — wrong. `vortex-io` ships a `WasmRuntime` (`spawn_local`, gated `cfg(all(target_arch=wasm32, target_os=unknown))`); native `smol` is gated off for wasm. **Vortex is wasm-capable.** The wasm blockers are the **zarrs array path** (`zstd-sys` C + `linux-raw-sys` filesystem) + `getrandom` (needs `wasm_js` feature).

## Execution model to record
| axis | native | wasm32 |
|---|---|---|
| public API | sync (ADR-0002) | sync |
| Vortex runtime | smol CurrentThreadRuntime | WasmRuntime (spawn_local) |
| cross-block parallel | std-thread pool (StreamWriter), no tokio | none (1 thread) |
| single-block parallel | opt-in tokio multi-thread *(see ADR-0026 fork)* | none |
| codec | pcodec + zstd (feature) | pcodec only |
| transpose (AoS→SoA) | pure CPU, std-thread/rayon | portable, 1 thread |

## Decisions
- **Parallel encode = std-thread/rayon** (ADR-0002 already blesses this); tokio only for the contested single-block case (defer to ADR-0026 multi-block-vs-tokio fork).
- **Feature-gate `zstd`** (default-on native; off for wasm) — keep the archival fallback + `auto` selector on native, give wasm a C-free build. pcodec stays universal default.
- **wasm target boundary**: core + Vortex *table* decode is the reachable richer story (getrandom wasm_js); the zarrs *array* path needs a non-filesystem store on wasm + zstd off.

## Deliverables
- **ADR-0034** (this); update ADR-0002 (tokio reconciliation), cross-ref ADR-0026 (parallelism) + ADR-0023/0024 (zstd feature-gate).
- Fix flake `wasm-core` comment + memory.

Context: branch `spike/tessera-core`.
---

# [Comment #1]() by [gerchowl]()

_Posted on June 28, 2026 at 03:07 PM_

Cohort-scale read intent + the async object-store backend (the one legit tokio use = read-side network fan-out) tracked in #225. ADR-0034 should record: tokio is for read-side concurrency (object-store), NOT ingest/encode (CPU-bound → std-threads); single-block ingest parallelism = Option A (multi-block) not internal tokio.

---

# [Comment #2]() by [gerchowl]()

_Posted on September 28, 2026 at 05:54 PM_

Closing as **done** — verified on `origin/dev` in the 2026-09-28 backlog triage.

Evidence: ADR-0034 accepted ; c5a48a2 multi-block, 541180b tessera bench --auto, tokio drift reconciled f1e46cc.

https://claude.ai/code/session_01XdERKMVDAwfMJSKdTytNnK

