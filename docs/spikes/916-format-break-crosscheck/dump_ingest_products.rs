//! #916 cross-check helper, not part of the build: pack every ingest-corpus product exactly as
//! `tessera_ingest::corpus::golden` seals it, into `<out>/<name>.tsra`, and print its triple.
//!
//! The ingest corpus keeps goldens only (no `.tsra`), so this is how its block digests and readers
//! are checked. Copy it to `tessera/crates/tessera-ingest/examples/` of the checkout to dump (the
//! same file compiles at `v0.1.0-alpha.6` and at dev), run
//! `cargo clean -p tessera-ingest && cargo run -p tessera-ingest --example dump_ingest_products -- <out>`
//! from `tessera/`, then delete the copy. The `cargo clean` is required when the target dir is shared
//! with another checkout (#979).
use std::path::Path;
fn main() {
    let out = std::env::args().nth(1).expect("out dir");
    let out = Path::new(&out);
    std::fs::create_dir_all(out).unwrap();
    let dir = tempfile::tempdir().unwrap();
    for f in tessera_ingest::corpus::enabled_fixtures() {
        let input = (f.build)(dir.path()).unwrap();
        let options = (f.options)(input.clone());
        let (source_format, decoder) =
            tessera_ingest::engine::generic_source_and_decoder(&options).unwrap();
        let (m, payloads) = tessera_ingest::engine::seal_generic_product(
            &options,
            &tessera_ingest::canonical::GenericIngest {
                name: f.name,
                timestamp: "2024-01-01T00:00:00Z",
                description: "ingest conformance fixture",
                source_format,
                source_path: &input,
                source_label: Some("corpus/ingest"),
                extra_sources: &[],
                decoder,
                generation: None,
                column_meta: &tessera_ingest::column_meta::ColumnMeta::empty(),
            },
        )
        .unwrap()
        .unwrap();
        let p = out.join(format!("{}.tsra", f.name));
        tessera_io::pack(&m, &payloads, &p).unwrap();
        println!("{} {} {:?} {:?}", f.name, m.id, m.content_hash, m.manifest_hash); // guardrails-ok
    }
}
