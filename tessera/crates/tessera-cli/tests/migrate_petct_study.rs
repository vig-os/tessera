//! Worked demo for the **PET/CT study migration shape** (spike #235) — the fine-grained
//! `--meta`-tagged collection an operator gets when they ingest one real PET/CT study via
//! `docs/examples/migrate-petct-study.toml`, exercised end-to-end on **synthetic data** so
//! it stays green in the hermetic gate (NEVER touches PHI under `/home/larsgerchow/Data/HDD/`).
//!
//! Proves the canonical migration shape:
//!
//!   1. one collection bundles **two products** for the same study:
//!        - `recon` (the hot, range-readable, cohort-pruneable tier — here built from a raw
//!          headerless binary; in real life it's `dicom-series` / `nifti` / `dicom`),
//!        - `blob` (the cold, bit-faithful archival tier — the original vendor file
//!          preserved verbatim with a blake3 digest);
//!   2. the `recon` is wired to its source-of-record blob via a `derived_from`
//!      provenance edge that pins the parent's `manifest_hash` (the integrity chain);
//!   3. EVERY member carries `study=` / `modality=` in its sealed `metadata` map (the
//!      `[product.metadata]` block in the spec → cohort-pruneable downstream);
//!   4. `tessera verify` + `tessera schema` pass on every member;
//!   5. `tessera tree` prints the structured hierarchy.
//!
//! Runs the real `tessera` binary via `CARGO_BIN_EXE_tessera`, so it executes inside the
//! hermetic flake check without cargo at test time (same pattern as `tests/lifecycle.rs`).

use std::path::Path;
use std::process::Command;

fn tessera() -> Command {
    Command::new(env!("CARGO_BIN_EXE_tessera"))
}

