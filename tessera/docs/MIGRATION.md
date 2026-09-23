# Migrating a PET/CT study into Tessera (spike #235)

This runbook is for the operator with **real patient data** on disk who wants to land
one study into a Tessera **collection**: normalized, range-readable, cohort-pruneable
products on the hot tier + the original vendor file preserved bit-faithfully on the
cold tier, all joined by `derived_from` provenance edges.

> **STATUS:** spike — the shape is proven on synthetic data
> (`tessera/crates/tessera-cli/tests/migrate_petct_study.rs`). The template is real;
> several real-data backends (DICOM-series, NIfTI, GE-HDF5 listmode) already exist;
> the `roi` backend is still TODO.

> **NEVER COMMIT REAL PATIENT DATA.** Real DUPLET PET/CT studies live under
> `/home/larsgerchow/Data/HDD/data/sdsc_dump/GEDiscoveryMIGen2/...` and
> `/home/larsgerchow/Data/SSD/KSB/...`. These paths are PHI — manual-bench-only,
> never committed, never read into a test fixture.

---

## The shape (the WHY)

```
                        ┌──────────────────────────────────────────┐
                        │ collection.json                          │
                        │   study = "STUDY-EXAMPLE-2024-01"        │
                        │   members: [vendor-raw, recon-ct, recon-pt]
                        └─────────────┬────────────────────────────┘
                                      │
   ┌──────────────────────────────────┼──────────────────────────────┐
   ▼                                  ▼                              ▼
 vendor-raw.tsra                  recon-ct.tsra                  recon-pt.tsra
   product = blob                   product = recon                product = recon
   metadata.study = …               metadata.study = …             metadata.study = …
                                    metadata.modality = CT         metadata.modality = PT
   ◄──── derived_from ─────────────── │                              │
                                      ◄──── derived_from ────────────┤
   ◄──── derived_from ─────────────────────────────────────────────────┘
   (the AC-CT lineage)

   ▲ COLD TIER ▲                ▲ HOT TIER ▲                ▲ HOT TIER ▲
   bit-faithful                 Zarr v3 + pcodec            Zarr v3 + pcodec
   blake3-sealed                cohort-pruneable            cohort-pruneable
   audit / re-recon             range-readable              range-readable
```

* **Hot tier** (`recon` members): array data lives in Zarr v3 chunks compressed with
  pcodec. Cluster / cloud reads scan the manifest first, prune by `metadata.study`
  (or any other indexed field), and **only fetch the chunks they need**. The
  `tessera_io::cloud::cohort_prune_before_fetch_skips_non_matching_product`
  capstone test (`crates/tessera-io/src/cloud.rs`, gated on the live MinIO check)
  proves a non-matching product's events block is *never fetched* over the wire.

* **Cold tier** (`blob` member): the vendor file (`.7z` / `.l64` / DICOMDIR
  tarball — whatever the scanner emitted) sealed as opaque bytes with a blake3
  digest. `tessera extract` recovers it byte-identically. This is the
  source-of-record an auditor or a re-reconstruction toolchain reaches for when a
  hot-tier product turns out to have been built with a buggy pipeline.

`derived_from` edges pin each parent's `manifest_hash`; `tessera_core::provenance::
verify_chain` rejects any tampered chain. So the recon `derived_from` the blob is a
**cryptographic** lineage statement, not a documentation comment.

---

## The template

Edit a copy of `docs/examples/migrate-petct-study.toml`. Point each `input` / `inputs`
field at your real data under `/home/larsgerchow/Data/HDD/...`. Pick the right
**backend** for what your scanner gave you:

| You have                                                | `format =`        | Notes                                                                                                          |
|---------------------------------------------------------|-------------------|----------------------------------------------------------------------------------------------------------------|
| Per-slice DICOM `.dcm` files (most clinical CT/PET)     | `"dicom-series"`  | `inputs = [...]` — all slices in one product. Uniform shape/modality/rescale required (else rejected).         |
| Single multi-frame DICOM (`.dcm` enhanced CT)           | `"dicom"`         | `input = "..."`.                                                                                               |
| Pre-reconstructed NIfTI (`.nii` / `.nii.gz`)            | `"nifti"`         | `input = "..."`. Carries voxel size + affine.                                                                  |
| Headerless binary (vendor `.dat` / `.bin` / `.raw`)     | `"raw"`           | Supply `shape = [z, y, x]` + `dtype = "i2"` (or `f4`, …). Operator MUST add `[product.metadata] modality = …`. |
| GE Discovery MI Gen2 listmode (`LIST_*.h5`)             | `"hdf-compound"`  | `dataset = "events_2p"` / `"events_3p"` / `"singles"`. Streams above 256 MiB by default.                       |
| Anything else (`.7z`, `.l64`, PDF, logs, archives)      | `"blob"` / `"junk"` | Bit-faithful preservation, blake3-sealed. The cold-tier escape hatch.                                          |

Every product MUST carry `[product.metadata] study = "<your-study-id>"` (the `blob`
schema marks it `recommended` and the engine emits a `WARN` on stderr if absent —
the FAIR-completeness nudge). The `recon` schema marks `modality` as **required**;
the DICOM/NIfTI backends auto-populate it, the `raw` backend doesn't — supply it
explicitly as an fd5 `Coded` value:

```toml
[product.metadata]
study = "MY-STUDY-2024-01"
modality = { _vocabulary = "DICOM", _code = "CT" }
```

---

## Running it

```bash
# Build the CLI inside the project's nix devshell (needs HDF5 + clang):
nix develop -c cargo build -p tessera-cli --release

# Or, with a system HDF5_DIR set:
HDF5_DIR=/tmp/hdf5root cargo build -p tessera-cli --release

