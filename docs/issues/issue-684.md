---
type: issue
state: open
created: 2026-10-26T19:41:07Z
updated: 2026-10-27T08:34:34Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/684
comments: 1
labels: dependencies, security
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-28T07:54:45.476Z
---

# [Issue 684]: [chore(deps): remove the zstd advisory ignores (RUSTSEC-2026-0391/0398/0403/0410)](https://github.com/vig-os/tessera/issues/684)

## Why this exists

`tessera/deny.toml` ignores four RustSec advisories against the zstd crates (#683). No dependency bump can clear
them yet. None of them is reachable from tessera today, and clippy plus cargo-deny guards keep it that way. This
issue tracks removing the ignores.

| Advisory | Crate (locked) | Fixed in | What it is |
|---|---|---|---|
| [RUSTSEC-2026-0391](https://rustsec.org/advisories/RUSTSEC-2026-0391) | `zstd` 0.13.3 | `>= 0.14.3` | `with_context` stream encoder/decoder doesn't reset a reused context, so an abandoned stream leaks into the next one |
| [RUSTSEC-2026-0398](https://rustsec.org/advisories/RUSTSEC-2026-0398) | `zstd-safe` 7.2.4 | `>= 8.2.0` | `Seekable::decompress` calls `Vec::set_len` beyond capacity for an offset past the end (`seekable` feature) |
| [RUSTSEC-2026-0403](https://rustsec.org/advisories/RUSTSEC-2026-0403) | `zstd-safe` 7.2.4 | `>= 8.2.0` | `DCtx::decompress_stream` commits the output position on a failed legacy frame, exposing uninitialised memory (reached through `stream::write::Decoder`) |
| [RUSTSEC-2026-0410](https://rustsec.org/advisories/RUSTSEC-2026-0410) | `zstd-sys` 2.0.16 | `>= 2.1.2` | wasm32 allocator shim doesn't check allocation failure |

## Reachability (at the locked versions)

zstd reaches the graph by three paths: `parquet` 58.3.0 (tessera-cli, tessera-ingest), `zarrs` 0.23.13
(tessera-io, the `"zstd"` array codec), and, under tessera-cli's `sql` feature, `arrow-ipc` 58.3.0's `zstd` feature
(enabled by datafusion 54). arrow-ipc uses only `bulk::Compressor` / `bulk::Decompressor`
(`arrow-ipc-58.3.0/src/compression.rs:33-78`). tessera's own crates have no direct zstd calls.

- **0391: not reachable.**
  - parquet's codec uses only the bulk API: `zstd::bulk::Compressor` / `Decompressor`
    (`parquet-58.3.0/src/compression.rs:513-552`).
    `Compressor::compress` is `ZSTD_compress2` (`zstd-safe-7.2.4/src/lib.rs:288`), which resets the session first
    (`zstd-sys-2.0.16/zstd/lib/compress/zstd_compress.c:6575`). `Decompressor::decompress` is the one-shot
    `ZSTD_decompressDCtx` (`zstd-safe-7.2.4/src/lib.rs:936`), so one whole frame per call.
  - zarrs builds a fresh `bulk::Compressor` per chunk, then decodes with `bulk::decompress` or `zstd::decode_all`
    (`zarrs-0.23.13/src/array/codec/bytes_to_bytes/zstd/zstd_codec.rs:109,126,130`).
  - Neither calls a `with_context` constructor. The upstream fix (gyscos/zstd-rs@74ace2d) changes only
    `stream::raw::{Encoder,Decoder}::with_context`.
- **0398: not reachable.** The `seekable` feature isn't enabled on `zstd-safe` or `zstd-sys`
  (`cargo tree -e features -i zstd-safe`: `arrays experimental legacy std zdict_builder zstdmt`). The `Seekable` code
  isn't compiled.
- **0403: not reachable.** It needs `DCtx::decompress_stream` writing into a `Vec<u8>`, reached through
  `zstd::stream::write::Decoder`. The advisory itself says `stream::read::Decoder`, `decode_all` and the bulk API are
  not affected, and those are the only paths parquet and zarrs use (citations above).
- **0410: not reachable.** It's wasm32-only, and `zstd-sys` isn't in the `tessera-wasm` wasm32 graph
  (`cargo tree -p tessera-wasm -i zstd-sys --target wasm32-unknown-unknown` matches no package). Native targets
  don't compile the shim.

## Guards (correct by construction)

- `tessera/clippy.toml` `disallowed-methods` / `disallowed-types` reject, in tessera's own code: the four
  `zstd::stream::{raw,read,write}::*::with_context` constructors (0391), `zstd_safe::DCtx::decompress_stream`,
  and the `zstd::stream::write::Decoder` and `zstd::stream::raw::Decoder` types (0403). `raw::Decoder` is what
  `write::Decoder` is built on. Driving it through `Operation::run` or `zio::Writer` reaches `decompress_stream`
  inside zstd, where the method ban can't see it, so the type itself is banned. Every entry's `reason` points here.
- The bans only cover tessera's own code. Dependencies are covered by the reachability rule below.
- Every entry has `allow-invalid = true`, because crates that can't see zstd can't resolve the paths. The catch is
  that a misspelled or renamed path goes silent. All entries were proved to fire in #687: a scratch `zstd = "0.13"`
  dependency in tessera-io plus one call to each banned API made `cargo clippy -p tessera-io -- -D warnings` fail
  with one error per entry, including both `raw::Decoder` bypasses. **Repeat that check whenever zstd's version
  moves.**
- `tessera/deny.toml` `[[bans.features]]` denies the `seekable` feature on `zstd-safe` and `zstd-sys` (0398), so
  nothing in the graph can turn the affected code on. `Seekable` isn't nameable in clippy because the feature is off.
- 0410 has no code guard. Its precondition is a wasm32 build linking `zstd-sys`. Re-check the wasm graph whenever
  `tessera-wasm` gains a dependency.

## Blockers to a real fix

- **zarrs:** the newest release (0.23.14) and `main` still require `zstd = "0.13.3"`.
- **parquet:** 60.0.0 is the first release on `zstd ^0.14`, and it needs arrow 60. The vortex fork pin (0.75,
  #480) requires `arrow-* = "58.3"`, and the newest datafusion (55.2) is on arrow `^59.2`. Two arrow majors can't
  coexist in one graph.

Upstream arrow-rs ignores the same advisories as unaffected: apache/arrow-rs#11922.

## Exit criteria

Remove the four `deny.toml` ignores when **both** are true:

1. A zarrs release depends on `zstd >= 0.14.3`.
2. An arrow `>= 60` path exists through vortex and datafusion, so parquet can move to `>= 60`, **and** datafusion
   itself depends on `zstd >= 0.14.3` (datafusion 55.2 has a direct optional `zstd ^0.13`).

Then bump, check that `cargo tree -i zstd@0.13.3` matches nothing, and delete the ignores. Keep the clippy and
feature guards. A zstd bump moves the parquet lane's sealed decoder pre-image (`zstd=` is pinned in it,
`crates/tessera-ingest/src/decoder.rs:272`), so handle that as a Gate B change.

## Rule until then

**Re-verify the reachability above on every parquet, zarrs, arrow-ipc or datafusion bump.** A Renovate or manual
PR that touches any of them must cite this issue and confirm that the codec call sites above haven't moved to `with_context`, the
`seekable` feature, or `stream::write::Decoder`.

---

# [Comment #1]() by [gerchowl]()

_Posted on October 27, 2026 at 08:34 AM_

Follow-up from the #687 review: `tessera/clippy.toml` sets `allow-invalid = true`, so a banned path that stops resolving after a zstd 0.14 bump would silently guard nothing. The manual re-check above is the only defence. Make it automatic: a small check crate in CI (or a flake check) that calls each banned API and is EXPECTED to fail clippy with exactly the listed `disallowed` errors. Also: the `deny.toml` reason mentions "datafusion 55 (arrow 59)" as the upgrade target, while the lock pins datafusion 54.0.0; reword it so it's clear 55 is the target, not the current version.

