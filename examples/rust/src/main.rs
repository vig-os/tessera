//! Create a sealed fd5 file in Rust.
//!
//! Usage:
//!     cargo run --manifest-path examples/rust/Cargo.toml

use std::path::Path;

fn main() -> fd5::Fd5Result<()> {
    // Register a test product schema (real apps register domain schemas)
    fd5::register_schema(Box::new(fd5::product::TestProductSchema));

    // Create a builder — writes root attrs and opens a temp HDF5 file
    let mut builder = fd5::create(
        Path::new("/tmp/fd5-examples"),
        "test/product",
        "hello-fd5-rust",
        "First fd5 file created with the Rust builder",
        "2026-01-01T00:00:00Z",
    )?;

    // Write product data through the schema
    let data = serde_json::json!({
        "values": (0..100).map(|i| i as f32).collect::<Vec<f32>>()
    });
    builder.write_product(&data)?;

    // Write metadata (nested dict → HDF5 groups/attrs)
    let meta = serde_json::json!({
        "scanner": "Example PET/CT",
        "institution": "fd5 Lab"
    });
    builder.write_metadata(&meta)?;

    // Seal: validate → embed schema → compute id + content_hash → atomic rename
    let path = builder.seal()?;
    println!("Created  : {}", path.display());

    // Verify the sealed file
    match fd5::verify(&path)? {
        fd5::Fd5Status::Valid(hash) => println!("Verified : PASS ({})", hash),
        status => println!("Verified : FAIL ({:?})", status),
    }

    Ok(())
}
