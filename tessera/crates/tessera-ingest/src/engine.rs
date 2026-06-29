//! Declarative ingest engine (ADR-0035) — runs an [`crate::spec::IngestSpec`] into a sealed
//! [`tessera_core::Collection`] of `.tsra` products.
//!
//! ## Execution model
//! 1. Parse + validate the spec ([`crate::spec::validate`]) → topological order, parents-first.
//! 2. Compute the **spec_hash** ([`crate::spec::spec_hash`]) over the canonical-JSON of the parsed
//!    model. The same hash flows into every produced member as a `Source { role:
//!    "ingested_via_spec", reference: <spec_path>, content_hash: Some(<spec_hash>) }` edge — that's
//!    how a Tessera reader links a sealed product back to the spec that built it.
//! 3. For each product in topological order, the engine:
//!    - builds `extra_sources` = typed `Source { role: "derived_from", reference: <parent.id>,
//!      content_hash: Some(<parent.manifest_hash>) }` edges (one per declared parent) followed by
//!      the `ingested_via_spec` edge. The `manifest_hash` on a `derived_from` edge is exactly what
//!      [`tessera_core::provenance::verify_chain`] checks.
//!    - dispatches on the format-tagged variant ([`crate::spec::FormatOptions`]) to the
//!      extended ingest builders ([`crate::dicom::to_recon_product`] / [`crate::nifti`] /
//!      [`crate::raw`] / [`crate::ge_hdf5`]).
//!    - writes the sealed product to `<out_dir>/<member-id>.tsra` (via `tessera_io::pack` or the
//!      streaming `_to_file` for the hdf-compound stream path).
//!    - records `(name → (id, manifest_hash))` so children can reference this product.
//! 4. Assemble the collection: [`tessera_core::CollectionBuilder`] preserves member order =
//!    declared `[[product]]` order, with `add_member` carrying `derived_from` ids.
//! 5. Write `<out_dir>/collection.json` (the canonical descriptor).
//!
//! ## Why a Collection has no spec field (the 3rd "hole" closed)
//! [`tessera_core::Collection`] has NO `sources` field — adding one would bump the format seal and
//! poison the conformance corpus. The spec-hash provenance lives on EACH MEMBER as an
//! `ingested_via_spec` edge instead. That's the right place anyway: a reader looking at one
//! `.tsra` immediately sees which spec produced it, without needing the collection descriptor.

use std::collections::BTreeMap;
use std::path::Path;

use tessera_core::collection::CollectionBuilder;
use tessera_core::manifest::Manifest;
use tessera_core::provenance::Source;
use tessera_core::{Error, Result};
use tessera_io::pack;

use crate::spec::{spec_hash, validate, FormatOptions, IngestSpec, StreamingMode};

/// The provenance-edge role tag the engine uses to record "this product was produced by running
/// spec X". One per member, pinned to the spec's `content_hash` (`blake3` over canonical-JSON of
/// the parsed model).
pub const SPEC_PROVENANCE_ROLE: &str = "ingested_via_spec";

/// Default stream-vs-batch threshold for `hdf-compound`: a product whose estimated payload (rows ×
/// row_bytes) exceeds this many bytes streams; below it batches. Picked to keep moderate-sized
/// acquisitions in the batch path (lower overhead, less staging churn) while real listmode scales
/// over the bounded-memory streaming path.
pub const DEFAULT_STREAM_THRESHOLD_BYTES: u64 = 256 * 1024 * 1024; // 256 MiB

