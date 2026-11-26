//! Classification of stored version objects by their top-level keys (ADR-0067 D1.1).

use crate::{Error, Result};

/// The kind declared by a version object's bytes, independently of repository ref records.
///
/// This is distinct from [`crate::collection::MemberKind`], which is a member's claim
/// about its target. Readers compare that claim with this classification before dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ObjectKind {
    /// A product manifest, with `product` and without `members`.
    Product,
    /// A collection manifest, with `members` and without `product`.
    Collection,
}

impl ObjectKind {
    /// Classify JSON bytes using top-level key presence, without I/O or parser guessing.
    ///
    /// Requires a JSON object containing `tessera_version` and exactly one of
    /// `product` or `members`. Values are not interpreted: the selected kind's reader
    /// must still check the version, structure, canonical bytes and seal. Empty
    /// collections and zero-block products therefore classify unambiguously.
    ///
    /// # Errors
    ///
    /// Returns [`Error::AmbiguousObjectKind`] for both keys, neither key, a missing
    /// version key or non-object JSON. The shared strict JSON parser refuses malformed
    /// JSON, invalid UTF-8 and duplicate keys as [`Error::Serde`], and enforces its
    /// documented I-JSON limits (see [`crate::canonical::parse_strict`]). Classification
    /// itself does not require canonical whitespace or key ordering.
    pub fn of(bytes: &[u8]) -> Result<Self> {
        let value = crate::canonical::parse_strict_bytes(bytes)?;
        let object = value.as_object().ok_or(Error::AmbiguousObjectKind)?;
        if !object.contains_key("tessera_version") {
            return Err(Error::AmbiguousObjectKind);
        }
        match (
            object.contains_key("product"),
            object.contains_key("members"),
        ) {
            (true, false) => Ok(Self::Product),
            (false, true) => Ok(Self::Collection),
            _ => Err(Error::AmbiguousObjectKind),
        }
    }
}
