//! ADR-0059 C4 coverage guard: **every** public array-product entry point must attach the
//! `{hash, stats}` chunk-index for an integer array.
//!
//! Default-on is realised at the product-building paths rather than inside `array_block`, which
//! keeps batch and streaming byte-identical on both sides of the indexed/un-indexed pair — but it
//! means a NEW builder can silently skip the sidecar and nothing else would notice. Four paths
//! (raw, NIfTI, generic array, and the CLI pyramid) were in exactly that state after the first
//! pass. This test exists so the next one cannot be.
//!
//! The deliberate exclusions, which are NOT bugs and are recorded in ADR-0059 §11:
//!
//!  - `array_block` / `array_job` — the un-indexed primitives. `array_block_with_index` and
//!    `array_job_indexed` are their indexed counterparts; flipping the primitives would break the
//!    batch/stream byte-identity their contracts promise.
//!  - `tessera bench` — measures encode throughput, so indexing would change what it measures.
//!  - `tessera commit --add-block` — COPIES an already-encoded block rather than building one, so
//!    it has no values to fold. (It does remove a stale `<name>.cidx`; see the #347 review.)

use std::io::Write;
use tessera_core::block::array::ArraySpec;
use tessera_core::Manifest;
use tessera_io::array::ArrayData;
use tessera_io::BlockPayload;

/// Assert a sealed product carries `<block>.cidx` beside `<block>`, and that it is a real index.
fn assert_indexed(what: &str, sealed: &Manifest, payloads: &[BlockPayload], block: &str) {
    let cidx = format!("{block}.cidx");
    let names: Vec<&str> = sealed.blocks.iter().map(|b| b.name.as_str()).collect();
    assert!(
        names.contains(&block),
        "{what}: expected a '{block}' block, got {names:?}"
    );
    assert!(
        names.contains(&cidx.as_str()),
        "{what}: an INTEGER array must carry '{cidx}' by default (ADR-0059 C4) — got {names:?}"
    );

    // The manifest ref must bind to the data block's digest, and the payload must be present.
    let data_digest = sealed
        .blocks
        .iter()
        .find(|b| b.name == block)
        .and_then(|b| b.digest.clone())
        .unwrap_or_else(|| panic!("{what}: '{block}' has no digest"));
    let sref = sealed
        .blocks
        .iter()
        .find(|b| b.name == cidx)
        .unwrap_or_else(|| panic!("{what}: no '{cidx}' ref"));
    assert_eq!(
        sref.spec["indexes"], block,
        "{what}: the sidecar must declare which block it indexes"
    );
    assert_eq!(
        sref.spec["indexed_digest"], data_digest,
        "{what}: the sidecar must be bound to that block's digest (M1)"
    );
    assert_eq!(sref.spec["recipe"], "chunk_index@2", "{what}: recipe");
    assert!(
        payloads.iter().any(|p| p.name == cidx),
        "{what}: the '{cidx}' payload must be packed, not just referenced"
    );
}

fn int16_cube() -> (Vec<u64>, Vec<i16>) {
    let shape = vec![4u64, 4, 4];
    let vals: Vec<i16> = (0..64).map(|k| (k as i16 * 37) - 512).collect();
    (shape, vals)
}

#[test]
fn raw_ingest_indexes_by_default() {
    let (shape, vals) = int16_cube();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vol.raw");
    let mut f = std::fs::File::create(&path).unwrap();
    for v in &vals {
        f.write_all(&v.to_le_bytes()).unwrap();
    }
    drop(f);

    let (sealed, payloads) = tessera_ingest::raw::to_recon_product(
        &path,
        shape,
        "i2",
        "raw-guard",
        "2024-01-01T00:00:00Z",
        Some("raw-guard"),
        &[],
    )
    .unwrap();
    assert_indexed("raw::to_recon_product", &sealed, &payloads, "volume");
}

