//! The dependency resolution this crate was **compiled against** — the source of every decoder version
//! and every decode-path pin that [`crate::decoder`] seals and `tessera info` prints (ADR-0056 §6a, #979).
//!
//! # Why this is read by the compiler, not by `build.rs`
//!
//! Until #979 `build.rs` read the workspace `Cargo.lock` and re-emitted the pins as `rustc-env` values.
//! That gave the right answer only when the build script happened to re-run, and cargo cannot know when it
//! must: it never keys a build-script **run** on the package's normal dependencies (only on the script's
//! own build-dependencies and on `links` dependencies), and it leaves a workspace member's path out of the
//! unit hash. So two checkouts sharing one `CARGO_TARGET_DIR` share one run, the second reuses the first's
//! output, and a binary linking parquet `58.4.1` sealed `=58.4.2`. Any `rerun-if-changed` trigger is
//! mtime-based and cannot tell two checkouts apart, so no trigger fixes that.
//!
//! The **library** compile unit is the one cargo does key on what is linked: its unit hash covers the
//! version, source and resolved features of every crate in its dependency graph, so a different decoder is
//! a different unit and is compiled afresh. This module therefore takes the lockfile with `include_str!`
//! *in that unit*, with the path built from this compile's own `CARGO_MANIFEST_DIR` — the checkout being
//! compiled, never one remembered from an earlier run. The lockfile is the resolution cargo is building
//! this unit from, so every version of a linked crate read out of it is the version linked. No option
//! that reads the linked crates themselves exists for most of them: of the lane roots, only parquet
//! exports its version (`DEFAULT_CREATED_BY`), and `tests/linked_decoder_identity.rs` holds the two to
//! each other.
//!
//! # Limits, stated
//!
//! - **The candidate set is wider than what is linked.** A lane's pins are the lockfile closure of its
//!   root crates (see [`crate::decode_path`]), which can include a crate this build does not link — one
//!   reached only on another target, or only under a feature no crate in this build enables. For such a
//!   crate the pin is what the lockfile said when this unit was compiled. It can go stale only when two
//!   checkouts lock identical linked graphs and differ in that unlinked crate alone, and then the pin
//!   describes code that did not run in either binary.
//! - **This crate's own sources are still told apart by mtime.** The lane definitions (`Cargo.toml`
//!   `[features]`) and the digest membership (`decode_path.rs`) are this crate's source, and cargo checks
//!   a path package's sources by mtime, so two checkouts sharing a target dir that link identical graphs
//!   and differ only in those files can reuse each other's library unit. That is cargo's limit for every
//!   line of every workspace crate, not something specific to the seal, and it is why each agent gets its
//!   own target slot.
//! - **Built as a git or path dependency of another workspace, the lockfile read is ours, not theirs.**
//!   `../../Cargo.lock` is then this repository's lockfile, while cargo resolved the consumer's, so a
//!   pin can name a version the consumer did not link. Unchanged by #979 (the build script read the same
//!   file); tracked in #989.
//! - **Size.** The lockfile (~210 KiB) and this crate's `Cargo.toml` are embedded in every binary that
//!   links this crate, and parsed once, on first use.
//! - **No workspace, no record.** Built as a packaged crate (crates.io, `cargo package`), there is no
//!   workspace lockfile next to the manifest; `build.rs` then leaves the `tessera_workspace_lock` cfg off,
//!   every lookup here is `None`, and the sealed triple carries only the decoder's name — absent rather
//!   than guessed. Before #979 the build script also found a packaged crate's bundled `Cargo.lock`
//!   by walking up from its manifest, so packaged builds did seal versions. The crate is not published;
//!   supporting the packaged layout is tracked in #989.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use crate::decode_path::{lane_preimage_in, lane_roots, parse_lock, resolve, Pkg};

/// The workspace lockfile and this crate's manifest, as compiled into this unit.
#[cfg(tessera_workspace_lock)]
const SOURCES: Option<(&str, &str)> = Some((
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.lock")),
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")),
));
#[cfg(not(tessera_workspace_lock))]
const SOURCES: Option<(&str, &str)> = None;

/// The parsed resolution: every locked package, and each lane's pre-image.
struct Resolution {
    pkgs: Vec<Pkg>,
    preimages: BTreeMap<String, String>,
}

static RESOLUTION: LazyLock<Option<Resolution>> = LazyLock::new(|| {
    let (lock, cargo_toml) = SOURCES?;
    let pkgs = parse_lock(lock);
    let preimages = lane_roots(cargo_toml)
        .into_iter()
        .filter_map(|(lane, roots)| Some((lane, lane_preimage_in(&pkgs, &roots)?)))
        .collect();
    Some(Resolution { pkgs, preimages })
});

/// The version `krate` resolved to in this build, or `None` with no workspace lockfile — or when the name
/// is ambiguous (two versions locked), which [`resolve`] refuses to guess.
pub(crate) fn version_of(krate: &str) -> Option<&'static str> {
    let r = RESOLUTION.as_ref()?;
    resolve(&r.pkgs, krate, None).map(|p| p.version.as_str())
}

/// The `v2;pins=…` pre-image of the ingest lane named by its Cargo feature (`parquet`, `arrow`, `csv`,
/// `npz`), or `None` with no workspace lockfile or for a lane with no third-party decode path.
pub(crate) fn preimage_of(lane: &str) -> Option<&'static str> {
    RESOLUTION.as_ref()?.preimages.get(lane).map(String::as_str)
}
