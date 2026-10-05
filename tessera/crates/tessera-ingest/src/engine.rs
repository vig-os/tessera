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

use tessera_core::collection::{
    member_filename, sanitize_reference, CollectionBuilder, MemberKind,
};
use tessera_core::manifest::Manifest;
use tessera_core::provenance::Source;
use tessera_core::{Error, Result};
use tessera_io::{pack, pack_streaming_verified, stamp_ingest_provenance, ProvenanceOptions};

use crate::spec::{spec_hash, validate, FormatOptions, IngestSpec, StreamingMode};

/// The provenance-edge role tag the engine uses to record "this product was produced by running
/// spec X". One per member, pinned to the spec's `content_hash` (`blake3` over canonical-JSON of
/// the parsed model).
pub const SPEC_PROVENANCE_ROLE: &str = "ingested_via_spec";

/// Loud warning when a DICOM ingest seals WITHOUT any de-identification. The raw header is no longer
/// embedded on this path (#269 leak fix — [`without_raw_header`]), but the curated `identifying`-tier
/// fields (patient pseudonym, study/series UIDs) are still sealed in the clear. Non-silent so an
/// operator can't ship identifiable data by forgetting a flag.
fn warn_dicom_not_deidentified() {
    tracing::warn!(
        target: "tessera::ingest",
        "PHI RISK: ingesting DICOM without de-identification — curated identifying fields (patient \
         pseudonym, study/series UIDs) are sealed in the clear. Pass --deidentify to strip PS3.15 \
         tags (ADR-0040), --recipient <age-pubkey> to crypto-shred (de-identify + keep a recoverable \
         encrypted copy, ADR-0047), and/or --source-label to redact the source path."
    );
}

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
    // Validate BEFORE touching the filesystem. `run_into` validates too (it needs the topo order),
    // so this is a cheap pure re-check bought for one property: a rejected spec leaves **nothing**
    // behind, not even an empty `out_dir`. That matters most for the ADR-0056 §7 laundering rule,
    // whose whole point is to refuse at the door — an operator who is told "this spec cannot run"
    // should not then find an output directory suggesting it partly did.
    validate(spec)?;
    // Atomicity (#302): the engine writes each member `.tsra` as it goes, so a failure on member N
    // used to leave members 0..N orphaned in `out_dir` with NO `collection.json` — a half-written
    // collection indistinguishable from a complete one. Fix: run the whole spec into a private
    // staging dir, then PROMOTE it into `out_dir` only on full success (atomic per-file renames on
    // the same filesystem). Any failure removes the staging dir → no orphans, no partial catalog.
    std::fs::create_dir_all(out_dir).map_err(|e| {
        Error::Invalid(format!(
            "ingest-engine: create out_dir {}: {e}",
            out_dir.display()
        ))
    })?;
    let staging = out_dir.join(".staging-ingest");
    // Clear any leftover staging from a previously-crashed run before starting.
    let _ = std::fs::remove_dir_all(&staging);
    match run_into(spec, spec_path, &staging, cfg, stream_threshold) {
        Ok(collection) => {
            promote_staging(&staging, out_dir)?;
            Ok(collection)
        }
        Err(e) => {
            let _ = std::fs::remove_dir_all(&staging);
            Err(e)
        }
    }
}

/// Move every entry from the completed staging dir into `out_dir` (atomic per-file renames — same
/// filesystem, since staging is a subdir of `out_dir`), then remove the now-empty staging dir. This
/// is the commit point of the transactional [`run`]: before it, `out_dir` has no partial output.
fn promote_staging(staging: &Path, out_dir: &Path) -> Result<()> {
    let entries = std::fs::read_dir(staging).map_err(|e| {
        Error::Invalid(format!(
            "ingest-engine: read staging {}: {e}",
            staging.display()
        ))
    })?;
    for entry in entries {
        let entry =
            entry.map_err(|e| Error::Invalid(format!("ingest-engine: staging entry: {e}")))?;
        let dest = out_dir.join(entry.file_name());
        std::fs::rename(entry.path(), &dest).map_err(|e| {
            Error::Invalid(format!(
                "ingest-engine: promote {} -> {}: {e}",
                entry.path().display(),
                dest.display()
            ))
        })?;
    }
    std::fs::remove_dir_all(staging).map_err(|e| {
        Error::Invalid(format!(
            "ingest-engine: remove staging {}: {e}",
            staging.display()
        ))
    })?;
    Ok(())
}

