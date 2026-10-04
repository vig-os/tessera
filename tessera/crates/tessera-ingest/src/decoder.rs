//! The sealed **decoder identity** (ADR-0056 §6a) — a build-honest triple in the provenance recipe
//! bag, under the well-known key `ingest_decoder`.
//!
//! # Why this is a recipe fact and not a format field
//!
//! An earlier draft of ADR-0056 §6 made `ingest_decoder` a fourth sealed *format field*. It is not
//! one: it names *how the product was made*, which is a **recipe** fact, and ADR-0058 already gives
//! recipe facts a sealed home — `Generation.config`, "a deliberately non-opinionated bag of the
//! settings the generator used". Putting it there costs **zero new format surface** while keeping the
//! record inside the seal, so it is tamper-evident on disk and tape and not only over digest-pinned
//! OCI.
//!
//! # Why record it at all — `content_hash` does not depend on it
//!
//! It genuinely does not, and ADR-0056 §6a retracts the sentence that claimed otherwise. Ingest is a
//! *logical re-encode* (§2), so `content_hash = f(extracted logical values, Tessera's encoder)`: a
//! decoder bump that extracts identical values **cannot** move it. And detection needs no decoder id
//! either, because every ingest seals an `ingested_from` edge whose `content_hash` is a merkle root
//! over the source bytes — so *same source digest + changed `content_hash` ⟹ the interpretation
//! changed*, from sealed data alone.
//!
//! What the record adds is **attribution** and **recipe completeness**. A recipe is a reproduction
//! contract: `Generation` exists so someone holding the source can re-run it. A sealed recipe that
//! names the energy window but omits the decoder is knowingly incomplete on the single dimension most
//! likely to move the values (hazards H1–H9). A decoder bump therefore moves `manifest_hash` — like
//! any other recipe change, and for the same reason: it was **made differently**.
//!
//! # The triple, and why the feature digest is load-bearing
//!
//! ```json
//! "ingest_decoder": { "name": "arrow-rs/parquet", "version": "=58.3.0", "features": "blake3:9f2c…" }
//! ```
//!
//! A bare semver names two differently-behaving decoders identically whenever an unrelated optional
//! feature perturbs the decode path — ADR-0057 §5 proved `--features sql` flips
//! `arrow-array/chrono-tz`, which is hazard H1's exact mechanism arriving through a feature rather
//! than a host. Recording only the version would be a false claim of sameness.
//!
//! **Nobody types any of it.** The name is a constant per backend; the version comes out of the
//! workspace `Cargo.lock` via `build.rs`; the features digest is blake3 over a build-emitted
//! pre-image. A hand-written `ingest_decoder` in an ingest spec is a hard error (see
//! [`crate::engine`]) — a maintainer-typed claim inside an immutable record is the thing §6a rejected
//! a profile id for.
//!
//! # The residual, stated plainly
//!
//! The digest's pre-image is the pinned versions of the decode-path crates, and nothing else. It does
//! **not** capture a feature flipped *inside* the shared arrow tree by an unrelated crate (the `sql` →
//! `chrono-tz` case), because a build script cannot see the unified feature graph of its own
//! dependencies without shelling out to `cargo tree`. Two things discharge that gap rather than one:
//!
//! 1. **We never call arrow's timezone machinery.** Hazard H1's rule is "strip tz, take raw ticks,
//!    apply our own scale" — an Arrow `Timestamp(_, Some(tz))` already stores UTC-normalised ticks
//!    and the zone is a display annotation, so [`crate::arrow_table`] reads the raw `i64` buffer and
//!    no tzdb is consulted on any build.
//! 2. **ADR-0057 Gate A** compares goldens across four feature configurations *including*
//!    `--all-features`, which turns `sql` on. If a feature ever did change a decoded value, the
//!    goldens diverge and the gate fails.
//!
//! The pre-image is not itself sealed (the digest is, per §6a), but it is printed verbatim by
//! `tessera info --json`, so it is recoverable from any build rather than being a one-way function
//! with no inverse anywhere.

use tessera_core::provenance::Generation;

/// The well-known recipe-bag key (ADR-0056 §6a). Documented but never mandatory: an external producer
/// may omit it or record its own decoder, and nothing in the format requires one.
pub const RECIPE_KEY: &str = "ingest_decoder";

