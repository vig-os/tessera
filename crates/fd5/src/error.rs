/// Errors produced by the fd5 crate.
#[derive(Debug, thiserror::Error)]
pub enum Fd5Error {
    #[error("HDF5 error: {0}")]
    Hdf5(#[from] hdf5_metno::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("missing attribute: {0}")]
    MissingAttribute(String),

    #[error("hash mismatch: stored={stored}, computed={computed}")]
    HashMismatch { stored: String, computed: String },

    #[error("not an fd5 file (no content_hash attribute)")]
    NotFd5,

    #[error("{0}")]
    Other(String),
}

pub type Fd5Result<T> = std::result::Result<T, Fd5Error>;
