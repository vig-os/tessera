//! Regenerate `tessera/corpus/ingest-corpus.json` — the ingest conformance goldens (ADR-0056 §5).
//!
//! ```sh
//! cargo run -p tessera-ingest --example gen_ingest_corpus > corpus/ingest-corpus.json
//! ```
//!
//! This is also **ADR-0057 Gate A's** driver for the ingest half: the gate runs it under several
//! feature configurations and requires every output to be byte-identical to every other and to the
//! committed file. A feature may decide whether a format is *readable*; it must never change the bytes
//! produced for a readable one, and comparing this output across configurations is how that sentence
//! stops being an assertion.
//!
//! Regenerating is a **deliberate corpus event**, not a routine step. Per ADR-0056 §6a's
//! value-preservation check, a decoder bump may move a fixture's `manifest_hash` (the `ingested_from`
//! edge pins the generated source bytes, and arrow-rs stamps its version into a Parquet footer) but
//! must not move `content_hash` or `id`. A moved `content_hash` means the new decoder extracted
//! different values, and the PR has to say which H-rule changed behaviour and why the new values are
//! correct.

fn main() {
    let dir = tempfile::tempdir().expect("staging dir");
    let corpus = tessera_ingest::corpus::build(dir.path()).expect("build the ingest corpus");
    println!(
        "{}",
        serde_json::to_string_pretty(&corpus).expect("serialize the corpus")
    );
}
