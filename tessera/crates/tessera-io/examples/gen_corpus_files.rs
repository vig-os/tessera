//! Write each conformance fixture to `corpus/files/<name>.tsra` — the concrete `.tsra` test vectors
//! that an independent implementation (the v1.0 pure-Python reader gate) consumes alongside
//! `corpus.json`. Deterministic: regenerate only on an intended format change.
//!
//! Run from `tessera/`: `cargo run -p tessera-io --example gen_corpus_files`

use std::path::Path;

fn main() {
    let dir = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus/files"));
    std::fs::create_dir_all(dir).unwrap();
    for f in tessera_io::conformance::fixtures() {
        let path = dir.join(format!("{}.tsra", f.name));
        tessera_io::pack(&f.manifest, &f.payloads, &path).unwrap();
        println!("wrote {}", path.display());
    }
}
