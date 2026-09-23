# Tessera — agent orientation (read this first)

**What:** Tessera = **fd5 v2** — a substrate-agnostic, Rust-native **FAIR data-product format**.
Keeps fd5's model (one immutable, content-hashed, self-describing FAIR product) but generalises
the substrate from a single HDF5 file → manifest + shape-dispatched storage blocks.

**Where:** repo `vig-os/tessera`. Default branch **`main`**; integration branch **`dev`** — branch from
`dev`, PR into `dev`. (The old `spike/tessera-core` branch was graduated in #322 and no longer exists;
so does the `~/worktrees/tessera-core` path. Allocate a worktree with `anvil-task new tessera <branch>` —
never share one clone between two concurrent agents.)
fd5 white-paper (the founding vision): `white-paper.md` at the repo root.

**Read in this order:**
1. `docs/rfc-tessera.md` — the design (§0 capstone = the decisions; §13 fd5 conventions; §14 impl-readiness).
2. `tessera/docs/FEATURE-MATRIX.md` — status + **passing gates** + perf SLA floors (the benchable baseline).
3. `tessera/docs/SPIKE-RESULTS.md` — the evidence; `TEST-PLAN.md` — the guarding tests.

## Architecture (settled, evidence-backed)
- **Volumes → zarrs/OME-Zarr · 64³ cubic chunks · `pcodec`** (lossless; −21% CT / −33% PET vs zstd).
- **Tables → Vortex** (smallest + O(1) random-take + filter-pushdown + zero-copy Arrow→DuckDB).
- **Codec = pcodec** universal (zstd = decades-stable fallback). Container irrelevant (Zarr ≡ HDF5).
- **Identity/integrity:** blake3 Merkle, **hash-on-write** (chunks born with hash → first-moment
  integrity), seal = hash-of-hashes. Merkle is **integrity-only** (Vortex owns random-access).
- **Layout:** canonical **single sealed `.tsra` (STORED zip64, range-readable)**; opt-in exploded
  S3 prefix (parallel-write/CoW); **OCI artifact** distribution; RO-Crate/DataCite/InvenioRDM discovery.
- **Write engine (`tessera-io`):** streaming — bounded RAM ring → rayon encode pool → durable
  fragment commits + incremental Merkle + registry watermark; spill on burst; **never encode on the
  DAQ hot path** (Vortex footer-at-end = crash-total-loss). Compaction forms the full Vortex column;
  seal at completion.
- **Ingest (`tessera-ingest`):** per-vendor reader plugins (DICOM/GE-HDF5/Siemens-binary/raw/NIfTI) —
  normalise vendor-proprietary at the door; verify PS3.15 + re-attest; bidirectional (+ DICOMweb VNA).
- **Schema:** versioned, extensible **product-schema registry**; engine is **schema-driven /
  domain-agnostic** (schemas are embedded data, not engine code).
- **Crates:** `tessera-core` (format/spine, no I/O) · `tessera-io` (write/read engine) · `tessera-ingest` (vendor decoders).

## Proven (run pre-push)
- **S13** ✓ pcodec+Vortex **bit-exact lossless** (incl float NaN/±inf/−0.0/denormal) — clinical gate.
- **S15** ✓ **writer-deterministic** (same input→same bytes → content_hash=identity). *Caveat:* cross-version
  (pre-1.0) / cross-arch untested → hedge = pin codec versions + ship vendored readers.

## Phase & next steps
**Do not read a static task list from this file — it rots.** The spike phase and the P0 ADRs are long
done; the build phase is deep in flight (33 ADRs in `docs/adr/`, version `0.1.0-alpha.1`, signing /
WASM / OCI / WORM / ingest all shipped). Get current status from, in this order:

1. `tessera/docs/FEATURE-MATRIX.md` — what passes, and the test+gate proving each row.
2. `docs/adr/README.md` — the ADR index and status table.
3. `gh issue list` / `gh pr list` — the live backlog.
4. `tessera/docs/ROADMAP.md` — dependency order (phases → release gates).

