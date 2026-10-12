//! A/B harness for #551 — dump the three values that a dicom-rs bump could move, for one fixture.
//!
//! Temporary scaffolding: run it under each dicom-rs version and diff the output. Not a test, because the
//! point is to compare across two dependency sets rather than to assert within one.
fn main() {
    let path = std::path::Path::new("crates/tessera-ingest/tests/implicit_vr_nested.dcm");
    let img = tessera_ingest::dicom::read_image(path).expect("read the implicit-VR fixture");
    let (m, _payloads) = tessera_ingest::dicom::to_recon_product(
        &img,
        "implicit-vr-nested",
        "2024-01-01T00:00:00Z",
        "implicit_vr_nested.dcm",
        Some("blake3:fixed-for-the-ab"),
        &[],
    )
    .expect("build the product");
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "dicom_crate_version": option_env!("TESSERA_DEP_DICOM").unwrap_or("unknown"),
            "content_hash": m.content_hash,
            "manifest_hash": m.manifest_hash,
            "header_json": img.header_json,
        }))
        .unwrap()
    );
}