/// Run a parsed spec into `out_dir` writing members + `collection.json` directly (no staging). The
/// transactional [`run`] wraps this against a staging dir; call this only where partial output on
/// failure is acceptable (it isn't, for a user-facing ingest — use [`run`]).
fn run_into(
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
    // (name → sealed Manifest) — retained so a derived product can inherit schema-flagged identity
    // from its parents at seal (ADR-0058 §5). Topo order (parents-first) guarantees a parent is
    // present before any child that derives from it.
    let mut manifests: BTreeMap<String, Manifest> = BTreeMap::new();

    // The collection's normalised timestamp IS the per-product timestamp — re-running the same
    // spec must produce byte-identical member ids, so this MUST go through the same
    // `normalize_timestamp` the manifest itself does (the manifest's `Manifest::new` does it too,
    // but doing it here keeps the engine's own determinism transparent in one place).
    let timestamp = tessera_core::identity::normalize_timestamp(&spec.collection.timestamp);
    for &idx in &order {
        let p = &spec.products[idx];
        let extra = build_extra_sources(p, &built, &spec_ref, &h)?;
        // Resolve this product's `derived_from` parents to their sealed manifests (topo order ⇒
        // present) so the seal can inherit their schema-flagged identity (ADR-0058 §5).
        let parents: Vec<&Manifest> = p
            .derived_from
            .iter()
            .filter_map(|n| manifests.get(n))
            .collect();
        let (manifest, _payloads_written_in_dispatch) = dispatch(
            p,
            &extra,
            out_dir,
            cfg,
            stream_threshold,
            &timestamp,
            &parents,
        )?;
        // Honor the fd5 schema contract AT INGEST: a product that claims a known schema must satisfy
        // it now, not only on a later `tessera schema`. Open-world → unknown product names pass.
        let registry = tessera_core::SchemaRegistry::builtin();
        registry.validate(&manifest).map_err(|e| {
            Error::Invalid(format!(
                "ingest-engine: member '{}' fails its declared schema '{}': {e}",
                p.name, manifest.product
            ))
        })?;
        // …and surface the WARN tier (recommended-but-absent fields) without blocking — the
        // schema-driven FAIR-completeness nudge (supply via `[product.metadata]` / `--meta`).
        for f in registry.missing_recommended(&manifest) {
            tracing::warn!(
                target: "tessera::ingest",
                member = %p.name,
                product = %manifest.product,
                field = %f.id,
                "recommended metadata '{}' absent — {} (supply via --meta {}=…)",
                f.id,
                f.description,
                f.id
            );
        }
        // ADR-0040 §1 precursor warn: any field the schema marks `Identifying` (direct PHI per the
        // DICOM PS3.15 confidentiality profile) **present in metadata in the clear** is the hook
        // the future field-encryption / redact phases replace. Never blocks — the spike only proves
        // the schema-driven tier reaches ingest; encryption/redaction lands in the next phase
        // (#240 / PR #238).
        for f in registry
            .fields_by_sensitivity(&manifest, tessera_core::schema::Sensitivity::Identifying)
        {
            if manifest.metadata.contains_key(&f.id) {
                tracing::warn!(
                    target: "tessera::ingest::phi",
                    member = %p.name,
                    product = %manifest.product,
                    field = %f.id,
                    sensitivity = "identifying",
                    "PHI in the clear: identifying field '{}' present unencrypted in metadata — \
                     {} (ADR-0040: field-encryption / redact lands in a follow-up phase)",
                    f.id,
                    f.description,
                );
            }
        }
        let id = manifest.id.clone();
        let mh = manifest.manifest_hash.clone().ok_or_else(|| {
            Error::Invalid(format!(
                "ingest-engine: member '{}' has no manifest_hash after seal",
                p.name
            ))
        })?;
        built.insert(p.name.clone(), (id, mh));
        manifests.insert(p.name.clone(), manifest);
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
    parents: &[&Manifest],
) -> Result<(Manifest, ())> {
    let name = p.name.as_str();
    // collection-level timestamp is the per-product timestamp too: the engine takes its identity
    // discipline from the spec, never from `Local::now()` or filesystem mtimes.
    let timestamp = timestamp.to_string();
    // ADR-0040: a `source_label` (if declared on the spec / `--source-label` on the CLI) REPLACES
    // the input path in the `ingested_from` reference. The path itself is still used to READ the
    // bytes — but never appears in the sealed manifest. Computed once here, threaded into every
    // backend (the seam each backend now exposes as a `source_label: Option<&str>` parameter).
    let label = p.source_label.as_deref();
    match &p.options {
        FormatOptions::Blob { input, media_type } => {
            // Bounded-memory: stream the file's blake3, then pack_streaming with the source file as the
            // `data` block's fragment — a multi-GB blob never enters RAM (#231).
            let m = crate::blob::to_blob_product_streaming(
                input,
                name,
                &timestamp,
                media_type.as_deref(),
                label,
                extra_sources,
            )?;
            let m = seal_streaming_to_tsra(
                m,
                &[("data".to_string(), input.as_path())],
                out_dir,
                p,
                parents,
            )?;
            Ok((m, ()))
        }
        FormatOptions::BlobSeries { inputs, media_type } => {
            // Block-per-file preservation → one `.tsra`, no tar (#301/#329). In-memory seal (each file
            // read whole in turn); DICOM-series-scale slices fit comfortably.
            let (m, payloads) = crate::blob::to_blob_multi_product(
                inputs,
                name,
                &timestamp,
                media_type.as_deref(),
                label,
                extra_sources,
            )?;
            let m = seal_to_tsra(m, &payloads, out_dir, p, parents, timestamp.as_str())?;
            Ok((m, ()))
        }
        FormatOptions::Dicom {
            input,
            deidentify,
            recipients,
        } => {
            let recips = parse_recipients(recipients)?;
            // Three modes (ADR-0047): crypto-shred (recipients present) → de-id + recoverable
            // encrypted identity; destructive de-id (`deidentify`) → PHI dropped; keep-PHI default →
            // curated fields retained but the raw header is NOT embedded (#269 leak fix).
            let (img, identity) = if !recips.is_empty() {
                let (img, id) = crate::dicom::read_image_crypto_shred(input)?;
                (img, Some(id))
            } else if *deidentify {
                (crate::dicom::read_image_deidentified(input)?, None)
            } else {
                warn_dicom_not_deidentified();
                (without_raw_header(crate::dicom::read_image(input)?), None)
            };
            let source = label
                .map(str::to_string)
                .unwrap_or_else(|| input.display().to_string());
            let digest = crate::provenance::source_digest(&[input.as_path()])?;
            let (m, payloads) = crate::dicom::to_recon_product(
                &img,
                name,
                &timestamp,
                &source,
                Some(&digest),
                extra_sources,
            )?;
            let m = seal_to_tsra(m, &payloads, out_dir, p, parents, timestamp.as_str())?;
            attach_identity_envelope(out_dir, &m, identity.as_ref(), &recips)?;
            Ok((m, ()))
        }
        FormatOptions::DicomSeries {
            inputs,
            deidentify,
            recipients,
            rescale_mode,
        } => {
            // Three modes (ADR-0047 + #300): crypto-shred (recipients present) → de-id + recoverable
            // encrypted identity; destructive de-id (`deidentify`) → PHI dropped; keep-PHI default →
            // curated fields retained but the raw header is NOT embedded (#269 leak fix). All three
            // respect `rescale_mode` — `bit-exact` (default) rejects differing per-slice
            // `RescaleSlope`s; `global-int16` collapses a per-slice-rescaled series (GE quantitative
            // PET) to one global int16 scale.
            let recips = parse_recipients(recipients)?;
            let (img, identity) = if !recips.is_empty() {
                let (img, id) = crate::dicom::read_series_crypto_shred(inputs, *rescale_mode)?;
                (img, Some(id))
            } else if *deidentify {
                (
                    crate::dicom::read_series_rescaled(inputs, true, *rescale_mode)?,
                    None,
                )
            } else {
                warn_dicom_not_deidentified();
                (
                    without_raw_header(crate::dicom::read_series_rescaled(
                        inputs,
                        false,
                        *rescale_mode,
                    )?),
                    None,
                )
            };
            // With a `source_label`, recording N paths joined with commas is exactly what the label
            // exists to suppress (an 890-slice series would embed each path verbatim). When no label
            // is given, the joined paths are kept as the v0 behavior.
            let source = label.map(str::to_string).unwrap_or_else(|| {
                inputs
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            });
            // Merkle root over every slice's raw bytes — pins the product to the exact series even
            // when a `--source-label` replaces the (PHI-bearing) per-slice paths in the reference.
            let paths: Vec<&std::path::Path> = inputs.iter().map(|p| p.as_path()).collect();
            let digest = crate::provenance::source_digest(&paths)?;
            let (m, payloads) = crate::dicom::to_recon_product(
                &img,
                name,
                &timestamp,
                &source,
                Some(&digest),
                extra_sources,
            )?;
            let m = seal_to_tsra(m, &payloads, out_dir, p, parents, timestamp.as_str())?;
            attach_identity_envelope(out_dir, &m, identity.as_ref(), &recips)?;
            Ok((m, ()))
        }
        FormatOptions::Nifti { input } => {
            let img = crate::nifti::read_nifti(input)?;
            let source = label
                .map(str::to_string)
                .unwrap_or_else(|| input.display().to_string());
            let digest = crate::provenance::source_digest(&[input.as_path()])?;
            let (m, payloads) = crate::nifti::to_recon_product(
                &img,
                name,
                &timestamp,
                &source,
                Some(&digest),
                extra_sources,
            )?;
            let m = seal_to_tsra(m, &payloads, out_dir, p, parents, timestamp.as_str())?;
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
                label,
                extra_sources,
            )?;
            let m = seal_to_tsra(m, &payloads, out_dir, p, parents, timestamp.as_str())?;
            Ok((m, ()))
        }
        // ── The generic-ingest lanes (ADR-0056 §11 P1) ──
        //
        // ADR-0057 §6's shape: the enum variant always **parses** (so a TOML spec's `spec_hash` is
        // portable across builds and an archival spec never becomes unreadable), and only the
        // *handler* is feature-gated. An unavailable backend is a clean, typed runtime error — never
        // a parse error that would point at the file instead of at the build. The gating itself lives
        // in `decode_generic_table`, so all three lanes share one shape.
        FormatOptions::Parquet { input, .. }
        | FormatOptions::Arrow { input, .. }
        | FormatOptions::Csv { input, .. } => {
            #[cfg(any(feature = "parquet", feature = "arrow", feature = "csv"))]
            {
                // #458: bounded memory when the input earns it. Small inputs stay on the batch path,
                // which is what keeps the conformance corpus from regenerating — and the two paths
                // are pinned to the same `content_hash` AND `manifest_hash` by test either way, so
                // this branch is a memory/time trade and never a format decision.
                let m = if decide_stream_table(&p.options, stream_threshold)? {
                    stream_generic_table(
                        p,
                        input,
                        label,
                        extra_sources,
                        &timestamp,
                        out_dir,
                        parents,
                        cfg,
                    )?
                } else {
                    let decoded = decode_generic_table(&p.options)?.ok_or_else(|| {
                        Error::Invalid(
                            "ingest-engine: internal — a generic variant did not decode".into(),
                        )
                    })?;
                    seal_generic_table(
                        &decoded,
                        p,
                        input,
                        generic_column_meta(&p.options),
                        label,
                        extra_sources,
                        &timestamp,
                        out_dir,
                        parents,
                    )?
                };
                Ok((m, ()))
            }
            // A build with every generic lane off still parses the spec and still computes its
            // `spec_hash`; only the run fails, and it names which backend is missing.
            #[cfg(not(any(feature = "parquet", feature = "arrow", feature = "csv")))]
            {
                let _ = input;
                Err(Error::BackendNotCompiled(crate::backends::backend_name(
                    &p.options,
                )))
            }
        }
        // ── The generic ARRAY lane (ADR-0056 §11) ──
        //
        // The one generic backend whose primitive depends on the FILE rather than the format: a NumPy
        // structured dtype is rows of typed fields, so it routes to the table primitive (§1). The spec
        // declares `array`, and this arm honours whichever the header turned out to hold rather than
        // making an operator predict their own dtype.
        FormatOptions::Npy { input } | FormatOptions::NpzMember { input, .. } => {
            #[cfg(feature = "npy")]
            {
                // `.npy` is a forward read of the whole file, so it works on a pipe and shares #542's
                // defect; the bytes are in hand, so the digest is free. `.npz` goes through a zip
                // central directory, which needs to seek — a pipe never reaches it, so the re-read
                // fallback is always over a real file there.
                let mut npy_digest: Option<String> = None;
                let content = match &p.options {
                    // The enum stays closed and total (ADR-0057 §6); only the handler is gated. A
                    // build with `npy` but not `npz` can still read a bare `.npy`, and says so about
                    // the archive rather than failing as though the format were unknown (§7).
                    #[cfg(feature = "npz")]
                    FormatOptions::NpzMember { input, member } => {
                        crate::npy::read_npz_member(input, member)?
                    }
                    #[cfg(not(feature = "npz"))]
                    FormatOptions::NpzMember { .. } => {
                        return Err(Error::BackendNotCompiled(crate::backends::backend_name(
                            &p.options,
                        )))
                    }
                    _ => {
                        let (content, digest) = crate::npy::read_npy_digested(input)?;
                        npy_digest = Some(digest);
                        content
                    }
                };
                let source_format = match &p.options {
                    FormatOptions::NpzMember { .. } => "npz",
                    _ => "npy",
                };
                let opts = crate::canonical::GenericIngest {
                    name: &p.name,
                    timestamp: &timestamp,
                    description: p
                        .description
                        .as_deref()
                        .unwrap_or("generically-ingested array"),
                    source_format,
                    source_path: input,
                    source_label: label,
                    extra_sources,
                    // Branches exactly where `source_format` does: `.npz` goes through the zip
                    // archive reader, `.npy` through nothing but our own parser, and the seal must
                    // not claim otherwise (#477).
                    decoder: match &p.options {
                        FormatOptions::NpzMember { .. } => crate::decoder::Decoder::NPZ,
                        _ => crate::decoder::Decoder::NPY,
                    },
                    generation: p.generation.clone(),
                    column_meta: generic_column_meta(&p.options),
                    source_digest: npy_digest.as_deref(),
                };
                // One dispatch, shared with `seal_generic_product`, so the declared-vs-actual check
                // cannot be present in one copy and missing in the other.
                let (m, payloads) =
                    seal_npy_content(content, Some(p.schema.as_str()), input, &p.name, &opts)?;
                let m = seal_to_tsra(m, &payloads, out_dir, p, parents, timestamp.as_str())?;
                Ok((m, ()))
            }
            #[cfg(not(feature = "npy"))]
            {
                let _ = input;
                Err(Error::BackendNotCompiled(crate::backends::backend_name(
                    &p.options,
                )))
            }
        }
        FormatOptions::HdfCompound {
            input,
            dataset,
            row_index,
            block_prefix,
            streaming,
            slab_rows,
            quantize,
        } => {
            // The opt-in GEDDF quantize/annotate transform (#310) operates on the whole in-memory
            // table, so it forces the batch path (the streaming slab writer has no transform seam yet).
            let should_stream =
                !*quantize && resolve_streaming(*streaming, input, dataset, stream_threshold)?;
            if should_stream {
                // Stream straight to disk — the bounded-memory + multi-block path. The output path
                // is computed BEFORE the seal because the streaming writer writes the .tsra
                // directly; we then re-open the sealed manifest to record its id.
                let stage = out_dir.join(format!("__stage_{}", sanitize_reference(name)));
                let tmp_out = out_dir.join(format!("__pending_{}.tsra", sanitize_reference(name)));
                // ADR-0058 §5: inherit schema-flagged identity from parents up-front (the streaming
                // writer applies metadata pre-seal on the WriteSession, so there is no post-build
                // re-seal hook — and re-writing a multi-GB streamed .tsra just for metadata would
                // defeat the bounded-memory path). The backend layers the three tiers in ascending
                // priority (inherited < product-own default < spec), so `inherited` and `p.metadata`
                // are passed SEPARATELY — never pre-merged — to keep an inherited value from ever
                // clobbering a product-own default. `study` (first-class) inherits from the parent too.
                let inherited = inherited_metadata(parents, "listmode");
                let inherited_study = parents.iter().find_map(|m| m.study.as_deref());
                // Build extra_sources with the canonical `ingested_from` flowing through the
                // streaming session (it adds its own `ingested_from`); pass `extra_sources` as-is.
                // #416: the spec's `[product.generation]`/`[product.producer]` (ADR-0058 §1/§2) ride
                // the same pre-seal bag — the batch path's `apply_spec_metadata` has no counterpart
                // here, so anything not declared before the first block commits is dropped silently.
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
                    label,
                    extra_sources,
                    &crate::ge_hdf5::StreamProvenance {
                        inherited: Some(&inherited),
                        inherited_study,
                        metadata: Some(&p.metadata),
                        generation: p.generation.as_ref(),
                        producer: p.producer.as_ref(),
                    },
                )?;
                // Rename the pending .tsra to its id-named final path. Same filesystem → rename is
                // atomic, so a crash here leaves either the old or the new file in place.
                let final_path = out_dir.join(member_filename(&m.id, MemberKind::Product));
                std::fs::rename(&tmp_out, &final_path).map_err(|e| {
                    Error::Invalid(format!(
                        "ingest-engine: rename {} -> {}: {e}",
                        tmp_out.display(),
                        final_path.display()
                    ))
                })?;
                // ADR-0042: aux/provenance.json stamp — matches the batch path's seal_to_tsra. The
                // streaming path only becomes stampable after the atomic rename to the id-named
                // final path (before that, the file is `__pending_*.tsra` in flux).
                stamp_ingest_provenance(&final_path, &ProvenanceOptions::default())?;
                // Tidy up the per-product stage dir — best-effort (failure here would not change
                // the sealed product's correctness, so it's not a hard error).
                let _ = std::fs::remove_dir_all(&stage);
                Ok((m, ()))
            } else {
                // Batch path: read the whole compound, build the in-memory product, pack.
                let mut cols = crate::ge_hdf5::read_compound(input, dataset)?;
                let source = label
                    .map(str::to_string)
                    .unwrap_or_else(|| input.display().to_string());
                let (m, payloads) = if *quantize {
                    // Opt-in transform (#310): annotate + requantize float columns to int16 before
                    // sealing, so both payload and manifest reflect the physical-resolution encoding.
                    let schema = crate::ge_hdf5::apply_geddf_dictionary(dataset, &mut cols, true);
                    crate::ge_hdf5::to_listmode_product_with_schema(
                        &cols,
                        name,
                        &timestamp,
                        &source,
                        block_prefix,
                        row_index,
                        extra_sources,
                        schema,
                    )?
                } else {
                    // #343: annotate columns (unit/description/short_name) from the GEDDF dictionary
                    // even without quantization, so batch tables (time-markers, coin-counters, …) are
                    // self-describing. `quantize = false` leaves dtypes + values untouched.
                    let schema = crate::ge_hdf5::apply_geddf_dictionary(dataset, &mut cols, false);
                    crate::ge_hdf5::to_listmode_product_with_schema(
                        &cols,
                        name,
                        &timestamp,
                        &source,
                        block_prefix,
                        row_index,
                        extra_sources,
                        schema,
                    )?
                };
                let m = seal_to_tsra(m, &payloads, out_dir, p, parents, timestamp.as_str())?;
                Ok((m, ()))
            }
        }
    }
}