# Run the migration. NEVER commit the output dir — it contains PHI-derived bytes.
./target/release/tessera ingest \
  --spec docs/examples/migrate-petct-study.toml \
  --out /home/larsgerchow/Data/HDD/tsra/<STUDY-ID> \
  --workers 8 \
  --ram-budget 2G
```

You get:

```
/home/larsgerchow/Data/HDD/tsra/<STUDY-ID>/
  collection.json            # sealed MMR catalog (cohort root)
  blake3_<id1>.tsra          # vendor-raw (cold tier blob)
  blake3_<id2>.tsra          # recon-ct (hot tier)
  blake3_<id3>.tsra          # recon-pt (hot tier)
```

Verify + inspect:

```bash
tessera verify <out>/blake3_<id>.tsra      # re-hashes every block
tessera schema <out>/blake3_<id>.tsra      # checks the product's schema contract
tessera tree   <out>/blake3_<id>.tsra      # human-readable hierarchy
tessera inspect <out>/blake3_<id>.tsra     # one-line summary
```

---

## Determinism (the load-bearing guarantee)

Re-running the same spec on the same data produces **byte-identical** `.tsra` files +
a byte-identical `collection.json`. This means:

* you can re-ingest on a different host (different worker counts, different HDF5
  build) and get the same archive — proven by the determinism re-run assertion in
  `tests/migrate_petct_study.rs`;
* a collaborator can verify your archive against your published `collection.json`
  by re-running the spec on their copy of the source data;
* the `manifest_hash` / `content_hash` baked into each `derived_from` edge is a
  **stable** integrity anchor — not just a transient checksum.

Re-runs use the spec's `[collection].timestamp` (and the per-product timestamp it
flows into); the engine NEVER reads `Local::now()` or filesystem mtimes for
identity (`tessera_core::identity::normalize_timestamp`).

---

## Pushing to S3 / OCI (the eventual cloud move)

When the migration is done locally, push the collection to object storage so the
cluster / cloud read path can scan it cohort-aware:

```bash
# Per-product OCI push (with the `cloud` feature):
cargo build -p tessera-cli --release --features cloud
./target/release/tessera push <out>/blake3_<id>.tsra <registry>/<repo>:<tag>

# Reading is symmetric (any feature-built tessera, on any host):
./target/release/tessera inspect s3://<bucket>/<study>/<id>.tsra
./target/release/tessera verify  https://<host>/<bucket>/<study>/<id>.tsra
```

The cohort prune-before-fetch path lives in `tessera_io::cloud::ObjectStoreReader`
(file `crates/tessera-io/src/cloud.rs`); the live-MinIO test
`cohort_prune_before_fetch_skips_non_matching_product` proves the wire-saving
guarantee.

**Tier policy (the WHY of recon-hot / blob-cold):**

* Push the `recon` members to a hot bucket (e.g. NVMe-backed MinIO). Operators
  scanning the cohort fetch only the manifest + the per-chunk byte ranges their
  query touches — pcodec + Zarr v3 means `study=… AND modality=PT AND t in [t0,t1]`
  reads kilobytes, not gigabytes.
* Push the `blob` member to a cold archive (S3 Glacier, tape, off-site mirror).
  It's typically the largest member, rarely fetched, and the audit anchor — exactly
  the workload cold storage is built for. Operators with sufficient bandwidth can
  keep it on the hot tier too; the format makes no policy choice.

---

## What's complete vs. what's TODO

| Capability                                                | Status                                                                                  |
|-----------------------------------------------------------|-----------------------------------------------------------------------------------------|
| `vendor-raw` cold-tier blob                               | done — `tessera-ingest::blob`, bounded-memory streaming                                 |
| `recon` from DICOM single-frame                           | done — `tessera-ingest::dicom`                                                          |
| `recon` from DICOM series                                 | done — `tessera-ingest::dicom::read_series`                                             |
| `recon` from NIfTI                                        | done — `tessera-ingest::nifti`                                                          |
| `recon` from headerless raw                               | done — `tessera-ingest::raw` (operator supplies shape/dtype)                            |
| `listmode` from GE HDF5                                   | done — `tessera-ingest::ge_hdf5` (streaming above 256 MiB)                              |
| `derived_from` edges with parent `manifest_hash`          | done — engine threads them, `verify_chain` walks them                                   |
| `[product.metadata] study=` cohort tagging                | done — every backend (engine `apply_spec_metadata`)                                     |
| Determinism across re-runs                                | done — engine test + this spike's `migrate_petct_study.rs`                              |
| Wire-level cohort prune over MinIO                        | done — `tessera-io::cloud::cohort_prune_before_fetch_*`                                 |
| `roi` ingest backend (raster + parametric)                | **TODO** — interim: seal a label-volume NIfTI through `format = "nifti"` (yields `recon`) |
| DICOM RadiopharmaceuticalInformationSequence parsing      | **TODO** — operator supplies `tracer` + `decay_correction_reference` via `[product.metadata]` |
| DICOM-series de-identification                            | **TODO** — `DicomSeries { deidentify = true }` currently rejects with a clear error     |
| GE listmode → recon (TOF-OSEM) reconstruction             | **out of scope** — recon is upstream of Tessera; we carry whatever the toolchain emits   |

---

## Files in this spike

* `docs/examples/migrate-petct-study.toml` — the template (this is what an operator edits)
* `docs/MIGRATION.md` — this runbook
* `crates/tessera-cli/tests/migrate_petct_study.rs` — the worked demo (synthetic data)
* `crates/tessera-ingest/src/spec.rs::committed_petct_migration_toml_parses_and_validates`
  — drift guard, pins the template into the build
