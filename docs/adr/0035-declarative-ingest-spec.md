# ADR-0035 — Declarative ingest spec + general engine

Status: **Proposed** (2026-06-29) · Implements the second half of ROADMAP **P5** (vendor raw →
collection). Relates ADR-0020 (identity / canonical-JSON), ADR-0025 (ingest model), ADR-0026
(streaming table ingest), ADR-0033 (collections), ADR-0034 (runtime / parallelism).

## Context
ADR-0025 settled "ingest = normalise at the door." The first cut hard-coded each backend's call
site (DICOM, GE-HDF5, NIfTI, raw). A real vendor acquisition is a **multi-product collection** —
GE listmode brings `singles` + `coin_2p` + `coin_3p` (with their distinct on-disk record layouts +
distinct row indices) AND a derived recon, all of which share a study (ADR-0033). Hardcoding each
layout in Rust grows linearly with vendor work; meanwhile the underlying **backends** are a closed
handful (HDF5 compound, DICOM single + series, NIfTI, raw binary). Separating "what to ingest"
(config) from "how to decode" (Rust, closed set) makes the engine schema-driven (ADR-0025's spirit,
extended to whole collections).

A spike on the ingest paths surfaced **five must-fix holes** that informed this ADR:

1. The four convenience builders (`dicom::to_recon_product`, …) took only one `source: &str` — they
   produced a single `ingested_from` edge with no `content_hash`. A `derived_from` chain cannot
   `verify_chain` against a typed parent if the edge can't carry the parent's `manifest_hash`.
2. `ge_hdf5::to_listmode_product` hard-coded `"events"` as the block name and `"ms"` as the row
   index — fine for the GE 2p/3p layouts the conformance corpus pins, but a generic engine
   ingesting `coin_3p` keyed on `time_ps` needs them as parameters (with the historical defaults).
3. The collection has no `sources` field — adding one would bump the format seal and poison the
   corpus. The spec-hash provenance must live on EACH MEMBER as an `ingested_via_spec` edge.
4. `--auto` is **runtime measurement** of read vs encode rates (it picks the worker knee). It is
   not a property of the data — it cannot live in a spec.
5. `derived_from` references in a v1 spec must resolve to spec-local product names only — cycles,
   dangling references, and duplicate names are parse-time errors.

## Decision
1. **Format-tagged enum** for backend options. One TOML file, `format = "hdf-compound" | "dicom" |
   "dicom-series" | "nifti" | "raw"`. Adding a new BACKEND (a novel binary container) is a new
   Rust reader + a new enum variant + a dispatch arm in `engine::run`. Adding a new dataset
   **layout** in an existing backend is config — no Rust change.
2. **Spec-as-member-provenance.** The engine `blake3`s the canonical-JSON of the parsed model
   (RFC 8785 JCS, via `tessera_core::canonical`), pins that hash to each produced member's
   `Source { role: "ingested_via_spec", reference: <spec_path>, content_hash: Some(<spec_hash>) }`
   edge — never on the collection. The collection's bytes-on-disk are untouched (no format change,
   no corpus regen).
3. **Streaming-vs-batch heuristic.** For `hdf-compound` with `streaming = "auto"` (the default),
   the engine measures `rows × row_bytes` from the dataset descriptor (no payload bytes touched)
   and streams when the estimate exceeds `--stream-threshold` (default 256 MiB). `"stream"` and
   `"batch"` force the path. For non-HDF formats `"auto"` is a batch (and an explicit `"stream"`
   warns).
4. **Closed-backend-set boundary.** The set of supported `format = "…"` tags is **Rust-side**:
   a new CONTAINER (a novel binary file with its own bytes-on-disk) needs a Rust reader; a new
   dataset LAYOUT inside an already-supported container needs only TOML. This is the line the
   spec model holds.

## Why
- **DRY / SSoT.** Hardcoded callsites in `tessera-cli`'s `IngestSrc::{Dicom,GeHdf5}` collapsed
  into "build a 1-product IngestSpec → engine::run" — one dispatcher for both per-format and
  declarative paths.
- **Vendor-format scaling.** Every new GE / Siemens / vendor dataset layout we encounter adds a
  TOML, not Rust.
- **Determinism is load-bearing.** Spec identity is the canonical JSON of the parsed model, so
  whitespace, key-order, and comments in the source TOML cannot change the spec hash. Member
  identity (`{product, name, timestamp}` via UTC normalisation) and member order (preserved from
  the `[[product]]` declarations) make re-runs byte-identical (proven by a test).
- **Honest runtime separation.** `--workers`, `--ram-budget`, and `--auto` are CLI flags only,
  per hole #4 — they affect throughput, never bytes.

## Consequences
- **New module** `tessera-ingest::spec` (TOML parse + topological validation: Kahn's algorithm,
  rejects cycles / dangling / duplicate names).
- **New module** `tessera-ingest::engine` (dispatch + spec hashing + collection assembly).
- **Builder signatures extended.** Each per-backend `to_*_product` (DICOM / NIfTI / raw /
  GE-HDF5 batch + streaming) takes an additional `extra_sources: &[Source]` slice (the typed
  edges the engine appends — `derived_from` + `ingested_via_spec`).
- **GE-HDF5 lifted.** `to_listmode_product` + `stream_to_listmode_product_2p_to_file` accept
  `block_prefix` and `row_index` params (defaults `"events"` / `"ms"`); the conformance corpus
  bytes stay identical under the defaults (no regen).
- **CLI** grows `tessera ingest --spec FILE [--out DIR] [--workers N] [--ram-budget BYTES]
  [--auto] [--stream-threshold BYTES]`. The existing `dicom` / `ge-hdf5` subcommands are now
  thin shims that synthesise a 1-product `IngestSpec` and run the engine (net code delete).
- **Forwards compatibility.** v1 limits `derived_from` to in-spec references; cross-spec links
  (a re-ingest that derives from an earlier collection's member) are a v2 extension once the
  read-path catalog story (ADR-0033 / #225) lands.

## Out of scope
- Per-product timestamp overrides (v1: the collection's timestamp is each member's timestamp).
- Spec-side parallelism / RAM hints (deliberately CLI-only, per hole #4).
- New container formats (a new spec variant requires a Rust reader — see the closed-backend-set
  boundary above).
