//! P0 "guarding scaffold" tests for the Tessera core spine (no storage backend required).
//! Derived from the survey of arrow/parquet/lance, zarrs/h5py/ome-ngff, iceberg/delta, and
//! blake3/proptest/FAIR practice — see `tessera/docs/TEST-PLAN.md`.

use proptest::prelude::*;
use tessera_core::block::array::{ArrayBlock, ArraySpec};
use tessera_core::block::table::{Column, TableBlock, TableSpec};
use tessera_core::block::{BlockKind, BlockRef};
use tessera_core::identity;
use tessera_core::manifest::Manifest;
use tessera_core::ProductBuilder;

/// Canonical sample product: a recon (int16 volume) + a listmode (events) block.
fn sample_product() -> Manifest {
    let vol = ArrayBlock::new(
        "volume",
        ArraySpec::new(vec![487, 512, 512], "int16").with_rescale(1.0, -1024.0),
    );
    let spec = TableSpec {
        columns: vec![Column {
            name: "lt".into(),
            dtype: "f4".into(),
            codec: Some("zstd".into()),
        }],
        rows: 2_696_935,
        row_index: Some("ms".into()),
    };
    let events = TableBlock::new("events_3p", spec);
    let mut b = ProductBuilder::new("recon", "DP06", "DP06 CT+events", "2023-12-08T00:00:00Z");
    b.add_block(&vol).unwrap();
    b.add_block(&events).unwrap();
    b.seal().unwrap()
}

// ---- hash / identity ---------------------------------------------------------------------

#[test]
fn digest_kat_empty_input() {
    // BLAKE3 known-answer vector for the empty input — pins algo + hex encoding + prefix.
    assert_eq!(
        tessera_core::hash::digest(b""),
        "blake3:af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
    );
}

#[test]
fn merkle_root_deterministic_and_order_sensitive() {
    let a = tessera_core::hash::digest(b"a");
    let b = tessera_core::hash::digest(b"b");
    let r1 = tessera_core::hash::merkle_root(&[a.clone(), b.clone()]);
    let r2 = tessera_core::hash::merkle_root(&[a.clone(), b.clone()]);
    assert_eq!(r1, r2, "same ordered digests must give the same root");
    assert_ne!(
        r1,
        tessera_core::hash::merkle_root(&[b, a]),
        "order is semantic"
    );
}

#[test]
fn id_timestamp_normalised_to_utc() {
    // Same instant, two offsets -> one id.
    let z = Manifest::new("recon", "DP06", "x", "2023-12-08T00:00:00Z");
    let plus1 = Manifest::new("recon", "DP06", "x", "2023-12-08T01:00:00+01:00");
    assert_eq!(z.id, plus1.id);
    assert_eq!(z.timestamp, "2023-12-08T00:00:00Z");
    // A genuinely different instant must differ.
    let other = Manifest::new("recon", "DP06", "x", "2023-12-08T02:00:00Z");
    assert_ne!(z.id, other.id);
    // normalize_timestamp leaves non-RFC3339 input untouched (lenient).
    assert_eq!(identity::normalize_timestamp("not-a-date"), "not-a-date");
}

// ---- seal / immutability -----------------------------------------------------------------

#[test]
fn seal_is_idempotent_for_identical_inputs() {
    let m1 = sample_product();
    let m2 = sample_product();
    assert_eq!(m1.id, m2.id);
    assert_eq!(m1.content_hash, m2.content_hash);
    assert_eq!(m1.to_json().unwrap(), m2.to_json().unwrap());
}

#[test]
fn seal_immutability_invariants() {
    let m = sample_product();
    assert!(m.is_sealed());
    assert!(m.content_hash.as_deref().unwrap().starts_with("blake3:"));
    assert_eq!(m.blocks.len(), 2);
    assert!(
        m.blocks.iter().all(|b| b.digest.is_some()),
        "no block may lack a digest"
    );
}

#[test]
fn seal_rejects_block_without_digest() {
    // A precomputed ref missing its digest must NOT be silently dropped from the Merkle root.
    let mut b = ProductBuilder::new("recon", "DP06", "x", "2023-12-08T00:00:00Z");
    b.add_block_ref(BlockRef {
        name: "orphan".into(),
        kind: BlockKind::Array,
        digest: None,
        spec: serde_json::Value::Null,
    });
    assert!(b.seal().is_err(), "sealing a digest-less block must fail");
}

// ---- tamper / reorder --------------------------------------------------------------------

