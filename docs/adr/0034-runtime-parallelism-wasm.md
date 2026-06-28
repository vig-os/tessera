# ADR-0034 — Runtime, parallelism & the wasm boundary

Status: **Proposed** (2026-06-28) · Relates ADR-0002 (concurrency), ADR-0026 (streaming table
ingest), ADR-0023/0024 (codecs). Tracks #224. Records the execution model and reconciles two
as-built drifts found during the #222/#224 work.

## Context
The concurrency model (ADR-0002) settled "sync API, smol bridge, no tokio runtime." Building the
streaming/ingest paths surfaced two facts that needed an authoritative home, plus one open decision:

1. **Vortex is wasm-capable.** `vortex-io` ships a `WasmRuntime` (`wasm_bindgen_futures::spawn_local`,
   gated `cfg(all(target_arch="wasm32", target_os="unknown"))`); the native `smol` executor is gated
   *off* for wasm. The earlier "Vortex/zarrs are non-wasm" framing (flake comment + memory) was wrong.
2. **tokio is compiled but never run.** `vortex-io`'s default features pull `tokio` (`rt`/`sync`/
   `io-util`/`bytes`); we instantiate **no** tokio runtime (smol `CurrentThreadRuntime` is the bridge)
   and `rt-multi-thread` is not enabled. ADR-0002's "no tokio in the tree" was inaccurate (now fixed).
3. **Open:** can a single canonical listmode block core-scale its encode? (The ADR-0026 fork.)

## Decision
1. **Public API stays synchronous** (reaffirms ADR-0002) — a data-format library must not impose an
   async runtime on its callers.
2. **Parallel encode = std-thread / rayon, never tokio.** `StreamWriter` already parallelizes encode
   **across blocks** on std threads (each block gets its own per-call smol runtime). The transpose
   (AoS→SoA, `ge_hdf5`) is pure CPU / zero-I/O / zero-async → wasm-portable and embarrassingly
   parallel per slab. ADR-0002 explicitly sanctions this (rayon/std parallel encode "inside the codecs").
3. **Single-block parallelism = Option A (multi-block decomposition), NOT internal tokio.** A single
   Vortex chunked block is encoded over a *sequential* `ArrayStream` on a single-thread runtime, so it
   cannot std-thread-parallelize byte-identically; the only multi-threaded vortex runtime is tokio,
   which is the *wrong tool* for CPU-bound compression (async ≠ parallelism) and re-adds the dep
   ADR-0002 avoids. Instead, a large acquisition becomes a **deterministic sequence of blocks** (a
   fixed block-row rule) encoded in parallel across cores by the std-thread pool — identity depends on
   data + the fixed rule, never on worker count. This is a frozen-format change (golden-corpus regen +
   a reader/`row_index` that spans blocks) → sequenced and gated under **ADR-0026**.
4. **tokio is reserved for exactly one thing: the read-side async object-store / cloud range-read
   backend (#225).** Network range-reads are latency-bound — async fans out hundreds/thousands of
   concurrent GETs on a few threads (intra-ROI *and* cohort-scale). That un-defers ADR-0002 §4 as an
   **additive async variant behind the sync `Reader`** (which is already generic over `Read + Seek`);
   it does not color the public API. This is the *only* place tokio earns its keep.
5. **wasm boundary.** `tessera-core` compiles to wasm32 today (the `wasm-core` gate). The wasm blocker
   in `tessera-io` is the **zarrs array path** (`zstd-sys` C + `linux-raw-sys` filesystem syscalls) +
   `getrandom` (needs the `wasm_js` feature) — **not** Vortex. Decision: **feature-gate `zstd`**
   (default-on native, compiled out for wasm); pcodec stays the universal default. The richer
   "core + Vortex *table* decode in wasm" build (in-browser `.tsra` table-column decode, vs Arrow-JS
   handoff) is therefore reachable and tracked.

## Why
- **async ≠ parallelism.** Ingest/encode is CPU-bound (pcodec) and blocking-C (libhdf5) → OS threads.
  Network reads are latency-bound → async. Each tool where it fits; tokio only on the read/network side.
- **Determinism is the gate.** Single-block parallel encode can't be byte-identical without tokio;
  multi-block is the std-thread path ADR-0002 already blesses.

## Consequences
- tokio stays compiled-but-unused until the #225 object-store backend lands (then it's instantiated
  only inside that backend, behind the sync surface).
- `zstd` becomes a cargo feature (archival fallback + `auto` selector on native; absent on wasm).
- Multi-block listmode is a future frozen-format change owned by ADR-0026 (corpus regen gated).
- A wasm build spanning core + Vortex table decode is unblocked by the zstd feature-gate + getrandom
  `wasm_js`; the zarrs array path needs a non-filesystem store on wasm (future).