/// Run a parsed ingest spec into a sealed [`tessera_core::Collection`] under `out_dir`.
///
/// `spec_path` is recorded verbatim in each member's `ingested_via_spec` edge (so a reader can find
/// the spec that produced the product). `cfg` controls the streaming write engine (workers + RAM
/// ceiling); `stream_threshold` is the byte ceiling above which `hdf-compound` with
/// `streaming = "auto"` flips to the streaming path.
///
/// `out_dir` is created if absent. Each member is written to `<out_dir>/<member-id>.tsra` (the
/// content-addressed id keeps file names stable across re-runs). The collection descriptor lands
/// at `<out_dir>/collection.json`.
pub fn run(
    spec: &IngestSpec,
    spec_path: &Path,
    out_dir: &Path,
    cfg: &tessera_io::WriteConfig,
    stream_threshold: u64,
) -> Result<tessera_core::Collection> {
    let order = validate(spec)?;
    let h = spec_hash(spec)?;
    let spec_ref = spec_path.display().to_string();

    std::fs::create_dir_all(out_dir).map_err(|e| {
        Error::Invalid(format!(
            "ingest-engine: create out_dir {}: {e}",
            out_dir.display()
        ))
    })?;

    // (name → (id, manifest_hash)) — the resolver `derived_from` edges look parents up in.
    let mut built: BTreeMap<String, (String, String)> = BTreeMap::new();

    // The collection's normalised timestamp IS the per-product timestamp — re-running the same
    // spec must produce byte-identical member ids, so this MUST go through the same
    // `normalize_timestamp` the manifest itself does (the manifest's `Manifest::new` does it too,
    // but doing it here keeps the engine's own determinism transparent in one place).
    let timestamp = tessera_core::identity::normalize_timestamp(&spec.collection.timestamp);
    for &idx in &order {
        let p = &spec.products[idx];
        let extra = build_extra_sources(p, &built, &spec_ref, &h)?;
        let (manifest, _payloads_written_in_dispatch) =
            dispatch(p, &extra, out_dir, cfg, stream_threshold, &timestamp)?;
        // Honor the fd5 schema contract AT INGEST: a product that claims a known schema must satisfy
        // it now, not only on a later `tessera schema`. Open-world → unknown product names pass.
        tessera_core::SchemaRegistry::builtin()
            .validate(&manifest)
            .map_err(|e| {
                Error::Invalid(format!(
                    "ingest-engine: member '{}' fails its declared schema '{}': {e}",
                    p.name, manifest.product
                ))
            })?;
        let id = manifest.id.clone();
        let mh = manifest.manifest_hash.clone().ok_or_else(|| {
            Error::Invalid(format!(
                "ingest-engine: member '{}' has no manifest_hash after seal",
                p.name
            ))
        })?;
        built.insert(p.name.clone(), (id, mh));
    }

    // Assemble the collection in DECLARED order (= TOML `[[product]]` order, not topo order — the
    // declared order is what authors see + what `content_hash` MMRs over). Resolve derived_from
    // *names* → ids via `built`.
    let mut cb = CollectionBuilder::new(
        &spec.collection.name,
        spec.collection
            .description
            .clone()
            .unwrap_or_else(|| "ingested via spec".to_string()),
        &spec.collection.timestamp,
    );
    if let Some(s) = &spec.collection.study {
        cb.with_study(s.clone());
    }
    for p in &spec.products {
        let (id, mh) = built
            .get(&p.name)
            .ok_or_else(|| {
                Error::Invalid(format!(
                    "ingest-engine: internal — product '{}' missing from built map",
                    p.name
                ))
            })?
            .clone();
        let parent_ids: Vec<String> = p
            .derived_from
            .iter()
            .map(|n| {
                built.get(n).map(|(pid, _)| pid.clone()).ok_or_else(|| {
                    Error::Invalid(format!(
                        "ingest-engine: internal — parent '{n}' missing from built map"
                    ))
                })
            })
            .collect::<Result<_>>()?;
        cb.add_member(id, mh, p.role, parent_ids);
    }
    let collection = cb.seal()?;

    let coll_path = out_dir.join("collection.json");
    std::fs::write(&coll_path, collection.to_json()?).map_err(|e| {
        Error::Invalid(format!("ingest-engine: write {}: {e}", coll_path.display()))
    })?;
    Ok(collection)
}

/// Build the typed `extra_sources` for one product: a `derived_from` edge per declared parent
/// (pinned to that parent's `manifest_hash`, so [`tessera_core::provenance::verify_chain`] can walk
/// the integrity chain), followed by the single `ingested_via_spec` edge to the spec file.
fn build_extra_sources(
    p: &crate::spec::ProductSpec,
    built: &BTreeMap<String, (String, String)>,
    spec_ref: &str,
    spec_hash_hex: &str,
) -> Result<Vec<Source>> {
    let mut out = Vec::with_capacity(p.derived_from.len() + 1);
    for parent_name in &p.derived_from {
        let (parent_id, parent_mh) = built.get(parent_name).ok_or_else(|| {
            Error::Invalid(format!(
                "ingest-engine: product '{}' derived_from '{parent_name}' but parent not yet built (topo order bug?)",
                p.name
            ))
        })?;
        out.push(Source::new("derived_from", parent_id).with_content_hash(parent_mh.clone()));
    }
    out.push(
        Source::new(SPEC_PROVENANCE_ROLE, spec_ref).with_content_hash(spec_hash_hex.to_string()),
    );
    Ok(out)
}