/// Decode a generic table source into a canonicalised table, naming the source format and the decoder
/// that read it.
///
/// The single place the three generic lanes are chosen between. Both the dispatch arms below and the
/// conformance corpus go through it, which is what keeps the corpus honest: a fixture must exercise the
/// same decode a real ingest does, not a parallel re-implementation that could drift from it.
///
/// Returns `None` for a non-generic (vendor) backend — the caller is expected to have dispatched it
/// elsewhere, and the `match` is total so a new variant cannot be silently forgotten.
#[cfg(any(feature = "parquet", feature = "arrow", feature = "csv"))]
pub fn decode_generic_table(opts: &FormatOptions) -> Result<Option<DecodedTable>> {
    // Only the three TABLE lanes decode here. `generic_source_and_decoder` also names the two array
    // lanes, which route to the array primitive instead, so they must come back as `None`.
    if !is_generic_table_lane(opts) {
        return Ok(None);
    }
    let Some((source_format, decoder)) = generic_source_and_decoder(opts) else {
        return Ok(None);
    };
    // `Some` only where the lane reads its input in ONE forward pass and can therefore hash it as it
    // goes. Parquet and Arrow IPC do random-access reads through a footer index, so an in-flight hash
    // would cover their bytes out of order and partially — and neither can read a non-seekable input
    // at all, so the re-read `provenance::ingested_from` falls back to is always over a real file for
    // them. CSV is a forward scan, reads a pipe happily, and so MUST supply one (#542).
    let mut source_digest: Option<String> = None;
    let table = match opts {
        FormatOptions::Parquet { input, exclude, .. } => {
            #[cfg(feature = "parquet")]
            {
                crate::parquet_table::read_table(input, exclude)?
            }
            // Kept as a typed error rather than an `unreachable!()` so a future reorder of these
            // arms cannot turn a missing backend into a panic.
            #[cfg(not(feature = "parquet"))]
            {
                let _ = (input, exclude);
                return Err(Error::BackendNotCompiled("parquet"));
            }
        }
        FormatOptions::Arrow { input, exclude, .. } => {
            #[cfg(feature = "arrow")]
            {
                crate::arrow_table::read_arrow_table(input, exclude)?
            }
            #[cfg(not(feature = "arrow"))]
            {
                let _ = (input, exclude);
                return Err(Error::BackendNotCompiled("arrow"));
            }
        }
        FormatOptions::Csv { input, .. } => {
            #[cfg(feature = "csv")]
            {
                let (table, digest) =
                    crate::csv_table::read_table_digested(input, &csv_options(opts)?)?;
                source_digest = Some(digest);
                table
            }
            #[cfg(not(feature = "csv"))]
            {
                let _ = input;
                return Err(Error::BackendNotCompiled("csv"));
            }
        }
        // `is_generic_table_lane` passed above, so the variant is one of the three arms here.
        _ => return Ok(None),
    };
    Ok(Some(DecodedTable {
        table,
        source_format,
        decoder,
        source_digest,
    }))
}

/// What a generic table lane produced: the table, the two facts only the lane knows, and the digest
/// only it could capture.
///
/// A struct rather than a tuple because the fourth member is an `Option` whose meaning is not
/// obvious from its position, and silently swapping it with `source_format` would still compile.
#[cfg(any(feature = "parquet", feature = "arrow", feature = "csv"))]
pub struct DecodedTable {
    pub table: crate::canonical::CanonicalTable,
    pub source_format: &'static str,
    pub decoder: crate::decoder::Decoder,
    /// `blake3` of the source bytes, captured DURING the decode — `None` for a lane that cannot hash
    /// in one forward pass. See the comment at the top of [`decode_generic_table`].
    pub source_digest: Option<String>,
}

/// Resolve a `Csv` variant's spec fields into [`crate::csv_table::CsvOptions`].
///
/// One place, because the whole-file read and the chunked read must apply the same delimiter, header
/// rule, null tokens and exclusions — ADR-0056 §8 makes all four part of what the declared schema
/// MEANS, so a divergence here would decode the same file into two different tables.
#[cfg(feature = "csv")]
fn csv_options(opts: &FormatOptions) -> Result<crate::csv_table::CsvOptions> {
    let FormatOptions::Csv {
        columns,
        delimiter,
        header,
        null_tokens,
        exclude,
        ..
    } = opts
    else {
        return Err(Error::Invalid(
            "ingest-engine: internal — csv_options on a non-csv variant".into(),
        ));
    };
    let mut o = crate::csv_table::CsvOptions::from_decls(columns)?;
    if let Some(d) = delimiter {
        o.delimiter = one_byte_delimiter(d)?;
    }
    o.header = *header;
    o.null_tokens = null_tokens.clone();
    o.exclude = exclude.clone();
    Ok(o)
}

