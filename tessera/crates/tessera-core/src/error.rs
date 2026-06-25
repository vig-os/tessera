//! Error type for the Tessera core.

use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("serialize: {0}")]
    Serde(#[from] serde_json::Error),

    /// A code path that the spike has not implemented yet (e.g. a storage backend).
    #[error("not yet implemented: {0}")]
    Unimplemented(&'static str),

    /// Mutating a product after it has been sealed (products are immutable).
    #[error("product is sealed and immutable")]
    Sealed,

    #[error("invalid product: {0}")]
    Invalid(String),
}
