//! Error type for the Tessera core.
//!
//! `#[non_exhaustive]` so adding a variant is not a breaking change, and the core **never
//! panics** on bad input — every fallible path returns a typed [`Error`]. Integrity failures
//! name the field plus the expected and actual values so a caller (or auditor) can see exactly
//! what diverged.

use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("serialize: {0}")]
    Serde(#[from] serde_json::Error),

    /// Canonical (RFC 8785 JCS) serialization failed — the manifest could not be reduced to
    /// its deterministic byte form for hashing.
    #[error("canonicalization: {0}")]
    Canonicalization(String),

    /// A recomputed hash did not match the one recorded in the manifest — tampering, corruption,
    /// or a producer bug. `what` names the field (`id` / `content_hash` / `manifest_hash`).
    #[error("integrity: {what} mismatch — expected {expected}, got {actual}")]
    Integrity {
        what: &'static str,
        expected: String,
        actual: String,
    },

    /// The manifest's spec version is unparseable or newer than this reader supports.
    #[error("unsupported version: {0}")]
    UnsupportedVersion(String),

    /// A block reference is missing its content digest (cannot be sealed or verified).
    #[error("block '{0}' has no digest")]
    MissingDigest(String),

    /// A `.tsra` container (zip) could not be read or written, or is malformed (bad magic,
    /// missing manifest, truncated central directory).
    #[error("container: {0}")]
    Container(String),

    /// A storage-block codec (array pcodec/zarr, table Vortex) failed to encode or decode a
    /// payload, or was asked for an unsupported dtype/codec combination.
    #[error("codec: {0}")]
    Codec(String),

    /// A code path not implemented yet (e.g. a storage backend behind a feature flag).
    #[error("not yet implemented: {0}")]
    Unimplemented(&'static str),

    /// Mutating a product after it has been sealed (products are immutable).
    #[error("product is sealed and immutable")]
    Sealed,

    /// Structurally invalid product (bad dtype, rank mismatch, missing required field, ...).
    #[error("invalid product: {0}")]
    Invalid(String),
}
