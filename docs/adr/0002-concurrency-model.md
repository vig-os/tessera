# ADR-0002 — Concurrency model (register D2): synchronous API, dependency-local runtimes

Status: **Accepted** (2026-06-26, as-built) · Register: D2 · Supersedes the original proposal
(async `io` on tokio + `object_store` + a rayon encode pool with a `spawn_blocking` boundary).

## Context
The original D2 proposal was an async I/O layer (tokio + `object_store`) with a rayon encode pool.
When `tessera-core`, `tessera-io` (array/table/container/write engine), and `tessera-ingest` were
actually built (P1–P6), a simpler model proved sufficient and was adopted.

## Decision
1. **The public API is fully synchronous** across all crates. `tessera-core` is pure-sync;
   `tessera-io` (`pack`/`Reader`/`array`/`table`/`WriteSession`) and `tessera-ingest` expose blocking
   functions. No `async fn` in the public surface, no executor in the caller's face.
2. **Async stays a dependency-internal detail.** The only async is inside Vortex (the table codec);
   it is bridged with a **per-call `CurrentThreadRuntime`** (from `vortex-io`, a `smol` executor) —
   **no tokio runtime is ever instantiated**.
3. **Durability is fsync-based**, not an async write pool: `WriteSession` writes each fragment +
   journal line with `sync_all()` (ADR-0023/0024 + S17). Determinism, not throughput, is the gate.
4. **Deferred:** an async `object_store`/HTTP range-read backend (S6/distribution). The `Reader` is
   already generic over `Read + Seek`, so that backend slots in later behind the same surface without
   changing the sync public API; if/when it lands it will be an additive async *variant*, not a
   rewrite.

## Why
- A synchronous API is far simpler to consume, test, and reason about for a data-format library;
  it composes with any runtime the caller already has rather than imposing tokio.
- The encode/seal path is CPU-bound and already fast (pcodec 113 MiB/s, blake3 6 GiB/s, Vortex
  214 MiB/s); a sync path meets the §D floors without an async pool.
- Avoiding a global tokio runtime keeps the dependency tree smaller and the hermetic build simpler,
  and sidesteps coloring the whole API async for a benefit only the cloud-range-read layer needs.

## Consequences
- No tokio *runtime* is instantiated; the one async dependency (Vortex) is contained behind a blocking
  `smol` bridge. **Correction (as-built, 2026-06-28):** tokio the *crate* IS compiled — `vortex-io`'s
  default features pull `tokio` with `rt`/`sync`/`io-util`/`bytes` — so "no tokio in the tree" was
  inaccurate. We never create a tokio runtime and `rt-multi-thread` is not enabled; the public API stays
  sync. Enabling `vortex-io`'s `tokio` feature (multi-thread) is the only route to single-block parallel
  encode — deferred to the ADR-0026 fork and the runtime/parallel/wasm definition in **ADR-0034** (#224).
- Cloud/object-store range-reads remain a future additive layer (the property is already validated
  synchronously via `range::CountingReader`).
- Rayon-based parallel encode (a perf optimization) can be added inside the codecs later without
  affecting the API.