/// Run, assert exit 0, return stdout (captures stderr in the failure message so the
/// schema-recommended-field warn doesn't get lost on failure).
fn ok(cmd: &mut Command) -> String {
    let o = cmd.output().unwrap();
    assert!(
        o.status.success(),
        "expected success: {cmd:?}\nstderr: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8(o.stdout).unwrap()
}

/// Write a synthetic 8×8×8 little-endian `int16` volume — the "CT recon" the operator
/// would normally get from `format = "dicom-series"`. Bytes are deterministic so re-runs
/// of this test produce byte-identical `.tsra` files (the engine's determinism contract).
fn write_synth_volume(path: &Path) {
    let voxels: Vec<i16> = (0..(8 * 8 * 8))
        .map(|k| (k as i16).wrapping_mul(7) - 1024)
        .collect();
    let bytes: Vec<u8> = voxels.iter().flat_map(|v| v.to_le_bytes()).collect();
    std::fs::write(path, &bytes).unwrap();
}

/// Write a synthetic "vendor raw" file — opaque high-entropy bytes that simulate the
/// `.7z` / `.l64` blob the scanner emitted. Tessera will preserve these bit-faithfully
/// (blake3-verified) so `tessera extract` later recovers byte-identical content.
fn write_synth_vendor_raw(path: &Path) {
    let bytes: Vec<u8> = (0..20_000u32)
        .map(|k| (k.wrapping_mul(2_654_435_761) >> 11) as u8)
        .collect();
    std::fs::write(path, &bytes).unwrap();
}

#[test]
fn petct_migration_shape_seals_a_fine_grained_collection_with_recon_plus_blob() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();

    // ── Synthetic source files (NEVER real patient data) ────────────────────────────
    let ct_raw = d.join("ct.raw");
    let vendor_raw = d.join("acquisition.bin");
    write_synth_volume(&ct_raw);
    write_synth_vendor_raw(&vendor_raw);

    // ── Spec: the migration shape this spike formalises ─────────────────────────────
    // Two products, same study, joined by a `derived_from` edge from the recon to the
    // preserved blob. Each carries `[product.metadata] study=…` so the collection is
    // cohort-pruneable. The `recon` member supplies `modality` explicitly because the
    // `raw` backend has no header to read it from (DICOM / NIfTI backends auto-populate
    // it; this is what an operator falling back to `format = "raw"` MUST add by hand).
    let spec_path = d.join("spec.toml");
    let spec_text = format!(
        r#"
[collection]
name = "SYNTH-PETCT-2024-01"
description = "Synthetic PET/CT migration spike — recon + blob fine-grained collection"
timestamp = "2024-01-01T00:00:00Z"
study = "SYNTH-PETCT-2024-01"

[spec]
description = "Spike #235 demo: one study → one recon (hot tier) + one blob (cold tier)"

# ── Cold tier: the original vendor file, bit-faithfully preserved ──────────────────
[[product]]
name = "vendor-raw"
role = "raw"
schema = "blob"
description = "Original vendor acquisition bundle (opaque, blake3-sealed)"
format = "blob"
input = "{vendor}"
media_type = "application/octet-stream"

[product.metadata]
study = "SYNTH-PETCT-2024-01"

# ── Hot tier: the CT recon, derived_from the preserved vendor blob ────────────────
[[product]]
name = "recon-ct"
role = "derived"
schema = "recon"
description = "CT reconstruction (synthetic int16 volume)"
derived_from = ["vendor-raw"]
format = "raw"
input = "{ct}"
shape = [8, 8, 8]
dtype = "i2"

[product.metadata]
study = "SYNTH-PETCT-2024-01"
modality = {{ _vocabulary = "DICOM", _code = "CT" }}
"#,
        vendor = vendor_raw.display(),
        ct = ct_raw.display()
    );
    std::fs::write(&spec_path, &spec_text).unwrap();

    // ── Run the engine via the CLI binary ───────────────────────────────────────────
    let out = d.join("out");
    let stdout = ok(tessera().args([
        "ingest",
        "--spec",
        spec_path.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]));
    assert!(
        stdout.contains("ingested collection") && stdout.contains("2 members"),
        "expected 2-member collection in CLI output, got:\n{stdout}"
    );

    // ── Both .tsra members + the collection descriptor exist ────────────────────────
    let coll_json =
        std::fs::read_to_string(out.join("collection.json")).expect("collection.json missing");
    let coll = tessera_core::Collection::from_json_verified(&coll_json).unwrap();
    assert_eq!(coll.members.len(), 2, "collection has 2 members");
    assert_eq!(
        coll.study.as_deref(),
        Some("SYNTH-PETCT-2024-01"),
        "collection-level `study` (from [collection].study) survives the seal"
    );
    // Member order matches the TOML `[[product]]` order (load-bearing for content_hash MMR).
    assert_eq!(
        (
            coll.members[0].reference.as_str().starts_with("blake3:"),
            coll.members[1].reference.as_str().starts_with("blake3:")
        ),
        (true, true)
    );

    // ── Every member: verify (re-hash blocks) + schema (validate the product contract) ─
    let mut by_id_path = std::collections::BTreeMap::<String, std::path::PathBuf>::new();
    for m in &coll.members {
        let p = out.join(format!("{}.tsra", m.reference.replace([':', '/'], "_")));
        assert!(p.exists(), "missing sealed product: {}", p.display());
        ok(tessera().args(["verify", p.to_str().unwrap()]));
        ok(tessera().args(["schema", p.to_str().unwrap()]));
        // `tree` renders without erroring (CLI smoke — the printed hierarchy is what an
        // operator scans during a migration to confirm the shape took).
        let tree_out = ok(tessera().args(["tree", p.to_str().unwrap()]));
        assert!(!tree_out.is_empty(), "tree printed nothing");
        by_id_path.insert(m.reference.clone(), p);
    }

    // ── Read each manifest, prove `study` / `modality` are baked in ─────────────────
    let mut by_product = std::collections::BTreeMap::<String, tessera_core::Manifest>::new();
    for (id, p) in &by_id_path {
        let r = tessera_io::Reader::open(p).unwrap();
        by_product.insert(r.manifest().product.clone(), r.manifest().clone());
        let _ = id;
    }
    let blob = by_product.get("blob").expect("blob member missing");
    let recon = by_product.get("recon").expect("recon member missing");

    // (3) the `--meta`-equivalent (`[product.metadata]`) fields are baked into the seal.
    assert_eq!(
        blob.metadata.get("study"),
        Some(&serde_json::json!("SYNTH-PETCT-2024-01")),
        "blob carries `study=` in its sealed manifest metadata — cohort-pruneable"
    );
    assert_eq!(
        recon.metadata.get("study"),
        Some(&serde_json::json!("SYNTH-PETCT-2024-01")),
        "recon carries `study=` in its sealed manifest metadata — cohort-pruneable"
    );
    assert_eq!(
        recon.metadata.get("modality"),
        Some(&serde_json::json!({"_vocabulary": "DICOM", "_code": "CT"})),
        "recon carries `modality=` as an fd5 Coded value (required by the `recon` schema)"
    );

    // (2) `derived_from` integrity chain — the recon references the blob's id and pins
    // the parent's `manifest_hash`, so `verify_chain` walks it cleanly.
    let df = recon
        .sources
        .iter()
        .find(|s| s.role == "derived_from")
        .expect("recon must carry a `derived_from` edge to vendor-raw");
    assert_eq!(
        df.reference, blob.id,
        "derived_from edge references the blob's id"
    );
    assert_eq!(
        df.content_hash.as_deref(),
        Some(blob.manifest_hash.as_deref().unwrap_or("")),
        "derived_from edge pins parent's manifest_hash (the integrity chain)"
    );
    let mut resolver = std::collections::BTreeMap::<String, tessera_core::Manifest>::new();
    resolver.insert(blob.id.clone(), blob.clone());
    tessera_core::provenance::verify_chain(recon, &resolver)
        .expect("derived_from chain verifies (recon → blob)");

    // (4) Spec-provenance: every member also carries an `ingested_via_spec` edge pinned
    // to the spec's content_hash — re-running the same spec produces the same hash.
    for m in by_product.values() {
        let via = m
            .sources
            .iter()
            .find(|s| s.role == tessera_ingest::engine::SPEC_PROVENANCE_ROLE)
            .expect("every member carries `ingested_via_spec`");
        assert!(via.content_hash.is_some(), "spec edge pins spec_hash");
    }

    // ── Cohort-pruneable smoke: in-process filter on study= produces both members ──
    // (The wire-level `cohort_prune_before_fetch_skips_non_matching_product` capstone in
    // tessera-io/src/cloud.rs proves the SAME metadata drives byte-saving over MinIO.
    // Here we just prove the metadata exists at the right place to be filtered.)
    let matching: Vec<_> = by_product
        .values()
        .filter(|m| m.metadata.get("study") == Some(&serde_json::json!("SYNTH-PETCT-2024-01")))
        .collect();
    assert_eq!(
        matching.len(),
        2,
        "both members match the cohort filter `study == SYNTH-PETCT-2024-01`"
    );

    // ── Determinism re-run (the load-bearing identity guarantee) ──────────────────
    // A second `tessera ingest --spec` into a fresh out dir produces the SAME collection id
    // + the SAME per-member ids. This is what makes the migration *re-runnable* — an
    // operator can re-ingest a study on a different host / under different worker counts
    // and get bit-identical archives.
    let out2 = d.join("out2");
    ok(tessera().args([
        "ingest",
        "--spec",
        spec_path.to_str().unwrap(),
        "--out",
        out2.to_str().unwrap(),
    ]));
    let coll2 = tessera_core::Collection::from_json_verified(
        &std::fs::read_to_string(out2.join("collection.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        coll.id, coll2.id,
        "collection id is deterministic across re-runs"
    );
    assert_eq!(coll.content_hash, coll2.content_hash);
    assert_eq!(coll.manifest_hash, coll2.manifest_hash);
    for (a, b) in coll.members.iter().zip(coll2.members.iter()) {
        assert_eq!(a.reference, b.reference, "member ids are deterministic");
        assert_eq!(a.manifest_hash, b.manifest_hash);
    }

    // ── Cold-tier round-trip: `tessera extract` recovers the vendor bytes byte-identical ─
    let recovered = d.join("recovered.bin");
    ok(tessera().args([
        "extract",
        by_id_path.get(&blob.id).unwrap().to_str().unwrap(),
        "data",
        recovered.to_str().unwrap(),
    ]));
    assert_eq!(
        std::fs::read(&recovered).unwrap(),
        std::fs::read(&vendor_raw).unwrap(),
        "the blob `data` block extracts byte-identical to the source vendor file (cold-tier promise)"
    );
}