/// Dispatch one product to the appropriate backend, write the sealed `.tsra` under `out_dir`, and
/// return its manifest. The bytes are written here so streaming backends (`hdf-compound`) can stay
/// constant-memory end-to-end — the in-memory `Vec<BlockPayload>` only materialises for the small,
/// inherently in-memory backends (DICOM / NIfTI / raw single-volume).
#[allow(clippy::too_many_arguments)] // every argument is load-bearing context, no natural grouping
fn dispatch(
    p: &crate::spec::ProductSpec,
    extra_sources: &[Source],
    out_dir: &Path,
    cfg: &tessera_io::WriteConfig,
    stream_threshold: u64,
    timestamp: &str,
) -> Result<(Manifest, ())> {
    let name = p.name.as_str();
    // collection-level timestamp is the per-product timestamp too: the engine takes its identity
    // discipline from the spec, never from `Local::now()` or filesystem mtimes.
    let timestamp = timestamp.to_string();
    match &p.options {
        FormatOptions::Dicom { input, deidentify } => {
            let img = if *deidentify {
                crate::dicom::read_image_deidentified(input)?
            } else {
                crate::dicom::read_image(input)?
            };
            let source = input.display().to_string();
            let (m, payloads) =
                crate::dicom::to_recon_product(&img, name, &timestamp, &source, extra_sources)?;
            let m = seal_to_tsra(m, &payloads, out_dir, p, timestamp.as_str())?;
            Ok((m, ()))
        }
        FormatOptions::DicomSeries { inputs, deidentify } => {
            // PS3.15 de-id for a series is per-file on the raw object; the in-memory series builder
            // doesn't carry a "deidentify the series" hook, so we map the request to per-file
            // de-id by reading each slice through `read_image_deidentified` first and reusing the
            // single-slice stacker. Reject the request to keep the contract honest until the
            // dicom-series API grows a de-id seam.
            if *deidentify {
                return Err(Error::Invalid(
                    "ingest-engine: dicom-series with deidentify=true is not yet supported \
                     (use per-file dicom + a derived recon stage, or pre-de-identify the .dcm \
                     fixtures)"
                        .into(),
                ));
            }
            let img = crate::dicom::read_series(inputs)?;
            let source = inputs
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(",");
            let (m, payloads) =
                crate::dicom::to_recon_product(&img, name, &timestamp, &source, extra_sources)?;
            let m = seal_to_tsra(m, &payloads, out_dir, p, timestamp.as_str())?;
            Ok((m, ()))
        }
        FormatOptions::Nifti { input } => {
            let img = crate::nifti::read_nifti(input)?;
            let source = input.display().to_string();
            let (m, payloads) =
                crate::nifti::to_recon_product(&img, name, &timestamp, &source, extra_sources)?;
            let m = seal_to_tsra(m, &payloads, out_dir, p, timestamp.as_str())?;
            Ok((m, ()))
        }
        FormatOptions::Raw {
            input,
            shape,
            dtype,
        } => {
            let (m, payloads) = crate::raw::to_recon_product(
                input,
                shape.clone(),
                dtype,
                name,
                &timestamp,
                extra_sources,
            )?;
            let m = seal_to_tsra(m, &payloads, out_dir, p, timestamp.as_str())?;
            Ok((m, ()))
        }
        FormatOptions::HdfCompound {
            input,
            dataset,
            row_index,
            block_prefix,
            streaming,
            slab_rows,
        } => {
            let should_stream = resolve_streaming(*streaming, input, dataset, stream_threshold)?;
            if should_stream {
                // Stream straight to disk — the bounded-memory + multi-block path. The output path
                // is computed BEFORE the seal because the streaming writer writes the .tsra
                // directly; we then re-open the sealed manifest to record its id.
                let stage = out_dir.join(format!("__stage_{}", sanitize_filename(name)));
                let tmp_out = out_dir.join(format!("__pending_{}.tsra", sanitize_filename(name)));
                let source = input.display().to_string();
                // Build extra_sources with the canonical `ingested_from` flowing through the
                // streaming session (it adds its own `ingested_from`); pass `extra_sources` as-is.
                let m = crate::ge_hdf5::stream_to_listmode_product_2p_to_file(
                    input,
                    dataset,
                    name,
                    &timestamp,
                    *slab_rows,
                    &stage,
                    &tmp_out,
                    cfg,
                    block_prefix,
                    row_index,
                    extra_sources,
                    &p.metadata,
                )?;
                // Rename the pending .tsra to its id-named final path. Same filesystem → rename is
                // atomic, so a crash here leaves either the old or the new file in place.
                let final_path = out_dir.join(format!("{}.tsra", sanitize_filename(&m.id)));
                std::fs::rename(&tmp_out, &final_path).map_err(|e| {
                    Error::Invalid(format!(
                        "ingest-engine: rename {} -> {}: {e}",
                        tmp_out.display(),
                        final_path.display()
                    ))
                })?;
                // Tidy up the per-product stage dir — best-effort (failure here would not change
                // the sealed product's correctness, so it's not a hard error).
                let _ = std::fs::remove_dir_all(&stage);
                // Mention `source` so the SAFETY/docs trail stays correct even though the streaming
                // session records `ingested_from` itself.
                let _ = source;
                Ok((m, ()))
            } else {
                // Batch path: read the whole compound, build the in-memory product, pack.
                let cols = crate::ge_hdf5::read_compound(input, dataset)?;
                let source = input.display().to_string();
                let (m, payloads) = crate::ge_hdf5::to_listmode_product(
                    &cols,
                    name,
                    &timestamp,
                    &source,
                    block_prefix,
                    row_index,
                    extra_sources,
                )?;
                let m = seal_to_tsra(m, &payloads, out_dir, p, timestamp.as_str())?;
                Ok((m, ()))
            }
        }
    }
}

