# Tessera — agent orientation (read this first)

**What:** Tessera = **fd5 v2** — a substrate-agnostic, Rust-native **FAIR data-product format**.
Keeps fd5's model (one immutable, content-hashed, self-describing FAIR product) but generalises
the substrate from a single HDF5 file → manifest + shape-dispatched storage blocks.

**Where:** repo `vig-os/fd5`, branch **`spike/tessera-core`** (worktree `~/worktrees/tessera-core`).
fd5 white-paper (the founding vision): `~/Projects/fd5/white-paper.md`.

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
- **Layout:** canonical **single sealed `.tessera` (STORED zip64, range-readable)**; opt-in exploded
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
Spike phase **done** (S0–S15 + 3 fresh-agent reviews + corrections). **Build phase next** — do the
**P0 ADRs before code** (they poison fixtures if deferred): #20 canonical-JSON(JCS)+identity-reconciliation+manifest/BlockRef
schema · #22 versioning-DAG+`.tessera` container spec · #19 restore fd5 conventions+fields · #21
read-path/Reader-API+error-taxonomy+conformance-corpus. Then S5 (zarrs backend), S17 (write engine),
S9 (DICOM ingest), S16 (signing). Track everything against `FEATURE-MATRIX.md` gates.

## Dev environment (Nix + guardrails)
The whole repo is Nix-managed. **`direnv allow`** (or `nix develop`) at the repo root loads the
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
  `…/GEDiscoveryMIGen2/2023|2024/<date>/<examid>/`. The `~/Projects/fd5` uv env has
  pcodec/vortex/zarr/duckdb/blake3/pydicom/hdf5plugin/numcodecs installed (`uv run python …`).
  Bench scratch: `…/processed/_bench/` (kept: `fd5_product/`, `h5_int16_slice_gzip4.h5`).
- **ALOCA** — concise, decision-line-per-item; lead with the verdict + the number that drives it.
- **Tests:** `cd tessera && cargo test` (17 pass) — or `cargo nextest run`.
- **Commits:** inside the devShell, `prek` gates run (the intended path). From raw agent Bash
  (no devShell), the installed hook can't find `prek`/the gates, so commit with
  `git -c commit.gpgsign=false commit --no-verify` (signing key also absent here). Prefer running
  commits from within `nix develop` so the gates actually fire.