/// Decode **and seal** one generic-ingest product, choosing the primitive the source implies.
///
/// The single path both the engine's dispatch arms and the conformance corpus take, so a fixture
/// exercises the real ingest rather than a parallel re-implementation that could drift from it. It stops
/// short of writing the `.tsra` — the caller does that, because the engine writes into a staging
/// directory and the corpus only needs the manifest.
///
/// Returns `None` for a non-generic (vendor) backend, so the `match` stays total and a new variant
/// cannot be silently forgotten.
/// Seal decoded `.npy` content, checking the declared schema against the primitive the file turned out
/// to hold.
///
/// **One function because there were two.** `run` and [`seal_generic_product`] both dispatched on
/// `NpyContent`, and only one of them checked the declaration — so the corpus path and the spec path
/// could disagree about what is acceptable, which is the shape of bug that survives review by looking
/// correct in whichever copy the reviewer opened. There is one dispatch now and the check rides inside
/// it.
///
/// `declared` is the spec's `schema`, or `None` where there is no declaration to falsify (the
/// conformance corpus seals content directly, with no spec).
#[cfg(feature = "npy")]
fn seal_npy_content(
    content: crate::npy::NpyContent,
    declared: Option<&str>,
    input: &std::path::Path,
    name: &str,
    opts: &crate::canonical::GenericIngest<'_>,
) -> Result<(Manifest, Vec<tessera_io::BlockPayload>)> {
    match content {
        crate::npy::NpyContent::Array(a) => {
            // `spec::check_no_schema_laundering` lets a `.npy` product declare `table`, because a
            // NumPy **structured** dtype really is rows of typed fields and an operator cannot know
            // which they have without opening the file. That relaxation is only honest until the file
            // IS open: a spec claiming `table` over a plain array would otherwise seal an **array**
            // whose declared contract said otherwise — the declared-vs-actual disagreement the
            // laundering rule exists to stop, merely deferred to where it became knowable.
            //
            // Only this direction is an error. `array` is the agnostic declaration and stays accepted
            // whichever primitive the header holds (that is the point of the relaxation); `table` is a
            // positive claim, and a plain array falsifies it.
            if declared == Some("table") {
                return Err(Error::Invalid(format!(
                    "ingest-spec: product '{name}' declares schema 'table', but {} holds a plain \
                     (non-structured) NumPy dtype, so the ingest produces an 'array'.\n  \
                     A structured/record dtype would be a table; this file is not one.\n  \
                     use:  schema = \"array\"  (or omit it — 'array' is the default for this backend, \
                     and it also accepts a structured dtype)",
                    input.display()
                )));
            }
            crate::canonical::to_array_product(&a.spec, &a.data, &a.transforms, opts)
        }
        crate::npy::NpyContent::Table(table) => {
            crate::column_meta::warn_unclassified_identifying(
                &table.column_names(),
                opts.column_meta,
                name,
            );
            crate::canonical::to_table_product(&table, opts)
        }
    }
}

#[cfg(any(
    feature = "parquet",
    feature = "arrow",
    feature = "csv",
    feature = "npy"
))]
pub fn seal_generic_product(
    options: &FormatOptions,
    opts: &crate::canonical::GenericIngest<'_>,
) -> Result<Option<(Manifest, Vec<tessera_io::BlockPayload>)>> {
    // The ARRAY lane, whose primitive depends on the file: a NumPy structured dtype is a table.
    #[cfg(feature = "npy")]
    if let FormatOptions::Npy { input } | FormatOptions::NpzMember { input, .. } = options {
        let content = match options {
            #[cfg(feature = "npz")]
            FormatOptions::NpzMember { member, .. } => crate::npy::read_npz_member(input, member)?,
            #[cfg(not(feature = "npz"))]
            FormatOptions::NpzMember { .. } => {
                return Err(Error::BackendNotCompiled(crate::backends::backend_name(
                    options,
                )))
            }
            _ => crate::npy::read_npy(input)?,
        };
        // No spec here, so no declaration to falsify — the corpus seals content directly.
        return Ok(Some(seal_npy_content(
            content, None, input, opts.name, opts,
        )?));
    }
    // The TABLE lane.
    #[cfg(any(feature = "parquet", feature = "arrow", feature = "csv"))]
    if let Some(decoded) = decode_generic_table(options)? {
        crate::column_meta::warn_unclassified_identifying(
            &decoded.table.column_names(),
            opts.column_meta,
            opts.name,
        );
        // The caller cannot know the digest — only the decode that read the bytes can — so it is
        // injected here rather than threaded through every caller. The VALUE is identical to what a
        // re-read would produce (`single_source_digest` applies the same MMR wrapper
        // `provenance::source_digest` does), so no golden moves; what changes is that a source which
        // cannot be read twice now gets the right answer instead of the digest of zero bytes.
        let opts = crate::canonical::GenericIngest {
            source_digest: decoded.source_digest.as_deref(),
            ..opts.clone()
        };
        return Ok(Some(crate::canonical::to_table_product(
            &decoded.table,
            &opts,
        )?));
    }
    Ok(None)
}

/// The `source_format` and decoder a generic backend records — the two facts `GenericIngest` needs that
/// only the backend knows.
#[cfg(any(
    feature = "parquet",
    feature = "arrow",
    feature = "csv",
    feature = "npy"
))]
pub fn generic_source_and_decoder(
    options: &FormatOptions,
) -> Option<(&'static str, crate::decoder::Decoder)> {
    match options {
        FormatOptions::Parquet { .. } => Some(("parquet", crate::decoder::Decoder::PARQUET)),
        FormatOptions::Arrow { .. } => Some(("arrow", crate::decoder::Decoder::ARROW_IPC)),
        FormatOptions::Csv { .. } => Some(("csv", crate::decoder::Decoder::CSV)),
        FormatOptions::Npy { .. } => Some(("npy", crate::decoder::Decoder::NPY)),
        FormatOptions::NpzMember { .. } => Some(("npz", crate::decoder::Decoder::NPZ)),
        _ => None,
    }
}

/// Is this one of the three generic **table** lanes?
///
/// Separate from [`generic_source_and_decoder`], which also names the `.npy`/`.npz` lanes: those are
/// generic ingest too, but they route to the array primitive, and a reader of either function should
/// not have to infer that distinction from a `match` arm's absence.
#[cfg(any(feature = "parquet", feature = "arrow", feature = "csv"))]
fn is_generic_table_lane(opts: &FormatOptions) -> bool {
    matches!(
        opts,
        FormatOptions::Parquet { .. } | FormatOptions::Arrow { .. } | FormatOptions::Csv { .. }
    )
}

/// Seal a canonicalised generic table as a `table` product and write its `.tsra`.
///
/// One helper for all three generic lanes, so the parts that must not drift between them — the
/// decoder record, the `source_format` field, the operator's recipe, the `--column-meta` application,
/// the PHI advisory — are written once. The lanes differ only in how they produced the `CanonicalTable`.
#[cfg(any(feature = "parquet", feature = "arrow", feature = "csv"))]
#[allow(clippy::too_many_arguments)] // the dispatch context, same as every other backend seam
fn seal_generic_table(
    decoded: &DecodedTable,
    p: &crate::spec::ProductSpec,
    input: &Path,
    column_meta: &crate::column_meta::ColumnMeta,
    label: Option<&str>,
    extra_sources: &[Source],
    timestamp: &str,
    out_dir: &Path,
    parents: &[&Manifest],
) -> Result<Manifest> {
    let DecodedTable {
        table,
        source_format,
        decoder,
        source_digest,
    } = decoded;
    crate::column_meta::warn_unclassified_identifying(&table.column_names(), column_meta, &p.name);
    let (m, payloads) = crate::canonical::to_table_product(
        table,
        &crate::canonical::GenericIngest {
            name: &p.name,
            timestamp,
            description: p
                .description
                .as_deref()
                .unwrap_or("generically-ingested table"),
            source_format,
            source_path: input,
            source_label: label,
            extra_sources,
            decoder: *decoder,
            generation: p.generation.clone(),
            column_meta,
            source_digest: source_digest.as_deref(),
        },
    )?;
    seal_to_tsra(m, &payloads, out_dir, p, parents, timestamp)
}

/// Should this generic table lane stream, rather than read the whole file into memory?
///
/// A **routing** decision and nothing more: both paths seal identical bytes — pinned on BOTH hashes
/// by the batch-equals-stream tests — so a wrong answer here costs memory, or a second read of the
/// input, and can never move a hash. That is precisely what licenses the `Auto` estimate below to
/// come from a Parquet footer, which #502 forbids *identity* from trusting. The distinction is the
/// point, not an inconsistency: a producer's unverified `num_rows`/column-chunk sizes are fine for
/// choosing a path and not fine for deciding what a column IS.
#[cfg(any(feature = "parquet", feature = "arrow", feature = "csv"))]
fn decide_stream_table(opts: &FormatOptions, stream_threshold: u64) -> Result<bool> {
    let (mode, input) = match opts {
        FormatOptions::Parquet {
            streaming, input, ..
        }
        | FormatOptions::Arrow {
            streaming, input, ..
        }
        | FormatOptions::Csv {
            streaming, input, ..
        } => (*streaming, input.as_path()),
        _ => return Ok(false),
    };
    // Streaming reads the input TWICE (the shape pass decides nullability over the whole file — see
    // `stream_table`'s module docs), so a pipe, fifo or character device cannot take this path: the
    // second pass would read an exhausted stream and seal a silently truncated product.
    //
    // Falling back rather than refusing, because `tessera ingest table <(zcat big.csv.gz) …` passes
    // `/dev/fd/63` and works today — refusing would regress behaviour that exists. An explicit
    // `streaming = "stream"` gets a warning, since the operator asked for bounded memory and is not
    // getting it.
    if !crate::stream_table::is_seekable(input) {
        if matches!(mode, StreamingMode::Stream) {
            tracing::warn!(
                target: "tessera::ingest::stream",
                input = %input.display(),
                "streaming = \"stream\" was requested but {} is not a regular file, so it cannot be \
                 read twice; falling back to the whole-file path. Memory will scale with the input. \
                 To stream, materialise it first (`zcat big.csv.gz > big.csv`).",
                input.display()
            );
        }
        return Ok(false);
    }
    Ok(match mode {
        StreamingMode::Batch => false,
        StreamingMode::Stream => true,
        StreamingMode::Auto => generic_table_size_estimate(opts)? > stream_threshold,
    })
}

