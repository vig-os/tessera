# ADR-0002 — Concurrency model (register D2): synchronous API, dependency-local runtimes

Status: **Accepted** (2026-06-26, as-built) · Register: D2 · Supersedes the original proposal
(async `io` on tokio + `object_store` + a rayon encode pool with a `spawn_blocking` boundary).

**Revision (2026-06-29, #225 landed):** §4's deferred async `object_store` read backend has now
landed as the additive `cloud` variant on `tessera-io` — `cloud::ObjectStoreReader` adapts the
async store to `Read + Seek` behind a **per-reader current-thread Tokio runtime** (the legitimate
read-side use carved out in **ADR-0034 §4**), and `cloud::open_url` is the convenience entry
point. The synchronous public `Reader::from_reader` surface is unchanged — local-file callers
are bit-identical to before, and cloud callers get the same sync API. `tokio` is instantiated
only when the `cloud` feature is enabled; default-feature builds + the `wasm-core` gate are
unaffected.

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
4. **Landed (#225, 2026-06-29):** an async `object_store`/HTTP range-read backend
   (`tessera_io::cloud`, behind the optional `cloud` feature). `ObjectStoreReader` adapts
   `object_store::ObjectStore` to `Read + Seek` so the existing `Reader::from_reader` code path
   serves an S3 / HTTP object as-is; `open_url` is the convenience entry. A 64 KiB **tail
   prefetch** strips the EOCD/central-directory GETs from open (verified by `get_count`), and a
   `LogicalTableView::select_blocks_overlapping` query that misses a product's stat range never
   fetches that product's data block — proven by the cohort test against MinIO. Originally
   "deferred", as predicted: an additive async **variant** behind the unchanged sync public API,
   not a rewrite.

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
- Cloud/object-store range-reads landed as the additive `cloud` feature (`tessera_io::cloud`),
  per the revision note above. The local `range::CountingReader` proof carried over to the wire
  via the `minio-range-read` flake check; the cohort prune-before-fetch extension is the vision
  capstone (cross-product prune over S3).
- Rayon-based parallel encode (a perf optimization) can be added inside the codecs later without
  affecting the API.