/// Resolve the streaming mode for an `hdf-compound` product: `auto` measures
/// `rows × row_bytes` via [`crate::ge_hdf5::compound_columns`] + the dataset shape; explicit
/// overrides bypass the measurement.
fn resolve_streaming(
    mode: StreamingMode,
    input: &Path,
    dataset: &str,
    stream_threshold: u64,
) -> Result<bool> {
    match mode {
        StreamingMode::Batch => Ok(false),
        StreamingMode::Stream => Ok(true),
        StreamingMode::Auto => {
            // Cheap probe: open the file, read the compound descriptor + dataset shape, close. The
            // numbers come from the descriptor — no payload bytes are decoded.
            let cols = crate::ge_hdf5::compound_columns(input, dataset)?;
            let row_bytes: u64 = cols
                .iter()
                .map(|c| {
                    u64::try_from(tessera_io::ColumnData::dtype_size(&c.dtype).unwrap_or(0))
                        .unwrap_or(0)
                })
                .sum();
            let n_rows = hdf_compound_rows(input, dataset)?;
            let estimated = row_bytes.saturating_mul(n_rows);
            Ok(estimated > stream_threshold)
        }
    }
}

/// Open the HDF5 file, read the row count of `dataset`, close. Cheap (no payload bytes touched).
fn hdf_compound_rows(input: &Path, dataset: &str) -> Result<u64> {
    use hdf5_metno as hdf5;
    let file = hdf5::File::open(input).map_err(|e| {
        Error::Invalid(format!("ingest-engine: open hdf5 {}: {e}", input.display()))
    })?;
    let ds = file
        .dataset(dataset)
        .map_err(|e| Error::Invalid(format!("ingest-engine: open dataset {dataset}: {e}")))?;
    let n = ds.shape().first().copied().unwrap_or(0);
    u64::try_from(n).map_err(|e| Error::Invalid(format!("ingest-engine: row count overflow: {e}")))
}