/// One decoder's build-honest identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decoder {
    /// The decoder's name. A constant per backend, never a profile id.
    pub name: &'static str,
    /// The `=`-pinned version read from the workspace lockfile at build time, or `None` when this
    /// crate was built without one (as a crates.io dependency). Absent rather than guessed.
    pub version: Option<&'static str>,
    /// **This lane's** decode-path pre-image, emitted by `build.rs` (#477).
    ///
    /// Per lane, not global. The digest answers "which decoder interpreted *these* bytes", so a parquet
    /// product must not commit to the zip library the `.npz` lane gained — which, while it was one global
    /// list, it did: adding that dependency moved the `manifest_hash` of every parquet and csv product.
    pub pins: Option<&'static str>,
}

impl Decoder {
    /// arrow-rs's Parquet reader — the table lane's Parquet decoder.
    pub const PARQUET: Decoder = Decoder {
        name: "arrow-rs/parquet",
        version: option_env!("TESSERA_DEP_PARQUET"),
        // The `parquet` feature's closure: the Parquet reader, the arrow tree it decodes through, the
        // Thrift metadata reader, and every page codec.
        pins: option_env!("TESSERA_DECODE_PINS_PARQUET"),
    };
    /// arrow-rs's IPC reader — Arrow IPC / Feather source files.
    pub const ARROW_IPC: Decoder = Decoder {
        name: "arrow-rs/arrow-ipc",
        version: option_env!("TESSERA_DEP_ARROW_IPC"),
        // The `arrow` feature's closure — no `parquet`, and no Thrift.
        pins: option_env!("TESSERA_DECODE_PINS_ARROW"),
    };
    /// The CSV lane. The `csv` crate tokenises RFC-4180 records; every *value* is parsed by Rust
    /// std's `str::parse`, so the tokenizer is the only third-party component there is to name.
    pub const CSV: Decoder = Decoder {
        name: "rust-csv",
        version: option_env!("TESSERA_DEP_CSV"),
        // The `csv` feature's closure: three crates. ADR-0057 §5 sketched `csv = ["arrow", …]` and it
        // turned out not to need arrow at all, so this lane's digest is genuinely tiny — and until #477
        // it carried the whole arrow tree and `parquet` besides.
        pins: option_env!("TESSERA_DECODE_PINS_CSV"),
    };

    /// The NumPy `.npy` lane — **an in-tree decoder**, and the only lane with no third-party code on
    /// its path at all. ADR-0056 §12 declined `ndarray-npy` because "NPY is a header parse plus a
    /// memcpy", so the parser is this crate and the honest record names no one else.
    ///
    /// All three optional components are absent, each for a stated reason rather than by omission:
    ///
    /// - `version` is `None` because the only version available would be **ours**, and `build.rs`
    ///   removed our version from the pre-image precisely because "our software version moves on every
    ///   release, which would re-introduce the corpus churn ADR-0052 §1 removed". Putting it in the
    ///   triple instead would recreate that churn one field over: every release would move the
    ///   `manifest_hash` of every `.npy` product without anything having changed about how the bytes
    ///   were read. Pinning *which* in-tree parser ran is worth having, but it needs an identity that
    ///   does not move per release (a digest of the parser's own sources) — #508.
    /// - `pins` is `None` because `npy = []` enables no dependency, so `lane_roots` correctly declines
    ///   to treat it as a third-party lane and `build.rs` emits no pre-image for it. A digest over an
    ///   empty pre-image would be *identical for every build*, which is a false claim of sameness —
    ///   the same reasoning `to_value` gives for omitting rather than defaulting.
    ///
    /// The record is therefore `{"name": "tessera/npy"}`: short, and true.
    pub const NPY: Decoder = Decoder {
        name: "tessera/npy",
        version: None,
        pins: None,
    };

    /// The NumPy `.npz` lane — the same in-tree member parser, reached **through a zip archive**.
    ///
    /// A separate lane from [`Decoder::NPY`] rather than a flag on it, because `zip` is third-party code
    /// on this path and on no other. While the two shared one feature, every plain `.npy` seal committed
    /// to a zip library that never touched its bytes — which is #477's defect exactly (a parquet seal
    /// committing to this very crate), just one scale down. `zip` reads the *container*; the members are
    /// still parsed in-tree, which is why the name stays ours and `zip` appears in the digest instead of
    /// in the name.
    pub const NPZ: Decoder = Decoder {
        name: "tessera/npz",
        version: None,
        pins: option_env!("TESSERA_DECODE_PINS_NPZ"),
    };

