//! Build script — two unrelated jobs, both purely about the *build*, neither in the sealed
//! byte-path: (1) capture the resolved decoder-crate pins for `tessera info`, and (2) the
//! `static-hdf5` lib64 link-search fallback.
//!
//! ## 1. Decoder pins for `tessera info` (ADR-0057 §7)
//!
//! Cargo gives a crate its *own* version as `CARGO_PKG_VERSION` but exposes nothing about its
//! dependencies' resolved versions, so the pins are read out of the workspace `Cargo.lock` — the
//! same file `--locked` builds resolve against — and re-emitted as `rustc-env` vars that
//! `option_env!` picks up in `backends::backend_version`. This is the read side of the fact
//! ADR-0056 §6.2 seals as `ingest_decoder`.
//!
//! Failure is silent **by design**: built as a crates.io dependency there is no workspace lockfile,
//! `option_env!` yields `None`, and `tessera info` prints the backend name without a version. A
//! missing diagnostic must never fail a build.
//!
//! Decoders whose version is better answered at runtime are deliberately not here: `hdf-compound`
//! reports the libhdf5 actually linked, which differs between a system libhdf5 and the `static-hdf5`
//! vendored build — a compile-time pin could not tell them apart.
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

use std::path::{Path, PathBuf};

// The decode-path classification, shared verbatim with the library and the gate test so the digest and
// the gate can never disagree about what the decode path is. `include!` rather than a module because a
// build script is its own crate root and cannot `use` the crate it builds.
include!("src/decode_path.rs");

/// `(Cargo.lock package name, env var read via `option_env!`)`.
///
/// Read by `backends::backend_version` (for `tessera info`) and — for the generic-ingest lane — by
/// `decoder::Decoder`, which seals the pin as part of the ADR-0056 §6a triple.
const DECODERS: &[(&str, &str)] = &[
    ("dicom", "TESSERA_DEP_DICOM"),
    // The generic-ingest table lane (ADR-0056/#386). These are `=`-pinned in the workspace manifest,
    // so the lockfile value IS the pin.
    ("parquet", "TESSERA_DEP_PARQUET"),
    ("arrow-ipc", "TESSERA_DEP_ARROW_IPC"),
    ("arrow-array", "TESSERA_DEP_ARROW_ARRAY"),
    ("csv", "TESSERA_DEP_CSV"),
];

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=src/decode_path.rs");
    println!("cargo::rerun-if-changed=Cargo.toml");
    println!("cargo::rerun-if-env-changed=DEP_HDF5_ROOT");

    emit_decoder_pins();
    emit_decode_feature_preimages();

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

/// Re-emit each [`DECODERS`] pin from the workspace lockfile as a `rustc-env` var. Silent no-op when
/// there is no lockfile to read (see the module docs).
fn emit_decoder_pins() {
    let Some(lock_path) = find_lockfile() else {
        return;
    };
    println!("cargo::rerun-if-changed={}", lock_path.display());
    let Ok(lock) = std::fs::read_to_string(&lock_path) else {
        return;
    };
    let pkgs = parse_lock(&lock);
    for (package, var) in DECODERS {
        // The reader's own version, for `tessera info`. Unambiguous by name here: these are the crates
        // whose single resolved version IS the `=` pin in the workspace manifest.
        if let Some(p) = resolve(&pkgs, package, None) {
            println!("cargo::rustc-env={var}={}", p.version);
        }
    }
}

/// Emit one ADR-0056 §6a decoder-digest **pre-image per ingest lane** (#477).
///
/// Per lane, because the digest answers "which decoder interpreted *these* bytes": a parquet product has
/// no business committing to the zip library the `.npz` lane gained, and before this it did — adding that
/// dependency moved the `manifest_hash` of every parquet and csv product in the corpus.
///
/// The lanes and their root crates come from `[features]`, the candidate closure from `Cargo.lock`, and
/// the membership from [`IN_DIGEST`] — see `decode_path` for why membership is declared rather than
/// derived, and for the measurements behind that.
///
/// # What is deliberately NOT in here, and why
///
/// Two things were in earlier versions of this digest and were removed, because each made `manifest_hash`
/// move for a reason that is not a difference in how the file was interpreted:
///
/// - **This crate's resolved `CARGO_FEATURE_*` set.** Whether the *CSV* lane was compiled in has nothing
///   to do with how a *Parquet* file was read — a lane that is off did not touch the bytes. Including it
///   made the format's own version identity a function of how the reader's binary was compiled.
/// - **This crate's own `CARGO_PKG_VERSION`.** The workspace version moves on every release, so sealing
///   it would make every release a conformance-corpus regeneration — re-introducing exactly the churn
///   ADR-0052 §1 / #336 removed by stamping the *format* version rather than the software version.
///
/// Both remain out. What is now *in*, and was wrongly out, is the rest of each lane's decode path: the
/// Thrift and FlatBuffers readers that parse Parquet's and Arrow IPC's own metadata, and every page codec.
fn emit_decode_feature_preimages() {
    // No lockfile (a crates.io build) means no pins can be read — and then every such build would seal an
    // identical digest over an empty pre-image, a false claim of sameness between builds that may have
    // resolved completely different decoders. Emitting nothing is the honest answer, and the same choice
    // `version` already makes (absent rather than guessed).
    let Some(lock_path) = find_lockfile() else {
        return;
    };
    let Ok(lock) = std::fs::read_to_string(&lock_path) else {
        return;
    };
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let Ok(cargo_toml) = std::fs::read_to_string(Path::new(&manifest_dir).join("Cargo.toml"))
    else {
        return;
    };
    for (lane, roots) in lane_roots(&cargo_toml) {
        if let Some(preimage) = lane_preimage(&lock, &roots) {
            println!(
                "cargo::rustc-env=TESSERA_DECODE_PINS_{}={preimage}",
                lane.to_uppercase().replace('-', "_")
            );
        }
    }
}

/// Walk up from this crate's manifest to the nearest `Cargo.lock` (the workspace root).
fn find_lockfile() -> Option<PathBuf> {
    let manifest_dir = std::env::var_os("CARGO_MANIFEST_DIR")?;
    let mut dir: Option<&Path> = Some(Path::new(&manifest_dir));
    while let Some(d) = dir {
        let candidate = d.join("Cargo.lock");
        if candidate.is_file() {
            return Some(candidate);
        }
        dir = d.parent();
    }
    None
}