/// Seal one in-memory product to `<out_dir>/<id>.tsra`. Idempotent: the same manifest → same path.
/// Pack a built (batch-path) product to `<out_dir>/<id>.tsra`, **applying the spec's
/// `[product.metadata]` overrides first** so they ride the sealed `manifest_hash`. Returns the
/// manifest actually written (re-sealed if metadata was applied) so the engine validates + records the
/// SAME manifest it wrote. `id` is unchanged by the override (from_manifest keeps product/name/
/// timestamp), so the filename is stable; only the metadata + `manifest_hash` change.
fn seal_to_tsra(
    m: Manifest,
    payloads: &[tessera_io::BlockPayload],
    out_dir: &Path,
    p: &crate::spec::ProductSpec,
    _timestamp: &str,
) -> Result<Manifest> {
    let m = apply_spec_metadata(m, &p.metadata)?;
    let path = out_dir.join(format!("{}.tsra", sanitize_filename(&m.id)));
    pack(&m, payloads, &path)?;
    Ok(m)
}

/// If the spec declared `[product.metadata]`, return a manifest re-sealed with those fields applied
/// (overriding any builder default); else the manifest unchanged. Blocks are reused by digest
/// (`from_manifest`), so `content_hash`/`id` are stable — only the metadata + `manifest_hash` change.
/// The streaming path applies its overrides directly on the `WriteSession` (no post-seal re-build);
/// this is the batch-path counterpart.
fn apply_spec_metadata(
    m: Manifest,
    meta: &BTreeMap<String, serde_json::Value>,
) -> Result<Manifest> {
    if meta.is_empty() {
        return Ok(m);
    }
    let mut b = tessera_core::ProductBuilder::from_manifest(&m);
    for (k, v) in meta {
        b.with_field(k, v.clone());
    }
    b.seal()
}

/// Strip filesystem-hostile chars (`:` / `/` from the blake3-prefixed id) so the member's id
/// becomes a portable filename. The id is hex + a single `:`; replacing the `:` with `_` is
/// lossless (the prefix is fixed: `blake3:`).
fn sanitize_filename(s: &str) -> String {
    s.replace([':', '/', '\\'], "_")
}

/// Public seam: re-export so a CLI caller can pre-parse + re-use the same spec without re-reading.
pub use crate::spec::parse as parse_spec;

#[cfg(test)]
mod tests {
    use super::*;
    use hdf5_metno as hdf5;
    use hdf5_metno::H5Type;
    use std::path::PathBuf;

    const TS: &str = "2024-01-01T00:00:00Z";

    // Mirrors the GE 2p record; only used to write the synthetic .h5 fixtures the engine then
    // reads through `crate::ge_hdf5::read_compound` (generic, no per-record struct).
    #[repr(C)]
    #[derive(H5Type, Clone, Copy)]
    struct Rec2p {
        ms: u32,
        en: [f32; 2],
        ax: [u8; 2],
        tx: [u16; 2],
        vtx: [f32; 3],
    }

    fn write_synth_2p(path: &std::path::Path, n: usize, dataset: &str) {
        let recs: Vec<Rec2p> = (0..n)
            .map(|k| Rec2p {
                ms: k as u32,
                en: [511.0, 510.0 + k as f32],
                ax: [(k % 64) as u8, (k % 32) as u8],
                tx: [k as u16, (k as u16).wrapping_add(7)],
                vtx: [0.1 * k as f32, 0.2, 0.3],
            })
            .collect();
        let f = hdf5::File::create(path).unwrap();
        f.new_dataset::<Rec2p>()
            .shape(n)
            .create(dataset)
            .unwrap()
            .write(&recs)
            .unwrap();
    }

    /// Write a tiny `.toml` spec with two `hdf-compound` products + a derived_from edge.
    fn write_spec(spec_path: &std::path::Path, h5_a: &std::path::Path, h5_b: &std::path::Path) {
        // The spec's `name` + `timestamp` are deterministic — re-runs must reproduce identity.
        let s = format!(
            r#"
[collection]
name = "DP06-study"
description = "synthetic spec test"
timestamp = "{TS}"
study = "DP06-2024-01"

[[product]]
name = "DP06-raw"
role = "raw"
schema = "listmode"
format = "hdf-compound"
input = "{a}"
dataset = "events_2p"
streaming = "batch"

[[product]]
name = "DP06-derived"
role = "derived"
schema = "listmode"
derived_from = ["DP06-raw"]
format = "hdf-compound"
input = "{b}"
dataset = "events_2p"
streaming = "batch"
"#,
            a = h5_a.display(),
            b = h5_b.display()
        );
        std::fs::write(spec_path, s).unwrap();
    }

