//! SPEC §6's canonical member set, held by the Rust reader over the same committed fixtures the
//! reference reader refuses (`corpus/refusals`, written by
//! `corpus/reference_reader/make_refusal_fixtures.py`; `run.py` checks them too).
//!
//! Each fixture is `recon_int16.tsra` with one fault in the zip layer: a repeated `manifest.seal`
//! or `manifest.json` whose last entry is the product's own, or that member DEFLATED. A reader that
//! resolves a repeated name to its last entry, or inflates a compressed member, verifies every one
//! of them; each is refused here, and none of the verifying doors gets as far as the seal.

use std::path::{Path, PathBuf};

use tessera_core::Error;
use tessera_io::Reader;

fn refusals() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/refusals")
}

/// The fixture and what its refusal must say. `zip` (≥ 9.0.5, with the header-consistency check
/// every member read uses) refuses any read from a central directory that names a member twice,
/// without naming it.
const FIXTURES: [(&str, &str); 4] = [
    (
        "duplicate-manifest-seal.tsra",
        "duplicate file names in central directory",
    ),
    (
        "duplicate-manifest-json.tsra",
        "duplicate file names in central directory",
    ),
    ("deflated-manifest-seal.tsra", "'manifest.seal' is Deflated"),
    ("deflated-manifest-json.tsra", "'manifest.json' is Deflated"),
];

#[test]
fn the_fixtures_are_exactly_the_ones_checked() {
    let mut found: Vec<String> = std::fs::read_dir(refusals())
        .expect("corpus/refusals present")
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    found.sort_unstable();
    let mut want: Vec<&str> = FIXTURES.iter().map(|(f, _)| *f).collect();
    want.sort_unstable();
    assert_eq!(found, want);
}

#[test]
fn an_archive_outside_the_canonical_member_set_is_refused() {
    for (fixture, needle) in FIXTURES {
        match Reader::open(&refusals().join(fixture)) {
            Err(Error::Container(msg)) => {
                assert!(msg.contains(needle), "{fixture}: {needle:?} in {msg}")
            }
            Err(other) => panic!("{fixture}: expected a container refusal, got {other}"),
            Ok(_) => panic!("{fixture}: opened"),
        }
    }
}
