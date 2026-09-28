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

/// The crates whose pinned version goes into the ADR-0056 §6a **feature digest** pre-image — i.e.
/// everything on the *decode* path of the generic-ingest lanes.
///
/// Deliberately not "every dependency": the digest answers "which decoder interpreted these bytes",
/// and widening it to the whole graph would make the digest move on changes that provably cannot
/// touch a decoded value, turning every `cargo update` into a recipe event for no information gain.
const DECODE_PATH_CRATES: &[&str] = &[
    "parquet",
    "arrow-array",
    "arrow-buffer",
    "arrow-schema",
    "arrow-ipc",
    "csv",
];

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-env-changed=DEP_HDF5_ROOT");

    emit_decoder_pins();
    emit_decode_feature_preimage();

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
    for (package, var) in DECODERS {
        if let Some(version) = lock_version(&lock, package) {
            println!("cargo::rustc-env={var}={version}");
        }
    }
}

/// Emit the ADR-0056 §6a decoder-digest **pre-image**: the `=`-pinned version of every crate on the
/// generic-ingest decode path.
///
/// # What is deliberately NOT in here, and why
///
/// Two things were in an earlier version of this and were removed, because each made `manifest_hash`
/// move for a reason that is not a difference in how the file was interpreted:
///
/// - **This crate's resolved `CARGO_FEATURE_*` set.** It looked like the obvious reading of "resolved
///   decode-relevant features", and it is the wrong one. Whether the *CSV* lane was compiled in has
///   nothing to do with how a *Parquet* file was read — a lane that is off did not touch the bytes.
///   Including it meant two builds of one version sealed different `manifest_hash`es for an identical
///   input, which makes the format's own version identity a function of how the reader's binary was
///   compiled. Caught by the `parquet-no-csv` corpus configuration, which is exactly what ADR-0057 §5's
///   declared-count table exists to make someone look at.
/// - **This crate's own `CARGO_PKG_VERSION`.** Tempting, because ADR-0056 leaves open that
///   "`tessera-ingest`'s own canonicalisation code is attributed by nothing". But the workspace version
///   moves on every release, so sealing it would make every release a conformance-corpus regeneration —
///   re-introducing precisely the churn ADR-0052 §1 / #336 removed by stamping `TESSERA_VERSION` (the
///   *format* version) rather than the software version. The gap stays open rather than closed at that
///   price.
///
/// What remains moves when, and only when, a decoder is bumped — which is what makes a decoder bump a
/// recipe change and nothing else one.
fn emit_decode_feature_preimage() {
    let mut pins: Vec<String> = Vec::new();
    if let Some(lock_path) = find_lockfile() {
        if let Ok(lock) = std::fs::read_to_string(&lock_path) {
            for c in DECODE_PATH_CRATES {
                if let Some(v) = lock_version(&lock, c) {
                    pins.push(format!("{c}={v}"));
                }
            }
        }
    }
    // Sorted by construction (`DECODE_PATH_CRATES` is walked in its declared order, which is fixed), so
    // the pre-image depends only on the resolved versions — never on filesystem or env iteration order.
    println!(
        "cargo::rustc-env=TESSERA_INGEST_DECODE_PINS=pins={}",
        pins.join(",")
    );
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

/// The `version` of one `[[package]]` entry in a `Cargo.lock`.
///
/// Hand-scanned rather than parsed with `toml`, to keep this build script dependency-free: adding a
/// `[build-dependencies]` entry would move the resolved feature graph, which is a Gate B event
/// (ADR-0057 §5) — a disproportionate price for reading one string. The lockfile's shape is fixed by
/// cargo (`[[package]]`, then `name = "…"`, then `version = "…"`, one key per line, in that order)
/// and contains no nested tables, so a scanner cannot mis-parse it.
fn lock_version(lock: &str, package: &str) -> Option<String> {
    let mut in_wanted_package = false;
    for line in lock.lines() {
        let line = line.trim();
        if line == "[[package]]" {
            in_wanted_package = false;
        } else if let Some(name) = line.strip_prefix("name = ") {
            in_wanted_package = name.trim_matches('"') == package;
        } else if in_wanted_package {
            if let Some(version) = line.strip_prefix("version = ") {
                return Some(version.trim_matches('"').to_owned());
            }
        }
    }
    None
}