    #[test]
    fn run_synthetic_spec_seals_a_collection_chain_verifies_and_is_deterministic() {
        let dir = tempfile::tempdir().unwrap();
        let h5_a = dir.path().join("a.h5");
        let h5_b = dir.path().join("b.h5");
        write_synth_2p(&h5_a, 50, "events_2p");
        write_synth_2p(&h5_b, 50, "events_2p");
        let spec_path = dir.path().join("spec.toml");
        write_spec(&spec_path, &h5_a, &h5_b);

        let out = dir.path().join("out");
        let cfg = tessera_io::WriteConfig::for_system().workers(2);
        let parsed = parse_spec(&spec_path).unwrap();
        let coll = run(
            &parsed,
            &spec_path,
            &out,
            &cfg,
            DEFAULT_STREAM_THRESHOLD_BYTES,
        )
        .unwrap();

        // 1. the collection seals + has two members in TOML order.
        assert!(coll.is_sealed());
        assert_eq!(coll.members.len(), 2);
        assert_eq!(coll.study.as_deref(), Some("DP06-2024-01"));
        // 2. members were written to disk + open.
        let mut by_name: BTreeMap<String, Manifest> = BTreeMap::new();
        for m in &coll.members {
            let path = out.join(format!("{}.tsra", m.reference.replace([':', '/'], "_")));
            assert!(path.exists(), "missing {}", path.display());
            let r = tessera_io::Reader::open(&path).unwrap();
            by_name.insert(m.reference.clone(), r.manifest().clone());
        }
        // 3. the derived member carries a `derived_from` edge pinned to the raw's manifest_hash,
        //    AND a separate `ingested_via_spec` edge pinned to the spec_hash.
        let derived_id = &coll.members[1].reference; // declared order: raw, then derived
        let derived_manifest = by_name.get(derived_id).unwrap();
        let raw_id = &coll.members[0].reference;
        let raw_manifest = by_name.get(raw_id).unwrap();
        let raw_mh = raw_manifest.manifest_hash.clone().unwrap();
        let df = derived_manifest
            .sources
            .iter()
            .find(|s| s.role == "derived_from")
            .expect("derived_from edge missing");
        assert_eq!(df.reference, *raw_id, "edge must reference parent id");
        assert_eq!(
            df.content_hash.as_deref(),
            Some(raw_mh.as_str()),
            "derived_from edge MUST pin parent's manifest_hash (closes hole #1)"
        );
        let via = derived_manifest
            .sources
            .iter()
            .find(|s| s.role == SPEC_PROVENANCE_ROLE)
            .expect("ingested_via_spec edge missing on member");
        let expected_spec_hash = crate::spec::spec_hash(&parsed).unwrap();
        assert_eq!(
            via.content_hash.as_deref(),
            Some(expected_spec_hash.as_str()),
            "ingested_via_spec edge MUST pin the spec_hash (closes hole #3)"
        );

        // 4. verify_chain accepts the derived member against a resolver populated with the raw.
        let mut resolver: BTreeMap<String, Manifest> = BTreeMap::new();
        resolver.insert(raw_manifest.id.clone(), raw_manifest.clone());
        tessera_core::provenance::verify_chain(derived_manifest, &resolver).unwrap();

        // 5. Determinism: a second run produces a byte-identical collection + member manifests
        //    (same content_hash, same manifest_hash, same per-member ids — the load-bearing
        //    determinism gate this whole feature is built around).
        let out2 = dir.path().join("out2");
        let coll2 = run(
            &parsed,
            &spec_path,
            &out2,
            &cfg,
            DEFAULT_STREAM_THRESHOLD_BYTES,
        )
        .unwrap();
        assert_eq!(coll.id, coll2.id);
        assert_eq!(coll.content_hash, coll2.content_hash);
        assert_eq!(coll.manifest_hash, coll2.manifest_hash);
        for (a, b) in coll.members.iter().zip(coll2.members.iter()) {
            assert_eq!(a.reference, b.reference);
            assert_eq!(a.manifest_hash, b.manifest_hash);
            assert_eq!(a.derived_from, b.derived_from);
        }
        // and collection.json was written.
        assert!(out.join("collection.json").exists());
    }

