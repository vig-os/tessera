//! **#979: the sealed `ingest_decoder` describes the decoders this build links.**
//!
//! ADR-0056 §6a seals `generation.config.ingest_decoder {name, version, features}` as a statement about
//! which decoder interpreted a product's bytes. Until #979 the `version` and the `features` pre-image were
//! `rustc-env` values emitted by `build.rs` from the lockfile text it read. Cargo never keys a build-script
//! *run* on the package's normal dependencies, and it leaves a workspace member's path out of the unit
//! hash, so two checkouts sharing one `CARGO_TARGET_DIR` shared that run: the second checkout reused the
//! first checkout's values: a binary linking `zip 9.1.0` sealed `zip=9.0.5`, and #979's own reproduction
//! sealed parquet `58.4.1` as `58.4.2`.
//!
//! This test is the independent statement of the invariant. It reads **this checkout's** `Cargo.lock` at
//! test run time — the file cargo resolved the build being tested against — and requires every sealed
//! component and every `tessera info` version to agree with it. It also checks the one decoder that can
//! attest to its own version: the linked parquet's `DEFAULT_CREATED_BY`, a `const` compiled into parquet
//! itself, which no lockfile can mislabel.
//!
//! On a clean target dir every build passes this, before and after #979. Its teeth are the shared-target
//! scenario: build it in one checkout, then run it from a second checkout that locks a different decoder
//! version into the same target dir. Before #979 the second run failed here; see the PR for the transcript.

use tessera_ingest::backends::backend_version;
use tessera_ingest::decode_path::{lane_preimage, lane_roots, parse_lock, resolve, Pkg};
use tessera_ingest::decoder::{Decoder, ALL};

/// This checkout's `Cargo.toml` and workspace `Cargo.lock`, read now rather than at build time.
fn this_checkout() -> (String, Vec<Pkg>, String) {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let cargo_toml =
        std::fs::read_to_string(manifest_dir.join("Cargo.toml")).expect("our Cargo.toml");
    // crates/tessera-ingest → crates → the workspace root.
    let lock = std::fs::read_to_string(manifest_dir.join("../../Cargo.lock"))
        .expect("a workspace checkout has its lockfile");
    (cargo_toml, parse_lock(&lock), lock)
}

/// The version this checkout's lockfile resolved for `krate`.
fn locked_version(pkgs: &[Pkg], krate: &str) -> String {
    resolve(pkgs, krate, None)
        .unwrap_or_else(|| panic!("{krate} resolves at exactly one version"))
        .version
        .clone()
}

/// Which crate's version each third-party lane seals, and which Cargo feature is its lane. Stated here
/// independently of `decoder.rs`, so the two can only agree by both being right.
fn lane_of(d: &Decoder) -> Option<(Option<&'static str>, &'static str)> {
    match d.name {
        "arrow-rs/parquet" => Some((Some("parquet"), "parquet")),
        "arrow-rs/arrow-ipc" => Some((Some("arrow-ipc"), "arrow")),
        "rust-csv" => Some((Some("csv"), "csv")),
        // The archive reader is pinned in the digest; the member parser is ours, so no version.
        "tessera/npz" => Some((None, "npz")),
        // In-tree `.npy`: nothing third-party to describe.
        "tessera/npy" => None,
        other => panic!("a new decoder lane `{other}` needs a row here"),
    }
}

#[test]
fn every_sealed_decoder_triple_matches_this_checkouts_resolution() {
    let (cargo_toml, pkgs, lock) = this_checkout();
    let lanes = lane_roots(&cargo_toml);
    // Empty under `--no-default-features`: no lane compiled in, nothing sealed, nothing to compare.

    for d in ALL {
        let Some((root, lane)) = lane_of(d) else {
            assert!(d.feature_preimage().is_none(), "{} is in-tree", d.name);
            continue;
        };
        let sealed = d.to_value();

        let want_version = root.map(|r| format!("={}", locked_version(&pkgs, r)));
        assert_eq!(
            sealed.get("version").and_then(|v| v.as_str()),
            want_version.as_deref(),
            "{}: the sealed version is not the version this checkout links",
            d.name
        );

        let roots = &lanes
            .iter()
            .find(|(l, _)| l == lane)
            .unwrap_or_else(|| panic!("lane {lane} declared in Cargo.toml"))
            .1;
        let want_preimage = lane_preimage(&lock, roots).expect("a third-party lane has pins");
        assert_eq!(
            d.feature_preimage(),
            Some(want_preimage.as_str()),
            "{}: the sealed feature pre-image names decoders this checkout does not link",
            d.name
        );
    }
}

#[test]
fn tessera_info_reports_the_versions_this_checkout_links() {
    let (_, pkgs, _) = this_checkout();
    let mut rows = vec![("dicom", "dicom"), ("dicom-series", "dicom")];
    if cfg!(feature = "parquet") {
        rows.push(("parquet", "parquet"));
    }
    if cfg!(feature = "arrow") {
        rows.push(("arrow", "arrow-ipc"));
    }
    if cfg!(feature = "csv") {
        rows.push(("csv", "csv"));
    }
    if cfg!(feature = "npz") {
        rows.push(("npz-member", "zip"));
    }
    for (backend, krate) in rows {
        assert_eq!(
            backend_version(backend),
            Some(locked_version(&pkgs, krate)),
            "`tessera info` would report a {krate} this checkout does not link (backend {backend})"
        );
    }
}

/// The linked parquet attests to its own version: `DEFAULT_CREATED_BY` is `concat!`ed from parquet's
/// `CARGO_PKG_VERSION` when *parquet* is compiled, and it is the string parquet writes into every footer.
#[cfg(feature = "parquet")]
#[test]
fn the_sealed_parquet_version_is_the_linked_parquets_own() {
    let linked = parquet::file::properties::DEFAULT_CREATED_BY
        .strip_prefix("parquet-rs version ")
        .expect("parquet's created_by shape");
    assert_eq!(
        Decoder::PARQUET.to_value()["version"],
        format!("={linked}"),
        "the seal names a parquet other than the one linked into this binary"
    );
    assert_eq!(backend_version("parquet").as_deref(), Some(linked));
}