#[test]
fn generic_array_ingest_indexes_by_default() {
    let (shape, vals) = int16_cube();
    let spec = ArraySpec::new(shape, "int16");
    let data = ArrayData::I16(vals);
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("a.npy");
    std::fs::write(&src, b"placeholder").unwrap();

    let meta = tessera_ingest::column_meta::ColumnMeta::default();
    let opts = tessera_ingest::canonical::GenericIngest {
        name: "generic-guard",
        timestamp: "2024-01-01T00:00:00Z",
        description: "generic array guard",
        source_format: "npy",
        source_path: &src,
        source_label: Some("generic-guard"),
        extra_sources: &[],
        decoder: tessera_ingest::decoder::Decoder::PARQUET,
        generation: None,
        column_meta: &meta,
    };
    let (sealed, payloads) =
        tessera_ingest::canonical::to_array_product(&spec, &data, &[], &opts).unwrap();
    // The generic lane names its block by its own constant, not "volume".
    let block = sealed
        .blocks
        .iter()
        .find(|b| !b.name.ends_with(".cidx"))
        .map(|b| b.name.clone())
        .expect("an array block");
    assert_indexed(
        "canonical::to_array_product",
        &sealed,
        &payloads,
        &block,
    );
}

/// A minimal single-file NIfTI-1 (`n+1`) holding an int16 cube — just the fields the reader needs.
fn minimal_nii(shape: [i16; 3], vals: &[i16]) -> Vec<u8> {
    let mut h = vec![0u8; 352];
    let put_i16 = |h: &mut [u8], o: usize, v: i16| h[o..o + 2].copy_from_slice(&v.to_le_bytes());
    let put_i32 = |h: &mut [u8], o: usize, v: i32| h[o..o + 4].copy_from_slice(&v.to_le_bytes());
    let put_f32 = |h: &mut [u8], o: usize, v: f32| h[o..o + 4].copy_from_slice(&v.to_le_bytes());
    put_i32(&mut h, 0, 348); // sizeof_hdr
    put_i16(&mut h, 40, 3); // dim[0] = rank
    put_i16(&mut h, 42, shape[0]);
    put_i16(&mut h, 44, shape[1]);
    put_i16(&mut h, 46, shape[2]);
    put_i16(&mut h, 70, 4); // datatype = DT_INT16
    put_i16(&mut h, 72, 16); // bitpix
    put_f32(&mut h, 76, 1.0); // pixdim[0] (qfac)
    put_f32(&mut h, 80, 1.0); // pixdim[1..3]
    put_f32(&mut h, 84, 1.0);
    put_f32(&mut h, 88, 1.0);
    put_f32(&mut h, 108, 352.0); // vox_offset — data starts after the header
    h[344..348].copy_from_slice(b"n+1\0");
    for v in vals {
        h.extend_from_slice(&v.to_le_bytes());
    }
    h
}

#[test]
fn nifti_ingest_indexes_by_default() {
    let (_, vals) = int16_cube();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vol.nii");
    std::fs::write(&path, minimal_nii([4, 4, 4], &vals)).unwrap();

    let img = tessera_ingest::nifti::read_nifti(&path).expect("minimal .nii parses");
    let (sealed, payloads) = tessera_ingest::nifti::to_recon_product(
        &img,
        "nifti-guard",
        "2024-01-01T00:00:00Z",
        "nifti-guard",
        None,
        &[],
    )
    .unwrap();
    assert_indexed("nifti::to_recon_product", &sealed, &payloads, "volume");
}

#[test]
fn a_float_array_is_excluded_by_dtype_not_by_path() {
    // The exclusion must be the integer-core rule, not a path that forgot. Same entry point, float
    // dtype -> no sidecar, and no error either.
    let spec = ArraySpec::new(vec![4, 4, 4], "float32");
    let data = ArrayData::F32(vec![0.5; 64]);
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("a.npy");
    std::fs::write(&src, b"placeholder").unwrap();
    let meta = tessera_ingest::column_meta::ColumnMeta::default();
    let opts = tessera_ingest::canonical::GenericIngest {
        name: "float-guard",
        timestamp: "2024-01-01T00:00:00Z",
        description: "float array guard",
        source_format: "npy",
        source_path: &src,
        source_label: Some("float-guard"),
        extra_sources: &[],
        decoder: tessera_ingest::decoder::Decoder::PARQUET,
        generation: None,
        column_meta: &meta,
    };
    let (sealed, _) = tessera_ingest::canonical::to_array_product(&spec, &data, &[], &opts).unwrap();
    assert!(
        !sealed.blocks.iter().any(|b| b.name.ends_with(".cidx")),
        "a float array has no integer chunk stats, so it must carry no sidecar"
    );
}
