//! # fd5
//!
//! Rust implementation of fd5 Merkle-tree hashing, verification, editing,
//! and file creation for immutable HDF5 data products sealed with `content_hash`.

pub mod attr_ser;
pub mod builder;
pub mod edit;
pub mod error;
pub mod h5io;
pub mod hash;
pub mod naming;
pub mod product;
pub mod schema;
pub mod verify;

pub use builder::{create, Fd5Builder, HashTrackingGroup};
pub use error::{Fd5Error, Fd5Result};
pub use hash::{compute_content_hash, compute_id};
pub use product::{get_schema, register_schema, ProductSchema};
pub use verify::{Fd5Status, verify};
