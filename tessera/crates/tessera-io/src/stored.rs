//! The stored form of a sealed JSON document: the bytes every writer puts on disk for
//! `manifest.json` and `collection.json` (ADR-0066 decision rule 1, implementation step 2).
//!
//! A sealed document is stored as exactly its RFC 8785 JCS serialisation: no whitespace, keys in
//! JCS order, JCS string escapes and number forms, UTF-8, one line. `jq . manifest.json` gives the
//! readable view. Until the A′ cutover (ADR-0066 step 4) the stored bytes still carry the in-band
//! `manifest_hash` key, and readers still accept any JSON spelling of a sealed object; this module
//! only fixes what tessera's own writers emit.
//!
//! **The seal does not move.** The seal is `digest(JCS(object − "manifest_hash"))` over the object
//! a reader parses (SPEC §4). Every function here re-encodes a *parsed* value with
//! [`tessera_core::canonical::to_bytes`] (the one JCS producer), and parsing JCS output gives back
//! the same object: a JCS number is the shortest form that round-trips its double, and a JCS key or
//! string unescapes to the same text. So the object a reader parses from these bytes is the object
//! parsed from the writer's previous output, key for key and number for number, and it seals to the
//! same `manifest_hash`. The corpus proof is `tests/stored_form.rs`.
//!
//! One path keeps the stored bytes instead: adding `aux/` members to an existing `.tsra`
//! ([`crate::add_aux_members`], used by signing and ingest provenance) copies `manifest.json`
//! byte-for-byte and refuses to change it, because aux members are outside the seal. A product
//! written before #912 therefore keeps its pretty manifest through `sign`; re-packing it
//! (`tessera unpack` + `tessera pack`) stores the JCS form under the same seal.
//!
//! I-JSON (RFC 7493, ADR-0066 rule 7) stays enforced at every door: the input is parsed with
//! [`tessera_core::canonical::parse_strict`] (duplicate keys, lone surrogates, integers beyond
//! ±(2^53−1)) and [`tessera_core::canonical::to_bytes`] checks the integers again.

use tessera_core::canonical;
use tessera_core::collection::Collection;
use tessera_core::manifest::Manifest;
use tessera_core::Result;

/// The stored bytes of a sealed manifest: the JCS form of [`Manifest::to_sealed_json`].
///
/// Used by [`crate::pack`], [`crate::pack_streaming`], [`crate::WriteSession`]'s seal and
/// [`crate::Repository::put_manifest`], so every manifest tessera writes from a typed value is
/// stored in this form.
///
/// # Errors
/// As [`Manifest::to_sealed_json`]: an unsealed manifest, a parsed view with newer fields, or a
/// manifest whose JSON doesn't reproduce its seal is refused.
/// [`tessera_core::Error::InexactInteger`] for an integer beyond ±(2^53−1).
pub fn manifest(m: &Manifest) -> Result<Vec<u8>> {
    canonical_form(&m.to_sealed_json()?)
}

/// The stored bytes of a sealed collection: the JCS form of [`Collection::to_sealed_json`].
///
/// Used by the `tessera collection` writer and the ingest engine's `collection.json`.
///
/// # Errors
/// As [`Collection::to_sealed_json`] and [`manifest`].
pub fn collection(c: &Collection) -> Result<Vec<u8>> {
    canonical_form(&c.to_sealed_json()?)
}

/// The stored form of sealed JSON text another writer produced: `JCS(parse_strict(json))`.
///
/// For the doors that copy a document instead of serialising a typed value (`pack_dir` re-packs an
/// exploded directory, `unpack` explodes a `.tsra`): the parsed object keeps every key it arrived
/// with, including newer-format fields this build doesn't model, so the seal over it is unchanged.
/// The caller verifies the seal first; this function only re-spells the bytes.
///
/// # Errors
/// As [`canonical::parse_strict`]: malformed JSON, a duplicate key, a lone surrogate, or an integer
/// beyond ±(2^53−1).
pub fn canonical_form(json: &str) -> Result<Vec<u8>> {
    canonical::to_bytes(&canonical::parse_strict(json)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_form_sorts_keys_drops_whitespace_and_uses_jcs_numbers() {
        let got = canonical_form("{ \"b\": 1.0, \"a\": [ 1e2, -0.0, \"\\u0041\" ] }").unwrap();
        assert_eq!(got, br#"{"a":[100,0,"A"],"b":1}"#);
    }

    #[test]
    fn canonical_form_is_idempotent() {
        let once = canonical_form("{\"z\": {\"y\": 0.1, \"x\": \"\\u00e9\"}}").unwrap();
        let twice = canonical_form(std::str::from_utf8(&once).unwrap()).unwrap();
        assert_eq!(once, twice);
    }

    #[test]
    fn canonical_form_keeps_i_json_at_the_door() {
        assert!(canonical_form(r#"{"a":1,"a":2}"#).is_err(), "duplicate key");
        assert!(
            canonical_form(r#"{"n":9007199254740993}"#).is_err(),
            "inexact integer"
        );
    }
}
