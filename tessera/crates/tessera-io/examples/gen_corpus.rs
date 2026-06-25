//! Generate the golden conformance index from the deterministic fixtures.
//!
//! Run: `cargo run -p tessera-io --example gen_corpus > ../../corpus/corpus.json`
//! (from `tessera/`). Re-run only when an *intended* format change shifts the hashes — that is
//! the moment to bump the spec version. The `conformance` test then locks the new goldens.

fn main() {
    let goldens = tessera_io::conformance::goldens();
    println!("{}", serde_json::to_string_pretty(&goldens).unwrap());
}