    /// The sealed JSON form of the triple.
    pub fn to_value(&self) -> serde_json::Value {
        let mut m = serde_json::Map::new();
        m.insert("name".into(), self.name.into());
        if let Some(v) = self.version {
            // `=`-prefixed to record that this is a pin, not merely the version that happened to
            // resolve — ADR-0056 §5 requires `=` pins for exactly this reason.
            m.insert("version".into(), format!("={v}").into());
        }
        // Omitted, not defaulted, when there is no pre-image to digest (a crates.io build with no
        // workspace lockfile). A digest over an empty pre-image would be identical for every such
        // build — a false claim of sameness between builds that may have resolved different decoders —
        // so absence is the honest record, exactly as it is for `version`.
        if let Some(digest) = self.feature_digest() {
            m.insert("features".into(), digest.into());
        }
        serde_json::Value::Object(m)
    }

    /// Insert this decoder under [`RECIPE_KEY`] into a (possibly operator-supplied) recipe bag.
    ///
    /// Additive: an operator's own `config` keys are preserved untouched. The engine rejects a
    /// spec-supplied `ingest_decoder` before reaching here, so this never overwrites a human claim.
    pub fn record_into(&self, generation: Option<Generation>) -> Generation {
        generation
            .unwrap_or_default()
            .with(RECIPE_KEY, self.to_value())
    }
}

impl Decoder {
    /// The pre-image this lane's digest is taken over — emitted verbatim by `build.rs`.
    ///
    /// Shape: `v2;pins=<crate>=<version>[@<source>],…` over the crates on **this lane's** decode path.
    /// Kept human-readable on purpose: a digest with no recoverable pre-image is an unfalsifiable label,
    /// which is the failure mode §6a rejected a profile id for. `tessera info --json` prints it.
    ///
    /// The `v2;` prefix is the derivation version. `v1` was the single global `pins=…`; this digest has
    /// been redefined three times, and each earlier redefinition was silent, so a reader comparing digests
    /// across one could not tell "the decoder changed" from "the digest is computed differently". It costs
    /// three bytes to end that.
    ///
    /// **It contains no feature list and no version of ours** — see `build.rs` for why both were tried and
    /// removed: a lane that was not compiled in did not read the file, and our software version moves on
    /// every release, which would re-introduce the corpus churn ADR-0052 §1 removed.
    pub fn feature_preimage(&self) -> Option<&'static str> {
        self.pins
    }

    /// `blake3:` digest over [`Decoder::feature_preimage`] — the third component of the triple.
    ///
    /// Computed at runtime from a build-emitted string rather than in `build.rs`, because hashing there
    /// would need a `[build-dependencies]` entry, and adding one moves the resolved feature graph — itself
    /// an ADR-0057 Gate B event. A disproportionate price for hashing one short string with a crate we
    /// already depend on.
    pub fn feature_digest(&self) -> Option<String> {
        self.pins.map(|p| tessera_core::hash::digest(p.as_bytes()))
    }
}

