//! Execute parallel backend work in a private, process-wide budgeted pool.
//!
//! A library cannot configure Rayon's global pool: the embedding application may
//! already own it. `install` scopes nested rayon work to this pool instead, shared
//! by array codecs and blob hashing. Pool creation failure is a typed I/O error.

use std::sync::OnceLock;
use tessera_core::{Error, Result};

pub(crate) fn install<T: Send>(work: impl FnOnce() -> Result<T> + Send) -> Result<T> {
    static POOL: OnceLock<std::result::Result<rayon::ThreadPool, String>> = OnceLock::new();
    let pool = POOL.get_or_init(|| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(tessera_core::threads::budget())
            .thread_name(|n| format!("tessera-codec-{n}"))
            .build()
            .map_err(|e| e.to_string())
    });
    match pool {
        Ok(pool) => pool.install(|| {
            #[cfg(test)]
            OBSERVED_POOL_SIZE.fetch_max(
                rayon::current_num_threads(),
                std::sync::atomic::Ordering::SeqCst,
            );
            work()
        }),
        Err(message) => Err(Error::Io(std::io::Error::other(format!(
            "budgeted worker pool: {message}"
        )))),
    }
}

#[cfg(test)]
pub(crate) static OBSERVED_POOL_SIZE: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);
