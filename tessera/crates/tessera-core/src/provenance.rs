//! Provenance as a DAG — each [`Source`] is a typed edge to an upstream artifact.
//!
//! e.g. a `recon` product has a `Source { role: "ingested_from", reference: "<DICOM path>",
//! content_hash: Some(...) }`; a lifetime `spectrum` has a `Source` to the `listmode` product
//! it was histogrammed from. This is fd5's `sources/` model, carried into the manifest.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Source {
    /// Typed role of this edge, e.g. "ingested_from", "emission_data", "calibration".
    pub role: String,
    /// Identifier or path of the upstream artifact.
    pub reference: String,
    /// Content hash of the upstream artifact, when known (closes the integrity chain).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
}

impl Source {
    pub fn new(role: impl Into<String>, reference: impl Into<String>) -> Self {
        Source { role: role.into(), reference: reference.into(), content_hash: None }
    }
}