/// Every lane **compiled into this build** that records a decoder triple — for `tessera info`, and for
/// the gate that checks each lane's digest is distinct from the others'.
///
/// Gated by feature, because a lane that is not compiled in cannot have read anything and must not be
/// reported as though it could. The pre-images themselves are derived from the *lockfile*, so they are
/// feature-selection-invariant (§6a requires that — whether the CSV lane was compiled has nothing to do
/// with how a Parquet file was read); what varies here is only which lanes exist to describe.
pub const ALL: &[Decoder] = &[
    #[cfg(feature = "parquet")]
    Decoder::PARQUET,
    #[cfg(feature = "arrow")]
    Decoder::ARROW_IPC,
    #[cfg(feature = "csv")]
    Decoder::CSV,
    #[cfg(feature = "npy")]
    Decoder::NPY,
    #[cfg(feature = "npz")]
    Decoder::NPZ,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_triple_is_derived_and_shaped_as_the_adr_specifies() {
        let v = Decoder::PARQUET.to_value();
        assert_eq!(v["name"], "arrow-rs/parquet");
        assert!(
            v["features"].as_str().unwrap().starts_with("blake3:"),
            "the feature component is a digest: {v}"
        );
        // Built inside the workspace, so the lockfile pin is present and `=`-prefixed.
        let version = v["version"]
            .as_str()
            .expect("a workspace build has the pin");
        assert!(
            version.starts_with("=58."),
            "expected the =-pinned parquet version, got {version}"
        );
    }

    /// The pre-image must name the decoder pins — otherwise the digest is stable for the wrong reason
    /// and would not move when a decoder did. It must also name the wire-format readers and the page
    /// codecs, which #477 found absent: a Parquet file's metadata is Thrift and its pages are compressed,
    /// so a digest that omits `thrift` and `snap` does not describe the decoder that read it.
    #[test]
    fn the_preimage_names_this_lanes_decode_path() {
        let p = Decoder::PARQUET
            .feature_preimage()
            .expect("a workspace build always emits the pre-image");
        assert!(
            p.starts_with("v2;pins="),
            "the derivation version is declared: {p}"
        );
        for want in [
            "parquet=58.",
            "arrow-array=58.",
            "thrift=",    // Parquet's footer / schema / page headers
            "snap=",      // a page codec
            "zstd=",      // ditto
            "flate2=",    // ditto
            "crc32fast=", // page checksums
            "chrono-tz=", // §6a names this as hazard H1's mechanism
        ] {
            assert!(p.contains(want), "the parquet lane must pin {want}: {p}");
        }
    }

    /// **Per lane, not global** — the defect #477 opened on. Each lane's pre-image covers its own decode
    /// path and nothing else, so adding a dependency for one lane cannot move another lane's seal.
    #[test]
    fn each_lane_pins_only_its_own_decode_path() {
        let csv = Decoder::CSV.feature_preimage().expect("workspace build");
        let ipc = Decoder::ARROW_IPC
            .feature_preimage()
            .expect("workspace build");
        let pq = Decoder::PARQUET
            .feature_preimage()
            .expect("workspace build");

        // CSV reads text through `csv` + `csv-core` + memchr's scanners. ADR-0057 §5 sketched
        // `csv = ["arrow", …]` and it turned out not to need arrow at all.
        assert!(csv.contains("csv=") && csv.contains("csv-core="), "{csv}");
        for absent in ["parquet=", "arrow-array=", "thrift=", "zstd="] {
            assert!(
                !csv.contains(absent),
                "the csv lane never touched {absent}, so it must not commit to it: {csv}"
            );
        }
        // Arrow IPC decodes FlatBuffers messages; it has no Parquet reader and no Thrift.
        assert!(ipc.contains("flatbuffers="), "{ipc}");
        assert!(!ipc.contains("parquet="), "{ipc}");
        assert!(!ipc.contains("thrift="), "{ipc}");
        // Parquet reads THROUGH arrow, so it is a superset of the IPC lane's readers.
        assert!(
            pq.contains("parquet=") && pq.contains("arrow-array="),
            "{pq}"
        );

        // Every lane that HAS a digest must have its own. Before #477 all of them were identical,
        // which is the whole defect in one assertion.
        let digests: std::collections::BTreeSet<String> =
            ALL.iter().filter_map(|d| d.feature_digest()).collect();
        let (in_tree, third_party): (Vec<&Decoder>, Vec<&Decoder>) =
            ALL.iter().partition(|d| d.feature_preimage().is_none());
        assert_eq!(
            digests.len(),
            third_party.len(),
            "every lane with a decode path must have its own digest"
        );
        // Tolerated, but BOUNDED and named rather than blanket: `.npy` is parsed in-tree, so it has no
        // third-party decode path and deliberately carries no digest. If a second such lane appears,
        // this fails — because two digest-less lanes would be indistinguishable in the seal, which is
        // the property the assertion above exists to protect.
        // Derived from the build, not hardcoded: a configuration without the array lane has NO
        // digest-less lane, and `--features arrow` is such a configuration now that it builds at all
        // (#509). The property being guarded is "at most one lane may be digest-less, and only the
        // in-tree parser may be it" — two would be indistinguishable in the seal — and that is what
        // this says in every configuration rather than only in the default one.
        let expected_in_tree: Vec<&str> = if cfg!(feature = "npy") {
            vec!["tessera/npy"]
        } else {
            vec![]
        };
        assert_eq!(
            in_tree.iter().map(|d| d.name).collect::<Vec<_>>(),
            expected_in_tree,
            "only the in-tree NPY parser may be digest-less, and only when it is compiled in"
        );
    }

    /// **The #477 property, one scale down: a `.npy` seal must not name the zip library.**
    ///
    /// `.npy` is parsed in-tree and `.npz` is the same parser behind a zip archive, so while the two
    /// shared one Cargo feature every plain `.npy` product committed to a decoder that never touched its
    /// bytes — exactly what a parquet product did to `zip` before the digest went per-lane. Asserted on
    /// `to_value()`, which IS the sealed form, so this is a statement about what lands in the file.
    #[test]
    fn a_npy_seal_names_no_third_party_decoder_and_a_npz_seal_names_zip() {
        let npy = Decoder::NPY.to_value();
        assert_eq!(npy["name"], "tessera/npy");
        assert!(
            npy.get("version").is_none() && npy.get("features").is_none(),
            "the in-tree lane seals a bare name — a version would be OURS and would churn every \
             release, a digest would be over an empty pre-image: {npy}"
        );
        assert!(
            Decoder::NPY.feature_preimage().is_none(),
            "the `.npy` lane has no decode path to pin, so it must have no pre-image either"
        );

        let npz = Decoder::NPZ.to_value();
        assert_eq!(npz["name"], "tessera/npz");
        // Only in a workspace build is there a lockfile to derive pins from; on a crates.io build both
        // are absent, which `to_value` documents as the honest record rather than a defaulted one.
        if let Some(p) = Decoder::NPZ.feature_preimage() {
            assert!(
                p.contains("zip="),
                "the `.npz` lane reads through the archive reader, so it must pin it: {p}"
            );
            assert!(
                npz.get("features").is_some(),
                "a lane with a pre-image seals its digest: {npz}"
            );
        }
    }

    /// A git-forked decode-path crate must be a different pin from the registry release at the same
    /// version — #477's second finding. Latent today (no decode-path crate is forked), so the property is
    /// tested on the pre-image builder rather than waiting for the first fork to expose it.
    #[test]
    fn a_git_source_makes_a_different_pin_than_the_registry_at_the_same_version() {
        const REGISTRY: &str = r#"
[[package]]
name = "csv"
version = "1.3.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
"#;
        const FORK: &str = r#"
[[package]]
name = "csv"
version = "1.3.1"
source = "git+https://github.com/example/csv?rev=deadbeef#deadbeef"
"#;
        let roots = vec!["csv".to_string()];
        let reg = crate::decode_path::lane_preimage(REGISTRY, &roots).expect("registry pin");
        let fork = crate::decode_path::lane_preimage(FORK, &roots).expect("fork pin");
        // A plain registry release keeps the bare shape, so adopting `source` moved nothing on the
        // fork-free builds that already exist.
        assert_eq!(reg, "v2;pins=csv=1.3.1");
        assert!(fork.contains("@git+"), "a fork records its source: {fork}");
        assert_ne!(
            reg, fork,
            "same version, different decoder — must not hash alike"
        );
    }

    /// The pre-image must **not** name this build's feature selection or this crate's software version.
    ///
    /// Both were in an earlier derivation and both were wrong, for different reasons (see `build.rs`).
    /// This is a regression test rather than a style check: either one makes `manifest_hash` move when
    /// the *interpretation of the file* did not, and `manifest_hash` is the format's version identity.
    #[test]
    fn the_preimage_is_invariant_to_feature_selection_and_to_our_own_version() {
        let p = Decoder::PARQUET
            .feature_preimage()
            .expect("a workspace build emits the pre-image");
        assert!(
            !p.contains("features="),
            "a lane that was not compiled in did not read the file, so it must not reach the seal: {p}"
        );
        assert!(
            !p.contains(env!("CARGO_PKG_VERSION")),
            "sealing our software version would make every release a corpus event (ADR-0052 §1): {p}"
        );
    }

    #[test]
    fn recording_preserves_an_operators_own_recipe_keys() {
        let operator = Generation::default().with("energy_window", serde_json::json!([425, 650]));
        let g = Decoder::CSV.record_into(Some(operator));
        assert!(g.config.contains_key("energy_window"), "operator key kept");
        assert_eq!(g.config[RECIPE_KEY]["name"], "rust-csv");
        // And it works from nothing at all.
        let g = Decoder::CSV.record_into(None);
        assert_eq!(g.config.len(), 1);
        assert!(!g.is_empty(), "a recorded decoder makes the recipe present");
    }
}