    #[test]
    fn streaming_auto_above_threshold_takes_the_stream_path() {
        // Threshold of 1 byte forces 'auto' to pick streaming. Path equivalence with the batch
        // path is proven by the existing ge_hdf5 byte-identical test
        // (`whole_file_and_streamed_multi_block_match_at_n_blocks`) — here we just assert the
        // engine actually drives the streaming code (sealed manifest exists, the .tsra opens).
        let dir = tempfile::tempdir().unwrap();
        let h5 = dir.path().join("big.h5");
        write_synth_2p(&h5, 200, "events_2p");
        let spec_text = format!(
            r#"
[collection]
name = "stream-test"
timestamp = "{TS}"

[[product]]
name = "big"
role = "raw"
schema = "listmode"
format = "hdf-compound"
input = "{}"
dataset = "events_2p"
streaming = "auto"
"#,
            h5.display()
        );
        let parsed = crate::spec::parse_str(&spec_text).unwrap();
        let out = dir.path().join("out");
        let cfg = tessera_io::WriteConfig::for_system().workers(2);
        let coll = run(&parsed, &PathBuf::from("inline-spec"), &out, &cfg, 1).unwrap();
        assert_eq!(coll.members.len(), 1);
        let member_path = out.join(format!(
            "{}.tsra",
            coll.members[0].reference.replace([':', '/'], "_")
        ));
        assert!(
            member_path.exists(),
            "stream path must write {}",
            member_path.display()
        );
        tessera_io::Reader::open(&member_path).unwrap();
    }

    #[test]
    fn spec_product_metadata_is_applied_and_overrides_defaults() {
        // A listmode product defaults coincidence_mode to "prompt-coincidence"; the spec's
        // [product.metadata] must OVERRIDE that and add arbitrary fields — on BOTH the batch and the
        // streaming path. (Closes the gap where ProductSpec.metadata was parsed but ignored.)
        let dir = tempfile::tempdir().unwrap();
        let h5 = dir.path().join("a.h5");
        write_synth_2p(&h5, 50, "events_2p");
        let spec_path = dir.path().join("spec.toml");
        let s = format!(
            r#"
[collection]
name = "DP06-meta"
timestamp = "2024-01-01T00:00:00Z"

[[product]]
name = "raw-batch"
role = "raw"
schema = "listmode"
format = "hdf-compound"
input = "{a}"
dataset = "events_2p"
streaming = "batch"
metadata = {{ coincidence_mode = "extended-coincidence", operator = "DP" }}

[[product]]
name = "raw-stream"
role = "raw"
schema = "listmode"
format = "hdf-compound"
input = "{a}"
dataset = "events_2p"
streaming = "stream"
metadata = {{ coincidence_mode = "singles", site = "anvil" }}
"#,
            a = h5.display()
        );
        std::fs::write(&spec_path, s).unwrap();

        let out = dir.path().join("out");
        let cfg = tessera_io::WriteConfig::for_system();
        let parsed = parse_spec(&spec_path).unwrap();
        let coll = run(
            &parsed,
            &spec_path,
            &out,
            &cfg,
            DEFAULT_STREAM_THRESHOLD_BYTES,
        )
        .unwrap();
        assert_eq!(coll.members.len(), 2);

        for member in &coll.members {
            let p = out.join(format!(
                "{}.tsra",
                member.reference.replace([':', '/'], "_")
            ));
            let mani = tessera_io::Reader::open(&p).unwrap().manifest().clone();
            if mani.metadata.contains_key("operator") {
                // batch product: spec overrode the default + added a field
                assert_eq!(
                    mani.metadata.get("coincidence_mode"),
                    Some(&serde_json::json!("extended-coincidence")),
                    "batch: spec [product.metadata] must override the default"
                );
                assert_eq!(
                    mani.metadata.get("operator"),
                    Some(&serde_json::json!("DP"))
                );
            } else {
                // streaming product: same, on the bounded-memory path
                assert_eq!(
                    mani.metadata.get("coincidence_mode"),
                    Some(&serde_json::json!("singles")),
                    "stream: spec [product.metadata] must override the default"
                );
                assert_eq!(mani.metadata.get("site"), Some(&serde_json::json!("anvil")));
            }
        }
    }
}