#[test]
fn tamper_one_block_changes_content_hash() {
    let base = sample_product();
    // Same product but the volume has a different shape -> different block digest -> new root.
    let vol2 = ArrayBlock::new("volume", ArraySpec::new(vec![488, 512, 512], "int16"));
    let mut b = ProductBuilder::new("recon", "DP06", "DP06 CT+events", "2023-12-08T00:00:00Z");
    b.add_block(&vol2).unwrap();
    let events = TableBlock::new(
        "events_3p",
        TableSpec {
            columns: vec![Column {
                name: "lt".into(),
                dtype: "f4".into(),
                codec: Some("zstd".into()),
            }],
            rows: 2_696_935,
            row_index: Some("ms".into()),
        },
    );
    b.add_block(&events).unwrap();
    let tampered = b.seal().unwrap();
    assert_ne!(base.content_hash, tampered.content_hash);
}

#[test]
fn block_reorder_changes_content_hash() {
    let vol = ArrayBlock::new("volume", ArraySpec::new(vec![4, 4, 4], "int16"));
    let tbl = TableBlock::new(
        "events",
        TableSpec {
            columns: vec![Column {
                name: "lt".into(),
                dtype: "f4".into(),
                codec: None,
            }],
            rows: 1,
            row_index: None,
        },
    );
    let mut a = ProductBuilder::new("p", "n", "d", "2023-12-08T00:00:00Z");
    a.add_block(&vol).unwrap();
    a.add_block(&tbl).unwrap();
    let mut b = ProductBuilder::new("p", "n", "d", "2023-12-08T00:00:00Z");
    b.add_block(&tbl).unwrap();
    b.add_block(&vol).unwrap();
    assert_ne!(
        a.seal().unwrap().content_hash,
        b.seal().unwrap().content_hash
    );
}

// ---- version handling --------------------------------------------------------------------

#[test]
fn unknown_tessera_version_errors_cleanly() {
    let json = sample_product()
        .to_json()
        .unwrap()
        .replace("\"0.0.0\"", "\"9.9.9\"");
    let err = Manifest::from_json(&json);
    assert!(
        err.is_err(),
        "a future major version must be refused, not silently accepted"
    );
    // a manifest at the supported version still parses
    assert!(Manifest::from_json(&sample_product().to_json().unwrap()).is_ok());
}

// ---- dtype allowlist (int16 recommended, not required) -----------------------------------

#[test]
fn array_dtype_allowlist_not_int16_only() {
    use tessera_core::dtype::DType;
    assert!(DType::is_supported("int16")); // recommended for CT/PET
    assert!(DType::is_supported("float32")); // computed maps (SUV/parametric/lifetime/μ-map)
    assert!(DType::is_supported("uint32")); // counts/labels
    assert!(!DType::is_supported("float24")); // junk

    // a non-int16 (float32) array is perfectly valid
    let mu = ArrayBlock::new("mu_map", ArraySpec::new(vec![2, 2, 2], "float32"));
    let mut ok = ProductBuilder::new("recon", "x", "d", "2023-12-08T00:00:00Z");
    assert!(ok.add_block(&mu).is_ok());

    // an unsupported dtype is rejected at add time (validated in digest)
    let bad = ArrayBlock::new("v", ArraySpec::new(vec![2, 2, 2], "float24"));
    let mut nope = ProductBuilder::new("recon", "x", "d", "2023-12-08T00:00:00Z");
    assert!(nope.add_block(&bad).is_err());
}

// ---- serde roundtrip (property) ----------------------------------------------------------

proptest! {
    #[test]
    fn manifest_json_roundtrip(
        product in "[a-z][a-z0-9_]{0,7}",
        name in "[A-Za-z0-9 _-]{1,16}",
        description in "[^\"\\\\\\u{0}-\\u{1f}]{0,40}",
        kinds in prop::collection::vec(any::<bool>(), 0..4),
    ) {
        let mut b = ProductBuilder::new(product, name, description, "2023-12-08T00:00:00Z");
        // keep owned blocks alive for the &dyn Block borrows
        let arrays: Vec<_> = kinds.iter().enumerate().filter(|(_, k)| **k)
            .map(|(i, _)| ArrayBlock::new(format!("a{i}"), ArraySpec::new(vec![2, 2, 2], "int16"))).collect();
        let tables: Vec<_> = kinds.iter().enumerate().filter(|(_, k)| !**k)
            .map(|(i, _)| TableBlock::new(format!("t{i}"),
                TableSpec { columns: vec![Column { name: "c".into(), dtype: "f4".into(), codec: None }], rows: i as u64, row_index: None })).collect();
        for a in &arrays { b.add_block(a).unwrap(); }
        for t in &tables { b.add_block(t).unwrap(); }
        let m = b.seal().unwrap();
        let round = Manifest::from_json(&m.to_json().unwrap()).unwrap();
        prop_assert_eq!(m, round);
    }
}
