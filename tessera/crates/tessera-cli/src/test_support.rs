//! Shared test helpers for the CLI's unit tests. Test-only: compiled under `cfg(test)`.

use tessera_core::canonical;

/// `bytes`, a sealed document a door wrote, are exactly their own RFC 8785 JCS form (ADR-0066
/// rule 1, #912): `bytes == JCS(parse(bytes))`.
#[track_caller]
pub(crate) fn assert_stored_form(what: &str, bytes: &[u8]) {
    let text = std::str::from_utf8(bytes).unwrap_or_else(|e| panic!("{what}: not UTF-8: {e}"));
    let jcs = canonical::to_bytes(&canonical::parse_strict(text).unwrap()).unwrap();
    assert!(
        bytes == jcs.as_slice(),
        "{what}: the written document is not its own JCS form (first difference at byte {:?})",
        bytes.iter().zip(&jcs).position(|(a, b)| a != b)
    );
}