## Dev environment (Nix + guardrails)
The whole repo is Nix-managed. **`cp .envrc.example .envrc && direnv allow`** (`.envrc` is per-developer + gitignored, #430; or `nix develop`) at the repo root loads the
`tessera-dev` devShell — pinned Rust toolchain (`tessera/rust-toolchain.toml` via rust-overlay),
Python 3.12 + `uv` (bench deps live in `uv.lock`, not Nix), native build deps (openssl/cmake/clang/
hdf5/zstd), and the **guardrails** toolbelt. Inputs come from the shared `/nix/store` (hot cache);
`sccache` (`RUSTC_WRAPPER`) shares compiled crates across every worktree — a fresh worktree's first
build is ~link time.
- **Governance = [gerchowl/guardrails](https://github.com/gerchowl/guardrails)** via `flake.nix`.
  `prek` (Rust pre-commit runner) auto-installs commit + push hooks on shell entry. Agent-drift gates:
  `no-fake-impl`, `no-debug-leftovers`, `no-commented-code`, `no-conflict-markers`, `derived-docs`,
  `gitleaks`, plus `rustfmt`/`clippy -D warnings`/`cargo-deny` (scoped to `tessera/`). Tune via
  `.pre-commit-config.yaml` · `tessera/deny.toml` · `tessera/perf-budgets.toml`. **`guardrails info`**
  lists every gate + knob. Escape one line with a trailing `guardrails-ok`.

## Working rules (this project)
- **Bench/verify empirically before claiming** — real DUPLET data at
  `/mnt/HDD/data/sdsc_dump/GEDiscoveryMIGen2/Projects/DUPLET-Patients/` and per-date studies under
  `…/GEDiscoveryMIGen2/2023|2024/<date>/<examid>/`. The `~/Projects/tessera` uv env has
  pcodec/vortex/zarr/duckdb/blake3/pydicom/hdf5plugin/numcodecs installed (`uv run python …`).
  Bench scratch: `…/processed/_bench/` (kept: `fd5_product/`, `h5_int16_slice_gzip4.h5`).
- **ALOCA** — concise, decision-line-per-item; lead with the verdict + the number that drives it.
- **The gate is `nix flake check`** at the repo ROOT — not `cargo test`. It is hermetic and supplies
  hdf5 + libclang, which the `tessera-ingest` / `tessera-cli` crates need and plain cargo lacks; a
  green `cargo test` with a red flake check is the normal failure mode, not a fluke.
  - `nix flake check -L --keep-going` — **always pass `--keep-going`**, or it stops at the first
    failing attribute and you pay a full cycle per failure.
  - Single check, warm, seconds: `nix build -L .#checks.x86_64-linux.<attr>` (`workspace-clippy`,
    `workspace-test`, `workspace-fmt`, `workspace-doctest`, `sql-tests`, `tessera-py-import`,
    `guardrails-gates`, …). Cold first run is ~25 min locally, ~46 min per arch in CI.
- **Tests:** `cd tessera && cargo nextest run` — **346** tests. Two traps:
  - `tessera-cli` is **bin-only** (use `--bins`, not `--lib`) and its `sql` module is behind
    `--features sql`; bare `cargo test` silently covers neither the SQL nor the cloud paths. The
    flake has dedicated `sql-tests` / `minio-range-read` checks for exactly this.
  - **nextest runs one process per test**, which hides cross-call regressions (a dropped runtime
    poisoning a cached session passed 345/345 and was caught only by `tessera-py-import`). Issue
    #356 is the mirror case — it fails only in shared-process `cargo test`. Both isolation modes
    hide a different bug class; the flake is the only thing that runs both.
- **Commits:** commit inside `nix develop` so the `prek` gates fire. If a hook fails to *install*,
  that is a bug worth fixing at the source — do not reach for `--no-verify` by reflex, because it
  disables every other gate in `.pre-commit-config.yaml` too. (Commit signing is unavailable on the
  agent VMs, so `-c commit.gpgsign=false` is expected; that is not the same as skipping gates.)
