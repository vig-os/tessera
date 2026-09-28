//! The ingest conformance corpus gate (ADR-0056 §5, ADR-0057 §5).
//!
//! Two guarantees ride on this file, and they fail differently:
//!
//! 1. **Anti-vacuity.** The fixture count for this configuration must match the *declared* count
//!    before a single hash is compared. ADR-0057 §5 asks for this by name, and the reason is local
//!    history: the trycmd docs-as-tests ran zero cases in CI for a period and reported the same green
//!    as a full run. A feature-conditional corpus is exactly that shape.
//! 2. **Value preservation.** Every enabled fixture reproduces its committed `id` / `content_hash` /
//!    `manifest_hash`. A `content_hash` that moved means a decoder extracted different values.

#![cfg(feature = "parquet")]

use std::collections::BTreeSet;

use tessera_ingest::corpus;

/// The committed goldens.
fn committed() -> corpus::IngestCorpus {
    // `CARGO_MANIFEST_DIR` is `crates/tessera-ingest`; the corpus lives at the workspace root beside
    // `tessera-io`'s, so both conformance suites are in one place a reviewer can find.
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/ingest-corpus.json");
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&text).expect("parse corpus/ingest-corpus.json")
}

/// Which declared configuration this test binary was built as.
///
/// Derived from `cfg!`, so it describes the binary rather than trusting an env var a CI job might
/// forget to set. `full` and `all-features` are indistinguishable from `default` *from the ingest
/// lane's point of view* — which is the point: neither adds a generic decoder, so all three must run
/// the same fixtures. The count table declares them equal for exactly that reason.
fn configuration() -> &'static str {
    // `parquet` implies `arrow`, and this file is `#![cfg(feature = "parquet")]`, so the only axis left
    // is whether the CSV lane is in.
    if cfg!(feature = "csv") {
        "default"
    } else if cfg!(feature = "parquet") {
        "parquet-no-csv"
    } else {
        // Unreachable under the file's own cfg, but kept total: an undeclared configuration must fail
        // loudly rather than be guessed at, because guessing is the blind spot the guard closes.
        panic!(
            "this build enables an undeclared mix of generic-ingest lanes (arrow={}, parquet={}, \
             csv={}). Add it to `EXPECTED_COUNTS`, with its count.",
            cfg!(feature = "arrow"),
            cfg!(feature = "parquet"),
            cfg!(feature = "csv"),
        )
    }
}

/// **The anti-vacuity guard.** Runs before anything compares a hash.
#[test]
fn the_fixture_count_matches_the_declared_count_for_this_configuration() {
    let config = configuration();
    let expected = corpus::EXPECTED_COUNTS
        .iter()
        .find(|(name, _)| *name == config)
        .map(|(_, n)| *n)
        .unwrap_or_else(|| panic!("no declared fixture count for configuration '{config}'"));
    let actual = corpus::enabled_fixtures().len();
    assert_eq!(
        actual, expected,
        "configuration '{config}' ran {actual} ingest fixtures but declares {expected}. A corpus that \
         silently runs fewer fixtures reports the same green as one that runs them all — which is the \
         failure this assertion exists to make impossible."
    );

    // The committed file must declare the same counts as the code, or the guard could be satisfied by
    // editing one of the two.
    let c = committed();
    for (name, n) in corpus::EXPECTED_COUNTS {
        assert_eq!(
            c.expected_counts.get(*name),
            Some(n),
            "corpus/ingest-corpus.json disagrees with EXPECTED_COUNTS about '{name}'"
        );
    }
}

/// ADR-0057 §5's second half: at least one fixture per live hazard.
#[test]
fn every_live_hazard_has_a_fixture() {
    let covered: BTreeSet<&str> = corpus::fixtures()
        .iter()
        .flat_map(|f| f.hazards.iter().copied())
        .collect();
    for h in corpus::LIVE_HAZARDS {
        assert!(
            covered.contains(h),
            "ADR-0056 §5 hazard {h} has no corpus fixture — a gate that covers every hazard but one \
             is a gate with a named blind spot"
        );
    }
    // And nothing claims to cover a hazard that is not on the live list, which would be a fixture
    // quietly asserting coverage the ADR does not recognise.
    for h in &covered {
        assert!(
            corpus::LIVE_HAZARDS.contains(h),
            "a fixture claims hazard '{h}', which is not in LIVE_HAZARDS"
        );
    }
}