/// A cheap upper-ish estimate of what decoding this input would have to hold, for routing only.
///
/// Each lane answers with the best number it can get without decoding: Parquet reads its footer's
/// uncompressed column-chunk total (the on-disk size understates it by the compression ratio),
/// Arrow IPC uses the file length (IPC is uncompressed by default, so that already *is* the decoded
/// size), and CSV uses the file length too — text is a rough proxy either way, since `"511.0"` is
/// five bytes on disk and eight in an `f64` while `"1"` is one and four.
///
/// Rough is sufficient and provably so: the number only picks between two paths that seal the same
/// bytes.
#[cfg(any(feature = "parquet", feature = "arrow", feature = "csv"))]
fn generic_table_size_estimate(opts: &FormatOptions) -> Result<u64> {
    let file_len = |p: &Path| -> Result<u64> {
        std::fs::metadata(p)
            .map(|m| m.len())
            .map_err(|e| Error::Invalid(format!("ingest-engine: stat {}: {e}", p.display())))
    };
    match opts {
        FormatOptions::Parquet { input, .. } => {
            #[cfg(feature = "parquet")]
            {
                crate::parquet_table::parquet_size_estimate(input)
            }
            #[cfg(not(feature = "parquet"))]
            {
                file_len(input)
            }
        }
        FormatOptions::Arrow { input, .. } => {
            #[cfg(feature = "arrow")]
            {
                crate::arrow_table::arrow_ipc_size_estimate(input)
            }
            #[cfg(not(feature = "arrow"))]
            {
                file_len(input)
            }
        }
        FormatOptions::Csv { input, .. } => file_len(input),
        _ => Ok(0),
    }
}

/// Ingest one generic table product in bounded memory (#458), sealing the `.tsra` directly.
///
/// The streaming twin of [`seal_generic_table`], and the reason it is a separate function rather
/// than a flag: the batch path builds a product and then re-seals it to apply the spec's metadata
/// tiers, while this path writes the archive **once**, straight to disk, with no post-seal hook. So
/// every tier has to be declared before the first block commits, which is what
/// [`crate::canonical::declare_table_with_tiers`] is for. A tier plumbed any later is dropped
/// silently and moves `manifest_hash` while leaving `content_hash` identical.
#[cfg(any(feature = "parquet", feature = "arrow", feature = "csv"))]
#[allow(clippy::too_many_arguments)] // each argument is a distinct, load-bearing piece of context
fn stream_generic_table(
    p: &crate::spec::ProductSpec,
    input: &Path,
    label: Option<&str>,
    extra_sources: &[Source],
    timestamp: &str,
    out_dir: &Path,
    parents: &[&Manifest],
    cfg: &tessera_io::WriteConfig,
) -> Result<Manifest> {
    let (source_format, decoder) = generic_source_and_decoder(&p.options).ok_or_else(|| {
        Error::Invalid("ingest-engine: internal — a generic variant did not name a lane".into())
    })?;
    let column_meta = generic_column_meta(&p.options);
    let ingest = crate::canonical::GenericIngest {
        name: &p.name,
        timestamp,
        description: p
            .description
            .as_deref()
            .unwrap_or("generically-ingested table"),
        source_format,
        source_path: input,
        source_label: label,
        extra_sources,
        decoder,
        generation: p.generation.clone(),
        column_meta,
        // No in-flight digest: this path already reads the input twice and routes any input it
        // cannot re-read to the batch path, so `ingested_from`'s own read is always over a real
        // file here. Hashing in pass 2 would also be wrong for Parquet, whose reader seeks.
        source_digest: None,
    };
    // ADR-0058 §5: the parents' schema-flagged identity, resolved here and passed as its own tier so
    // the driver can lay it down UNDER the lane's own fields. Never pre-merged with `p.metadata` —
    // the whole point of three tiers is that an inherited value loses to a product-own default while
    // an explicit spec value beats both, which one flattened map cannot express.
    let inherited = inherited_metadata(parents, "table");
    let inherited_study = parents.iter().find_map(|m| m.study.as_deref());
    let stage = out_dir.join(format!("__stage_{}", sanitize_reference(&p.name)));
    let tmp_out = out_dir.join(format!("__pending_{}.tsra", sanitize_reference(&p.name)));
    let stream_opts = crate::stream_table::StreamOpts {
        stage: &stage,
        out: &tmp_out,
        cfg,
        batch_rows: generic_batch_rows(&p.options),
        block_rows: tessera_io::BLOCK_ROWS as u64,
        tiers: crate::canonical::MetadataTiers {
            inherited: Some(&inherited),
            inherited_study,
            metadata: Some(&p.metadata),
            producer: p.producer.as_ref(),
        },
    };
    let m = stream_generic_product(&p.options, &ingest, &stream_opts)?;
    // The id is hashed over (product, name, timestamp), so the final path was computable up front —
    // but the pending-then-rename is how the `hdf-compound` streaming path does it and it is
    // crash-safe: same filesystem, so the rename is atomic and a crash here leaves either the old
    // file or the new one, never a half-written archive under the name a reader will trust.
    let final_path = out_dir.join(member_filename(&m.id, MemberKind::Product));
    std::fs::rename(&tmp_out, &final_path).map_err(|e| {
        Error::Invalid(format!(
            "ingest-engine: rename {} -> {}: {e}",
            tmp_out.display(),
            final_path.display()
        ))
    })?;
    // ADR-0042: the `aux/provenance.json` stamp the batch path applies in `seal_to_tsra`. Only
    // stampable after the rename — before it, the file is `__pending_*.tsra` and still in flux.
    stamp_ingest_provenance(&final_path, &ProvenanceOptions::default())?;
    // Best-effort: a leftover stage dir does not make the sealed product any less correct.
    let _ = std::fs::remove_dir_all(&stage);
    Ok(m)
}

/// Ingest one generic **table** lane in bounded memory, sealing the `.tsra` at `stream_opts.out`.
///
/// The streaming twin of [`seal_generic_product`], and the seam the conformance corpus streams
/// through — so a corpus fixture exercises the real streaming ingest rather than a parallel
/// re-implementation that could drift from it.
///
/// Each arm builds a **re-openable** chunk source, because the driver calls it twice (shape pass,
/// then encode pass); the sources are boxed so the three lanes share one iterator type.
#[cfg(any(feature = "parquet", feature = "arrow", feature = "csv"))]
pub fn stream_generic_product(
    opts: &FormatOptions,
    ingest: &crate::canonical::GenericIngest<'_>,
    stream_opts: &crate::stream_table::StreamOpts<'_>,
) -> Result<Manifest> {
    type Chunks = Box<dyn Iterator<Item = Result<crate::canonical::CanonicalTable>>>;
    match opts {
        FormatOptions::Parquet { input, exclude, .. } => {
            #[cfg(feature = "parquet")]
            {
                let rows = stream_opts.batch_rows;
                crate::stream_table::stream_to_table_product(
                    || {
                        Ok(
                            Box::new(crate::parquet_table::parquet_chunks(input, rows, exclude)?)
                                as Chunks,
                        )
                    },
                    ingest,
                    stream_opts,
                )
            }
            #[cfg(not(feature = "parquet"))]
            {
                let _ = (input, exclude);
                Err(Error::BackendNotCompiled("parquet"))
            }
        }
        FormatOptions::Arrow { input, exclude, .. } => {
            #[cfg(feature = "arrow")]
            {
                // No `batch_rows` knob: the IPC container's own record batches ARE the read unit, and
                // re-cutting them would buffer exactly what streaming exists to avoid.
                crate::stream_table::stream_to_table_product(
                    || {
                        Ok(
                            Box::new(crate::arrow_table::arrow_ipc_chunks(input, exclude)?)
                                as Chunks,
                        )
                    },
                    ingest,
                    stream_opts,
                )
            }
            #[cfg(not(feature = "arrow"))]
            {
                let _ = (input, exclude);
                Err(Error::BackendNotCompiled("arrow"))
            }
        }
        FormatOptions::Csv { input, .. } => {
            #[cfg(feature = "csv")]
            {
                let o = csv_options(opts)?;
                let rows = stream_opts.batch_rows;
                crate::stream_table::stream_to_table_product(
                    || Ok(Box::new(crate::csv_table::csv_chunks(input, &o, rows)?) as Chunks),
                    ingest,
                    stream_opts,
                )
            }
            #[cfg(not(feature = "csv"))]
            {
                let _ = input;
                Err(Error::BackendNotCompiled("csv"))
            }
        }
        _ => Err(Error::Invalid(
            "ingest-engine: internal — streamed a non-generic variant".into(),
        )),
    }
}

/// The `batch_rows` of a generic variant — the read-side bounded-memory unit on the streaming path.
#[cfg(any(feature = "parquet", feature = "arrow", feature = "csv"))]
fn generic_batch_rows(opts: &FormatOptions) -> usize {
    match opts {
        FormatOptions::Parquet { batch_rows, .. }
        | FormatOptions::Arrow { batch_rows, .. }
        | FormatOptions::Csv { batch_rows, .. } => *batch_rows,
        // Unreachable from the generic dispatch; a harmless default beats a panic on a new variant.
        _ => 64 * 1024,
    }
}

/// The `column_meta` of a generic variant — the one field the collapsed dispatch arm still needs
/// per-variant, and the `match` stays total so a new generic backend must declare it.
#[cfg(any(
    feature = "parquet",
    feature = "arrow",
    feature = "csv",
    feature = "npy"
))]
fn generic_column_meta(opts: &FormatOptions) -> &crate::column_meta::ColumnMeta {
    use crate::column_meta::ColumnMeta;
    /// The empty annotation set a vendor backend maps to. `OnceLock` rather than a `const`, because
    /// `ColumnMeta` owns a `BTreeMap` and a borrow has to outlive the call.
    static NONE: std::sync::OnceLock<ColumnMeta> = std::sync::OnceLock::new();
    match opts {
        FormatOptions::Parquet { column_meta, .. }
        | FormatOptions::Arrow { column_meta, .. }
        | FormatOptions::Csv { column_meta, .. } => column_meta,
        _ => NONE.get_or_init(ColumnMeta::empty),
    }
}

