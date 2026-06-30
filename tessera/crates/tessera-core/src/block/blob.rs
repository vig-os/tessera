//! Blob block — opaque bytes preserved **verbatim** (the "junk" preservation tier, ADR-0038).
//!
//! For the long tail of commercial-scanner output the engine does not (yet) parse: Siemens `.l64`
//! listmode (multi-GB opaque binary), GE `.7z`/`.cal` dumps, vendor PDFs/logs. The block's payload **is**
//! the file bytes; its digest is `blake3(bytes)`, so a blob rides the same hash-on-write → Merkle
//! `content_hash` → `manifest_hash` seal → signature machinery as every other block, and `tessera verify`
//! re-hashes it to confirm bit-faithfulness.
//!
//! A blob is **F**indable, **A**ccessible, **R**eusable-as-bytes, and integrity-verified — but **not
//! Interoperable** until a decoder exists. It is therefore a *preservation companion* to the normalised
//! array/table products, never a replacement: capture the truth off the scanner now, decode later — the
//! raw blob and a derived normalised product can coexist in one sealed `.tsra`, joined by a provenance
//! edge.

use serde::{Deserialize, Serialize};

/// Self-describing descriptor for an opaque [`crate::block::BlockKind::Blob`] block. The content digest
/// (`blake3` of the bytes) lives on the [`crate::block::BlockRef`] like every block — it is **not**
/// duplicated here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlobSpec {
    /// Original basename of the source file — provenance + the default `tessera extract` filename.
    pub filename: String,
    /// IANA media type if known (e.g. `application/pdf`). Absent ⇒ treated as opaque
    /// `application/octet-stream` on display.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    /// Size of the stored bytes. Redundant with the payload length, recorded so a manifest-only reader
    /// (no payload fetch) knows the size.
    pub size: u64,
}

impl BlobSpec {
    /// A blob descriptor with no media type.
    pub fn new(filename: impl Into<String>, size: u64) -> Self {
        Self {
            filename: filename.into(),
            media_type: None,
            size,
        }
    }

    /// Attach an IANA media type.
    pub fn with_media_type(mut self, media_type: impl Into<String>) -> Self {
        self.media_type = Some(media_type.into());
        self
    }

    /// The media type for display, defaulting to `application/octet-stream` when unknown.
    pub fn media_type_or_octet(&self) -> &str {
        self.media_type
            .as_deref()
            .unwrap_or("application/octet-stream")
    }
}
