//! fd5 integrity verification.
//!
//! Recomputes the Merkle tree and compares with the stored `content_hash`.

use std::path::Path;

use hdf5_metno::File;

use crate::error::{Fd5Error, Fd5Result};
use crate::hash::compute_content_hash;

/// Verification status of an fd5 file.
#[derive(Debug, Clone)]
pub enum Fd5Status {
    /// Currently checking (used for UI state).
    Checking,
    /// Hash verified successfully.
    Valid(String),
    /// Hash mismatch.
    Invalid { stored: String, computed: String },
    /// Not an fd5 file (no content_hash attribute).
    NotFd5,
    /// Error during verification.
    Error(String),
}

/// Recompute the Merkle tree and compare with the stored `content_hash`.
///
/// Returns `true` if the hashes match, `false` otherwise (including
/// when `content_hash` is missing).
///
/// Direct equivalent of Python's `verify(path)`.
pub fn verify(path: &Path) -> Fd5Result<Fd5Status> {
    let file = File::open(path)?;
    verify_file(&file)
}

/// Verify an already-opened file.
pub fn verify_file(file: &File) -> Fd5Result<Fd5Status> {
    let group = file.as_group()?;

    // Read stored content_hash
    let stored = match group.attr("content_hash") {
        Ok(attr) => {
            let val: String = attr
                .read_scalar::<hdf5_metno::types::VarLenUnicode>()
                .map(|v| v.as_str().to_string())
                .or_else(|_| {
                    attr.read_scalar::<hdf5_metno::types::VarLenAscii>()
                        .map(|v| v.as_str().to_string())
                })
                .map_err(|e| Fd5Error::Other(format!("Failed to read content_hash: {e}")))?;
            val
        }
        Err(_) => return Ok(Fd5Status::NotFd5),
    };

    // Compute fresh hash
    let computed = compute_content_hash(file)?;

    if computed == stored {
        Ok(Fd5Status::Valid(stored))
    } else {
        Ok(Fd5Status::Invalid { stored, computed })
    }
}