/// Parse a spec's one-character `delimiter` string into a byte.
///
/// A whole `char` rather than a byte in the TOML because `"\t"` must be writable; but the `csv`
/// tokenizer takes a byte, and a multi-byte delimiter is not a thing RFC 4180 has — so a
/// `delimiter = "::"` is an error rather than a silently truncated `:`.
#[cfg(feature = "csv")]
fn one_byte_delimiter(d: &str) -> Result<u8> {
    let bytes = d.as_bytes();
    match bytes {
        [b] => Ok(*b),
        _ => Err(Error::Invalid(format!(
            "ingest-spec: delimiter must be exactly one ASCII character, got '{d}' ({} bytes)",
            bytes.len()
        ))),
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
    parents: &[&Manifest],
    _timestamp: &str,
) -> Result<Manifest> {
    let m = apply_spec_metadata(m, &p.metadata, parents, p)?;
    let path = out_dir.join(member_filename(&m.id, MemberKind::Product));
    pack(&m, payloads, &path)?;
    // ADR-0042: stamp `aux/provenance.json` (wall-clock + producer + host) as a non-sealed aux
    // member. The sealed region is byte-identical afterwards (proven by container tests), so this
    // NEVER breaks writer-determinism on the seal-covered `id`/`content_hash`/`manifest_hash`.
    // Silenced under `TESSERA_SKIP_PROVENANCE=1` for the tests that DO compare whole-archive bytes.
    stamp_ingest_provenance(&path, &ProvenanceOptions::default())?;
    Ok(m)
}

/// Parse the crypto-shred recipient list (ADR-0047) — `age` public-key strings from the spec /
/// `--recipient` — into typed recipients. An empty list means "not crypto-shred" (the common case),
/// so this returns an empty `Vec` without error; a malformed key is a hard, typed error.
fn parse_recipients(recipients: &[String]) -> Result<Vec<age::x25519::Recipient>> {
    recipients
        .iter()
        .map(|s| crate::identity::parse_recipient(s))
        .collect()
}

/// #269 raw-header leak fix: on the keep-PHI (non-de-identified, non-crypto-shred) path, blank the
/// full DICOM header so it is **never** embedded into `extra["dicom_header"]` of a shareable, sealed
/// product. Identifying material lands in a shared product only via the encrypted crypto-shred
/// envelope (ADR-0047) or a deliberate de-id — never as a raw plaintext header (user decision).
fn without_raw_header(mut img: crate::dicom::DicomImage) -> crate::dicom::DicomImage {
    img.header_json = serde_json::Value::Null;
    img
}

/// Attach the crypto-shred identity envelope (ADR-0047) to the just-sealed `.tsra`. Encrypts the
/// captured [`IdentityDocument`] to `recipients` and writes it as the non-sealed aux member
/// `aux/identity/identity.age` (outside the seal, so `id`/`content_hash`/`manifest_hash` are
/// unchanged — a crypto-shred product and a plain de-id product share all three). A no-op when there
/// is no identity to attach (the non-crypto-shred paths pass `None`).
fn attach_identity_envelope(
    out_dir: &Path,
    m: &Manifest,
    identity: Option<&crate::identity::IdentityDocument>,
    recipients: &[age::x25519::Recipient],
) -> Result<()> {
    let Some(doc) = identity else { return Ok(()) };
    let envelope = crate::identity::encrypt_identity(doc, recipients)?;
    let path = out_dir.join(member_filename(&m.id, MemberKind::Product));
    tessera_io::add_aux_members(
        &path,
        &[tessera_io::AuxMember::new(
            crate::identity::AUX_IDENTITY_NAME,
            envelope,
        )],
    )?;
    Ok(())
}

/// Bounded-memory counterpart of [`seal_to_tsra`]: identical metadata + naming, but the block payloads
/// are **fragment files on disk** copied straight into the `.tsra` by `pack_streaming_verified` (no
/// in-RAM `BlockPayload`). Used by the blob backend, whose fragment is the un-parsed source file
/// itself — so the verified packer is mandatory here, not optional: the two-read race in
/// [`crate::blob::to_blob_product_streaming`] (hash the file, then pack copies it) becomes a loud
/// `Err(Integrity)` instead of a dead-on-arrival `.tsra` if the source file changes between the two
/// reads. The verify happens on the bytes already buffered for the write, so the on-disk archive is
/// byte-identical to the unverified path (the conformance corpus is unaffected).
fn seal_streaming_to_tsra(
    m: Manifest,
    sources: &[(String, &Path)],
    out_dir: &Path,
    p: &crate::spec::ProductSpec,
    parents: &[&Manifest],
) -> Result<Manifest> {
    let m = apply_spec_metadata(m, &p.metadata, parents, p)?;
    let path = out_dir.join(member_filename(&m.id, MemberKind::Product));
    pack_streaming_verified(&m, sources, &path)?;
    // ADR-0042: aux/provenance.json stamp, matching seal_to_tsra above.
    stamp_ingest_provenance(&path, &ProvenanceOptions::default())?;
    Ok(m)
}

/// The identity fields a `product`-schema'd child inherits from its `parents` (ADR-0058 §5), as a
/// plain metadata map. The **streaming** ingest path applies metadata on the `WriteSession` before
/// seal (not via [`apply_spec_metadata`]), so it needs the inherited fields up-front rather than a
/// post-build `inherit_identity_from`. Schema-driven — the engine holds no field list; the first
/// parent carrying a field wins. (`study` is a manifest field carried by the spec, not returned here.)
fn inherited_metadata(parents: &[&Manifest], product: &str) -> BTreeMap<String, serde_json::Value> {
    let mut out = BTreeMap::new();
    let registry = tessera_core::SchemaRegistry::builtin();
    let Some(schema) = registry.get(product) else {
        return out;
    };
    for f in schema.inheritable_fields() {
        for parent in parents {
            if let Some(v) = parent.metadata.get(&f.id) {
                out.entry(f.id.clone()).or_insert_with(|| v.clone());
                break;
            }
        }
    }
    out
}

/// Re-seal a product with (1) **inherited identity** from its `derived_from` parents (ADR-0058 §5)
/// and (2) the spec's `[product.metadata]` overrides, then the recorded `[generation]`/`[producer]`.
/// Priority is spec metadata > the product's own value > inherited-from-parent, so an explicit value
/// always wins. Blocks are reused by digest (`from_manifest`), so `content_hash`/`id` are stable —
/// only the metadata/provenance + `manifest_hash` change. Returns `m` unchanged when there is nothing
/// to apply (no parents, no spec metadata, no generation/producer) so the common path is untouched.
///
/// `parents` are the resolved parent manifests (the engine walks `derived_from` in declared order);
/// inheritance is driven by the **child's** embedded/builtin schema (`inheritable_fields`) — the
/// engine holds no field list. The streaming path (`WriteSession`) applies spec overrides directly;
/// this is the batch-path counterpart, and inheritance rides both.
fn apply_spec_metadata(
    m: Manifest,
    meta: &BTreeMap<String, serde_json::Value>,
    parents: &[&Manifest],
    p: &crate::spec::ProductSpec,
) -> Result<Manifest> {
    let has_generation = p.generation.is_some() || p.producer.is_some();
    if meta.is_empty() && parents.is_empty() && !has_generation {
        return Ok(m);
    }
    let mut b = tessera_core::ProductBuilder::from_manifest(&m);
    // (1) Inherit schema-flagged identity from each parent (fills only fields the child lacks).
    if !parents.is_empty() {
        let registry = tessera_core::SchemaRegistry::builtin();
        if let Some(schema) = registry.get(&m.product) {
            for parent in parents {
                b.inherit_identity_from(parent, schema);
            }
        }
    }
    // (2) Spec `[product.metadata]` overrides inherited + builder-default values.
    for (k, v) in meta {
        b.with_field(k, v.clone());
    }
    // (3) The sealed generation recipe + producer identity (ADR-0058 §1/§2).
    //
    // LAYERED over whatever the lane already recorded, not written over it. `with_generation`
    // replaces the whole bag, and several lanes seal a recipe of their own *before* this runs: the
    // generic table/array lanes record the decoder triple ADR-0056 §6a requires
    // (`Decoder::record_into`), and the NIfTI lane records which transform the geometry came from.
    // A bare overwrite deleted those whenever — and only when — a spec happened to declare
    // `[product.generation]`, which is invisible to a `content_hash` comparison and leaves the
    // product claiming it was decoded by nothing in particular. Spec keys still win on a collision,
    // because the spec is the highest tier.
    if let Some(g) = &p.generation {
        let mut merged = m.generation.clone().unwrap_or_default();
        for (k, v) in &g.config {
            merged = merged.with(k, v.clone());
        }
        if let Some(r) = &g.config_ref {
            merged = merged.with_config_ref(r.clone());
        }
        b.with_generation(merged);
    }
    if let Some(pr) = &p.producer {
        b.with_producer(pr.clone());
    }
    b.seal()
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

    /// `streaming = "auto"` must send a small input to the BATCH path.
    ///
    /// Not a performance preference — a correctness fence. The conformance corpus' goldens were
    /// sealed by the whole-file path, and `auto` promoting a small fixture to streaming would be a
    /// regeneration event wearing a routing default as a disguise. Every fixture in this repo is
    /// kilobytes against a 256 MiB threshold, so `auto` has to mean `batch` for all of them.
    #[cfg(feature = "csv")]
    #[test]
    fn streaming_auto_routes_by_size_and_a_pipe_always_falls_back() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("rows.csv");
        std::fs::write(&input, "id,energy\n1,511.0\n2,7.25\n").unwrap();
        let opts = |streaming| FormatOptions::Csv {
            input: input.clone(),
            columns: vec!["id:i4".into(), "energy:f8".into()],
            delimiter: None,
            header: true,
            null_tokens: Vec::new(),
            exclude: Vec::new(),
            column_meta: crate::column_meta::ColumnMeta::empty(),
            streaming,
            batch_rows: 64 * 1024,
        };

        assert!(
            !decide_stream_table(&opts(StreamingMode::Auto), DEFAULT_STREAM_THRESHOLD_BYTES)
                .unwrap(),
            "a few bytes under a 256 MiB threshold must route to batch — this is the guard that \
             keeps `auto` from regenerating the corpus"
        );
        assert!(
            decide_stream_table(&opts(StreamingMode::Auto), 4).unwrap(),
            "and the same input must stream once it is over the threshold, or `auto` is inert"
        );
        // The explicit modes ignore the size entirely.
        assert!(!decide_stream_table(&opts(StreamingMode::Batch), 4).unwrap());
        assert!(
            decide_stream_table(&opts(StreamingMode::Stream), DEFAULT_STREAM_THRESHOLD_BYTES)
                .unwrap()
        );

        // A non-regular input cannot be read twice, so it falls back to batch even when the operator
        // asked for `stream` — `tessera ingest table <(zcat big.csv.gz)` has to keep working.
        let fifo = dir.path().join("rows.fifo");
        if mkfifo(&fifo) {
            let piped = |streaming| match opts(streaming) {
                FormatOptions::Csv { columns, .. } => FormatOptions::Csv {
                    input: fifo.clone(),
                    columns,
                    delimiter: None,
                    header: true,
                    null_tokens: Vec::new(),
                    exclude: Vec::new(),
                    column_meta: crate::column_meta::ColumnMeta::empty(),
                    streaming,
                    batch_rows: 64 * 1024,
                },
                other => other,
            };
            assert!(
                !decide_stream_table(&piped(StreamingMode::Stream), 4).unwrap(),
                "a fifo must not take the two-pass path: the second pass would read an exhausted \
                 stream and seal a silently truncated product"
            );
            assert!(!decide_stream_table(&piped(StreamingMode::Auto), 4).unwrap());
        }
    }

    /// Parquet on a pipe must say *why*, not "not a readable Parquet file".
    ///
    /// The two table lanes diverge here for a reason the format dictates: CSV is a forward scan and a
    /// pipe is fine, while Parquet keeps its schema and row groups in a FOOTER, so the reader seeks to
    /// the end before reading the start. Arrow's own error for that is indistinguishable from "this is
    /// not Parquet", which sends an operator to inspect their data when the problem is their shell.
    #[cfg(feature = "parquet")]
    #[test]
    fn parquet_on_a_pipe_explains_the_footer_rather_than_blaming_the_data() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("rows.parquet");
        if !mkfifo(&fifo) {
            return; // no `mkfifo` here; the claim is unobservable rather than false
        }
        // Opening a fifo for reading blocks until a writer appears, so give it one that writes
        // nothing and closes. The reader then gets a seek failure, which is the case under test.
        let w = std::thread::spawn({
            let fifo = fifo.clone();
            move || {
                let _ = std::fs::OpenOptions::new().write(true).open(&fifo);
            }
        });
        let err = crate::parquet_table::read_table(&fifo, &[])
            .expect_err("a fifo cannot satisfy a footer read")
            .to_string();
        let _ = w.join();
        assert!(
            err.contains("not a regular file") && err.contains("FOOTER"),
            "the message must name the cause and the fix, not just the symptom: {err}"
        );
        assert!(
            err.contains("Materialise it first"),
            "and it must say what to do: {err}"
        );
    }

    /// Create a FIFO at `path`, returning false when the platform has no `mkfifo` to call.
    ///
    /// Nothing is read from it, so no writer is needed and nothing can block: the routing decision
    /// is made from `stat`, which is the whole point — a path that is not a regular file is refused
    /// the two-pass treatment before anything tries to open it twice.
    fn mkfifo(path: &std::path::Path) -> bool {
        #[cfg(unix)]
        {
            std::process::Command::new("mkfifo")
                .arg(path)
                .status()
                .is_ok_and(|s| s.success())
        }
        #[cfg(not(unix))]
        {
            let _ = path;
            false
        }
    }

    /// The streamed and the batched sealer must produce the **same manifest** from one spec — every
    /// metadata tier included.
    ///
    /// This is the test the streaming path's one structural hazard needs. The batch path applies the
    /// spec's tiers *after* sealing (`seal_to_tsra` → `apply_spec_metadata` re-seals a built
    /// product). A streamed `.tsra` is written once, straight to disk, with no post-seal hook — so a
    /// tier the streaming path forgets to declare before the first block commits is dropped
    /// **silently**, and the loss moves `manifest_hash` while leaving `content_hash` byte-identical.
    /// A content-hash comparison, which is what "determinism test" usually means here, cannot see it
    /// at all. Hence both hashes, and hence the field-by-field assertions underneath them: when this
    /// fails, the hash tells you only *that* something diverged.
    ///
    /// Driving both sealers from ONE `ProductSpec` is deliberate. The end-to-end route cannot pin
    /// this, because `streaming` and `batch_rows` are spec fields and `spec_hash` covers the whole
    /// parsed spec: two specs that differ in which path they ask for are two different archival
    /// documents, so their `ingested_via_spec` edges differ and their `manifest_hash`es *must* too.
    /// That is the record working as intended, not a divergence — so the equality worth pinning is
    /// over one spec, two sealers.
    #[cfg(feature = "csv")]
    #[test]
    fn the_streamed_and_batched_sealers_agree_on_every_metadata_tier() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("rows.csv");
        std::fs::write(&input, "id,energy\n1,511.0\n2,7.25\n3,-0.5\n4,1.0\n").unwrap();

        // A parent to inherit from, carrying a first-class `study` and a metadata field.
        let parent = {
            let mut b = tessera_core::ProductBuilder::new("table", "parent", "the parent", TS);
            b.add_block_ref(tessera_core::block::BlockRef {
                name: "data".into(),
                kind: tessera_core::block::BlockKind::Table,
                digest: Some("blake3:00".into()),
                spec: serde_json::Value::Null,
            });
            b.with_study("STUDY-A");
            b.with_field("modality", serde_json::json!("PT"));
            b.seal().unwrap()
        };
        let parents: Vec<&Manifest> = vec![&parent];

        let p = crate::spec::ProductSpec {
            name: "tiers-01".into(),
            role: tessera_core::collection::Role::Raw,
            schema: "table".into(),
            description: None,
            derived_from: Vec::new(),
            source_label: Some("fixture/rows.csv".into()),
            // The HIGHEST tier: an explicit operator value, including one that collides with the
            // lane's own `source_format` so the precedence is actually exercised and not just the
            // presence of the key.
            metadata: [
                ("study".to_string(), serde_json::json!("STUDY-B")),
                ("source_format".to_string(), serde_json::json!("csv")),
            ]
            .into_iter()
            .collect(),
            generation: Some(
                tessera_core::provenance::Generation::default()
                    .with("energy_window_kev", serde_json::json!([425, 650])),
            ),
            producer: Some(tessera_core::Producer::new("acme-sorter", "4.2")),
            options: FormatOptions::Csv {
                input: input.clone(),
                columns: vec!["id:i4".into(), "energy:f8".into()],
                delimiter: None,
                header: true,
                null_tokens: Vec::new(),
                exclude: Vec::new(),
                column_meta: crate::column_meta::ColumnMeta::empty(),
                streaming: StreamingMode::Auto,
                // Two rows per chunk, so the encode pass really does run over several chunks.
                batch_rows: 2,
            },
        };
        // The same spec edge both ways: this is what the engine threads in, and it rides the seal.
        let extra = vec![Source::new(SPEC_PROVENANCE_ROLE, "inline-spec")
            .with_content_hash("blake3:deadbeef".to_string())];
        let cfg = tessera_io::WriteConfig::default();

        let batched = {
            let out = dir.path().join("batch-out");
            std::fs::create_dir_all(&out).unwrap();
            let decoded = decode_generic_table(&p.options).unwrap().unwrap();
            seal_generic_table(
                &decoded,
                &p,
                &input,
                generic_column_meta(&p.options),
                p.source_label.as_deref(),
                &extra,
                TS,
                &out,
                &parents,
            )
            .expect("batch seal")
        };
        let streamed = {
            let out = dir.path().join("stream-out");
            std::fs::create_dir_all(&out).unwrap();
            stream_generic_table(
                &p,
                &input,
                p.source_label.as_deref(),
                &extra,
                TS,
                &out,
                &parents,
                &cfg,
            )
            .expect("streamed seal")
        };

        // Each tier, named, so a failure says WHICH one was dropped rather than only that a hash moved.
        assert_eq!(
            streamed.metadata, batched.metadata,
            "the spec's [product.metadata] rides the stream"
        );
        assert_eq!(
            streamed.metadata.get("source_format"),
            Some(&serde_json::json!("csv")),
            "and an explicit operator value beats the lane's own default"
        );
        assert_eq!(
            streamed.study, batched.study,
            "the inherited study rides it"
        );
        assert_eq!(
            streamed.generation, batched.generation,
            "the spec recipe AND the decoder triple both ride it"
        );
        assert!(
            streamed.generation.as_ref().is_some_and(|g| {
                g.config.contains_key("energy_window_kev")
                    && g.config.contains_key(crate::decoder::RECIPE_KEY)
            }),
            "both recipe facts, not one: {:?}",
            streamed.generation
        );
        assert_eq!(
            streamed.producer, batched.producer,
            "the spec's [product.producer] rides it"
        );
        assert_eq!(
            streamed.sources, batched.sources,
            "and every provenance edge"
        );

        assert_eq!(
            streamed.ingest_transform, batched.ingest_transform,
            "and the ADR-0056 §6.2 canonicalisation receipt"
        );
        assert_eq!(
            (&batched.content_hash, &batched.manifest_hash),
            (&streamed.content_hash, &streamed.manifest_hash),
            "one spec, two sealers, one product — on BOTH hashes"
        );
    }

    /// A `.npy` spec may declare `schema = "table"` — `spec::check_no_schema_laundering` permits it
    /// because a NumPy **structured** dtype genuinely is a table and an operator cannot know their own
    /// dtype without opening the file. But nothing re-checked the claim once the file *was* open, so a
    /// spec declaring `table` over a plain array sealed an **array** product while its sealed spec said
    /// `table` — the declared-vs-actual disagreement the laundering rule exists to prevent, deferred
    /// past the point where it became checkable.
    ///
    /// Only that direction is an error: `array` is the agnostic declaration and must keep accepting
    /// either primitive, which is the whole point of the relaxation.
    #[cfg(feature = "npy")]
    #[test]
    fn a_npy_spec_claiming_table_over_a_plain_array_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let npy = dir.path().join("vol.npy");
        // A plain `<f8` array — decidedly not a record dtype.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"\x93NUMPY\x01\x00");
        let header = "{'descr': '<f8', 'fortran_order': False, 'shape': (2,), }\n";
        bytes.extend_from_slice(&(header.len() as u16).to_le_bytes());
        bytes.extend_from_slice(header.as_bytes());
        bytes.extend_from_slice(&1.0f64.to_le_bytes());
        bytes.extend_from_slice(&2.0f64.to_le_bytes());
        std::fs::write(&npy, &bytes).unwrap();

        let spec_of = |schema: &str| {
            format!(
                r#"
[collection]
name = "npy-schema-test"
timestamp = "{TS}"

[[product]]
name = "vol"
role = "raw"
schema = "{schema}"
format = "npy"
input = "{}"
"#,
                npy.display()
            )
        };
        let cfg = tessera_io::WriteConfig::for_system().workers(1);

        // The false claim is refused, and the message says what the file actually is.
        let parsed = crate::spec::parse_str(&spec_of("table")).unwrap();
        let err = run(
            &parsed,
            &PathBuf::from("inline-spec"),
            &dir.path().join("out-table"),
            &cfg,
            1,
        )
        .expect_err("declaring 'table' over a plain array must be refused")
        .to_string();
        assert!(
            err.contains("declares schema 'table'") && err.contains("plain"),
            "got: {err}"
        );

        // The agnostic declaration still works on the same file.
        let parsed = crate::spec::parse_str(&spec_of("array")).unwrap();
        run(
            &parsed,
            &PathBuf::from("inline-spec"),
            &dir.path().join("out-array"),
            &cfg,
            1,
        )
        .expect("'array' is the correct declaration for a plain dtype");
    }

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
    fn failed_member_leaves_no_orphans_or_partial_collection() {
        // Atomicity (#302): a mid-run failure must leave `out_dir` with NO orphaned `.tsra`, NO
        // partial `collection.json`, and no leftover staging dir.
        let dir = tempfile::tempdir().unwrap();
        let h5 = dir.path().join("a.h5");
        write_synth_2p(&h5, 50, "events_2p");
        // Product 1 valid; product 2 points at a dataset that doesn't exist → dispatch fails.
        let spec_toml = format!(
            r#"
[collection]
name = "atomic-test"
timestamp = "{TS}"

[[product]]
name = "good"
role = "raw"
schema = "listmode"
format = "hdf-compound"
input = "{h5}"
dataset = "events_2p"
streaming = "batch"

[[product]]
name = "bad"
role = "raw"
schema = "listmode"
format = "hdf-compound"
input = "{h5}"
dataset = "does_not_exist"
streaming = "batch"
"#,
            h5 = h5.display()
        );
        let spec_path = dir.path().join("spec.toml");
        std::fs::write(&spec_path, spec_toml).unwrap();
        let out = dir.path().join("out");
        let cfg = tessera_io::WriteConfig::for_system().workers(2);
        let parsed = parse_spec(&spec_path).unwrap();

        let result = run(
            &parsed,
            &spec_path,
            &out,
            &cfg,
            DEFAULT_STREAM_THRESHOLD_BYTES,
        );
        assert!(result.is_err(), "the missing dataset should fail the run");

        assert!(
            !out.join("collection.json").exists(),
            "partial collection.json left behind"
        );
        assert!(
            !out.join(".staging-ingest").exists(),
            "staging dir not cleaned up"
        );
        let orphans: Vec<_> = std::fs::read_dir(&out)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter(|e| e.path().extension().is_some_and(|x| x == "tsra"))
                    .map(|e| e.file_name())
                    .collect()
            })
            .unwrap_or_default();
        assert!(orphans.is_empty(), "orphaned .tsra files: {orphans:?}");
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
            let path = out.join(member_filename(&m.reference, MemberKind::Product));
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
        let member_path = out.join(member_filename(
            &coll.members[0].reference,
            MemberKind::Product,
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
            let p = out.join(member_filename(&member.reference, MemberKind::Product));
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

    /// ADR-0058 through-line (#342/#324/#416): a derived product **inherits** schema-flagged
    /// identity from its parent on BOTH the batch and streaming paths, an explicit child value wins,
    /// and a **generation** recipe + external **producer** identity ride the seal on BOTH paths —
    /// the batch raw records one, and so does the streamed derived (#416: the streaming path used to
    /// drop them silently).
    #[test]
    fn derived_inherits_identity_and_raw_records_generation() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("raw.h5");
        let b = dir.path().join("der.h5");
        write_synth_2p(&a, 40, "events_2p");
        write_synth_2p(&b, 40, "events_2p");
        let spec_text = format!(
            r#"
[collection]
name = "inherit-test"
timestamp = "{TS}"
study = "DP06"

[[product]]
name = "raw"
role = "raw"
schema = "listmode"
format = "hdf-compound"
input = "{a}"
dataset = "events_2p"
streaming = "batch"
[product.metadata]
coincidence_mode = "singles"
patient_id = "ANON1"
exam = "9999"
[product.generation.config]
energy_window_keV = "425-650"
coincidence_window_ns = "4.5"
[product.producer]
tool = "ge-listmode-daq"
version = "3.2"

[[product]]
name = "derived-batch"
role = "derived"
schema = "listmode"
derived_from = ["raw"]
format = "hdf-compound"
input = "{b}"
dataset = "events_2p"
streaming = "batch"
[product.metadata]
coincidence_mode = "prompt-coincidence"

[[product]]
name = "derived-stream"
role = "derived"
schema = "listmode"
derived_from = ["raw"]
format = "hdf-compound"
input = "{b}"
dataset = "events_2p"
streaming = "stream"
[product.metadata]
coincidence_mode = "prompt-coincidence"
patient_id = "OVERRIDE"
[product.generation.config]
tof_cal_ref = "cal-2024-01"
seed = 7
[product.producer]
tool = "coincidence-sorter"
version = "1.4"
git_commit = "0855f5f"
"#,
            a = a.display(),
            b = b.display()
        );
        let parsed = crate::spec::parse_str(&spec_text).unwrap();
        let out = dir.path().join("out");
        let cfg = tessera_io::WriteConfig::for_system().workers(2);
        // threshold = 1 so the explicit `streaming = "stream"` product really streams.
        let coll = run(&parsed, &PathBuf::from("inline-spec"), &out, &cfg, 1).unwrap();
        let mani = |i: usize| {
            let path = out.join(format!(
                "{}.tsra",
                coll.members[i].reference.replace([':', '/'], "_")
            ));
            tessera_io::Reader::open(&path).unwrap().manifest().clone()
        };
        let raw = mani(0);
        let der_batch = mani(1);
        let der_stream = mani(2);

        // The raw records the generation recipe + external producer (ADR-0058 §1/§2), sealed.
        let g = raw
            .generation
            .as_ref()
            .expect("raw must carry a generation record");
        assert_eq!(
            g.config.get("energy_window_keV"),
            Some(&serde_json::json!("425-650"))
        );
        match raw.producer.as_ref().expect("raw producer") {
            tessera_core::ProducerRef::Structured(p) => {
                assert_eq!(p.tool, "ge-listmode-daq");
                assert_eq!(p.version, "3.2");
            }
            other => panic!("expected a structured producer, got {other:?}"),
        }

        // Batch derived inherits patient_id + exam; keeps its own coincidence_mode; no recipe of its own.
        assert_eq!(
            der_batch.metadata.get("patient_id"),
            Some(&serde_json::json!("ANON1")),
            "batch path inherits identity from parent"
        );
        assert_eq!(
            der_batch.metadata.get("exam"),
            Some(&serde_json::json!("9999")),
            "batch path inherits exam"
        );
        assert_eq!(
            der_batch.metadata.get("coincidence_mode"),
            Some(&serde_json::json!("prompt-coincidence")),
            "the product's own per-level field is not inherited"
        );

        // Streaming derived inherits too (pre-seal metadata merge) — and an explicit child value wins.
        assert_eq!(
            der_stream.metadata.get("exam"),
            Some(&serde_json::json!("9999")),
            "streaming path inherits identity"
        );
        assert_eq!(
            der_stream.metadata.get("patient_id"),
            Some(&serde_json::json!("OVERRIDE")),
            "an explicit child value wins over the inherited one"
        );

        // #416: the spec's recipe + producer must ride the STREAMING seal too. The streaming writer
        // has no post-build re-seal hook, so these are declared on the `WriteSession` before the
        // first block commits — a regression here is silent provenance loss on exactly the large
        // acquisitions that select the streaming path.
        let gs = der_stream
            .generation
            .as_ref()
            .expect("streamed product must carry its spec's generation record");
        assert_eq!(
            gs.config.get("tof_cal_ref"),
            Some(&serde_json::json!("cal-2024-01")),
            "streaming path seals the spec's [product.generation.config]"
        );
        assert_eq!(gs.config.get("seed"), Some(&serde_json::json!(7)));
        match der_stream.producer.as_ref().expect("streamed producer") {
            tessera_core::ProducerRef::Structured(p) => {
                assert_eq!(p.tool, "coincidence-sorter");
                assert_eq!(p.version, "1.4");
                assert_eq!(p.git_commit.as_deref(), Some("0855f5f"));
            }
            other => panic!("expected a structured producer, got {other:?}"),
        }
    }
}