/// **Value preservation.** Every enabled fixture reproduces its committed hashes.
#[test]
fn ingest_corpus_hashes_match_goldens() {
    let dir = tempfile::tempdir().unwrap();
    let built = corpus::build(dir.path()).expect("build the ingest corpus");
    let c = committed();

    // Compare field-wise per fixture rather than whole-file, so a failure names what moved. ADR-0056
    // §6a's review ergonomics depend on this: "a moved content_hash is not a formality".
    for want in &c.fixtures {
        // A fixture the committed file knows but this build cannot run is skipped — but only because
        // the count guard above already proved the skip set is the declared one.
        let Some(got) = built.fixtures.iter().find(|g| g.name == want.name) else {
            assert!(
                !corpus::fixtures()
                    .iter()
                    .find(|f| f.name == want.name)
                    .is_some_and(corpus::enabled),
                "fixture '{}' is enabled in this build but produced no golden",
                want.name
            );
            continue;
        };
        assert_eq!(&got.id, &want.id, "fixture '{}': id moved", want.name);
        assert_eq!(
            &got.content_hash, &want.content_hash,
            "fixture '{}': content_hash MOVED. This is not a formality — a decoder extracted \
             different values than the committed run did. State which ADR-0056 §5 H-rule changed \
             behaviour and why the new values are correct, then regenerate.",
            want.name
        );
        assert_eq!(
            &got.manifest_hash, &want.manifest_hash,
            "fixture '{}': manifest_hash moved. Legitimate ONLY if `ingest_decoder` moved with it \
             (a recipe change is a seal change); otherwise something in the sealed manifest drifted.",
            want.name
        );
        assert_eq!(
            &got.ingest_decoder, &want.ingest_decoder,
            "fixture '{}': the sealed decoder triple moved",
            want.name
        );
        assert_eq!(
            &got.hazards, &want.hazards,
            "fixture '{}': hazard coverage changed",
            want.name
        );
    }
}

/// The properties the corpus exists to pin, asserted against each other rather than against a file —
/// so they hold even in a build where the goldens were just regenerated.
#[test]
fn the_dictionary_plain_and_chunked_fixtures_agree_with_each_other() {
    let dir = tempfile::tempdir().unwrap();
    let built = corpus::build(dir.path()).unwrap();
    let hash = |name: &str| {
        built
            .fixtures
            .iter()
            .find(|f| f.name == name)
            .map(|f| f.content_hash.clone())
            .unwrap_or_else(|| panic!("fixture '{name}' missing"))
    };
    // ADR-0056 §2: a dictionary is an ENCODING, so materialising it to values must give the same
    // payload as the plain column — otherwise the writer's dictionary order reaches the hash.
    assert_eq!(
        hash("ingest_parquet_dictionary"),
        hash("ingest_parquet_plain_strings"),
        "dictionary encoding reached the payload"
    );
    // Chunking is the reader's business, not the product's.
    assert_eq!(
        hash("ingest_parquet_tiny_row_groups"),
        hash("ingest_parquet_plain_strings"),
        "row-group size reached the payload"
    );
    // And the container is not the product.
    assert_eq!(
        hash("ingest_arrow_ipc_scalars"),
        hash("ingest_parquet_plain_strings"),
        "the container reached the payload"
    );
}

/// Every fixture seals a decoder triple. ADR-0056 §6a's sequencing is that no ingest fixture may enter
/// the corpus without one — "no artifact is ever sealed with an ambiguous or missing decoder identity,
/// because none is sealed at all until the home exists".
#[test]
fn every_fixture_seals_a_decoder_triple() {
    let dir = tempfile::tempdir().unwrap();
    let built = corpus::build(dir.path()).unwrap();
    assert!(
        !built.fixtures.is_empty(),
        "this configuration runs fixtures"
    );
    for f in &built.fixtures {
        let d = &f.ingest_decoder;
        assert!(
            d.is_object(),
            "fixture '{}' sealed no decoder record: {d}",
            f.name
        );
        assert!(
            d["name"].is_string(),
            "fixture '{}': no decoder name",
            f.name
        );
        assert!(
            d["features"]
                .as_str()
                .is_some_and(|s| s.starts_with("blake3:")),
            "fixture '{}': the feature component must be a digest, got {d}",
            f.name
        );
    }
}
