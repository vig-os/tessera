//! Build script — two unrelated jobs, both purely about the *build*, neither in the sealed
//! byte-path: (1) say whether a workspace lockfile sits next to this crate, and (2) the
//! `static-hdf5` lib64 link-search fallback.
//!
//! ## 1. `tessera_workspace_lock` — and why this script no longer reads the lockfile (#979)
//!
//! The decoder versions and decode-path pins that ADR-0056 §6a seals as `ingest_decoder` used to be
//! read out of `Cargo.lock` here and re-emitted as `rustc-env` values. That was wrong in a way no
//! `rerun-if-changed` trigger can repair: cargo never keys a build-script **run** on the package's
//! normal dependencies, and it leaves a workspace member's path out of the unit hash, so two checkouts
//! sharing one `CARGO_TARGET_DIR` share this script's output. The second reused the first's pins, and
//! a binary linking parquet `58.4.1` sealed `=58.4.2`. Freshness triggers are mtime-based and cannot
//! tell two checkouts apart.
//!
//! So the lockfile is now read by the **compiler**, in the library unit (`src/linked.rs`), whose unit
//! hash cargo does key on every linked crate's version, source and features. All this script decides is
//! whether there is a workspace lockfile to read: `CARGO_MANIFEST_DIR/../../Cargo.lock`, the workspace
//! root this crate is a member of. That is a fact about the repository layout, the same for every
//! checkout of it, so sharing this script's output between checkouts cannot make it wrong. A packaged
//! build (crates.io, `cargo package`) has no such file; the cfg stays off and the sealed triple carries
//! the decoder's name only — absent rather than guessed. Before #979 this script walked up from the
//! manifest and found the packaged crate's bundled `Cargo.lock`, so packaged builds sealed versions too.
//! The crate is not published; supporting that layout is tracked in #989.
//!
//! ## 2. The `static-hdf5` lib64 fallback
//!
//! When we link a *vendored* libhdf5 (feature `static-hdf5` → `hdf5-metno-sys/static`, which builds
//! HDF5 from source via `hdf5-metno-src`), `hdf5-metno-sys`'s build script emits its link-search path
//! as a hard-coded `{install_prefix}/lib`. But CMake's `GNUInstallDirs` installs the static archive
//! into `lib64` instead of `lib` on "lib64" distros (Fedora / RHEL / SUSE / nix, anything where
//! `CMAKE_INSTALL_LIBDIR` resolves to `lib64`), so the archive is at `{prefix}/lib64/libhdf5.a` and
//! the linker — told to search only `{prefix}/lib` — fails with
//! `could not find native static library 'hdf5'`. Debian / Ubuntu / macOS use `lib`, so the prebuilt
//! release binaries (built on those runners) are unaffected; this only bites a source
//! `cargo install --features static-hdf5` on a lib64 platform.
//!
//! `hdf5-metno-sys` declares `links = "hdf5"` and re-emits the install prefix as `metadata=root`, so we
//! (a direct dependent) receive it as `DEP_HDF5_ROOT`. We add `{root}/lib64` as an *extra* link-search
//! path. It is purely additive — the linker searches every `-L` path, so adding the lib64 sibling next
//! to hdf5-metno-sys's `lib` makes the archive findable under either layout, on every platform.
//!
//! Determinism note: this touches only where the *input* libhdf5 is found at link time — HDF5 is
//! read-only input (Tessera encodes to Vortex/pcodec), never in the sealed byte-path, so nothing here
//! can move a content_hash.

use std::path::Path;

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-env-changed=DEP_HDF5_ROOT");
    println!("cargo::rustc-check-cfg=cfg(tessera_workspace_lock)");

    // The path `src/linked.rs` `include_str!`s, relative to this crate's manifest. Deliberately not
    // `rerun-if-changed` on it: the lockfile's *content* is tracked by the compiler through
    // `include_str!`, in the unit cargo keys correctly; this script only answers whether it exists.
    let manifest_dir = std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or_default();
    if Path::new(&manifest_dir).join("../../Cargo.lock").is_file() {
        println!("cargo::rustc-cfg=tessera_workspace_lock");
    }

    // Only relevant for the vendored-static build; a no-op for the default pkg-config path (where
    // hdf5-metno-sys does not emit `root`, so DEP_HDF5_ROOT is unset).
    if std::env::var_os("CARGO_FEATURE_STATIC_HDF5").is_none() {
        return;
    }
    if let Ok(root) = std::env::var("DEP_HDF5_ROOT") {
        // Additive fallback next to hdf5-metno-sys's own `{root}/lib`; covers the lib64 install layout.
        println!("cargo::rustc-link-search=native={root}/lib64");
    }
}
