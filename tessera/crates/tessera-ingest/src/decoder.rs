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
}

impl Decoder {
    /// arrow-rs's Parquet reader — the table lane's Parquet decoder.
    pub const PARQUET: Decoder = Decoder {
        name: "arrow-rs/parquet",
        version: option_env!("TESSERA_DEP_PARQUET"),
    };
    /// arrow-rs's IPC reader — Arrow IPC / Feather source files.
    pub const ARROW_IPC: Decoder = Decoder {
        name: "arrow-rs/arrow-ipc",
        version: option_env!("TESSERA_DEP_ARROW_IPC"),
    };
    /// The CSV lane. The `csv` crate tokenises RFC-4180 records; every *value* is parsed by Rust
    /// std's `str::parse`, so the tokenizer is the only third-party component there is to name.
    pub const CSV: Decoder = Decoder {
        name: "rust-csv",
        version: option_env!("TESSERA_DEP_CSV"),
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
        if let Some(digest) = feature_digest() {
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

/// The pre-image the [`feature_digest`] is taken over — emitted verbatim by `build.rs`.
///
/// Shape: `pins=<crate=version,…>` over every crate on the generic-ingest decode path. Kept
/// human-readable on purpose: a digest with no recoverable pre-image is an unfalsifiable label, which
/// is the failure mode §6a rejected the profile id for. `tessera info --json` prints it.
///
/// **It contains no feature list and no version of ours**, and `build.rs` documents why: a lane that was
/// not compiled in did not read the file, and this crate's software version moves on every release
/// (which would re-introduce the corpus churn ADR-0052 §1 removed). Both were tried; both made
/// `manifest_hash` move for a reason that was not a difference in interpretation.
pub fn feature_preimage() -> Option<&'static str> {
    option_env!("TESSERA_INGEST_DECODE_PINS")
}

/// `blake3:` digest over [`feature_preimage`] — the third component of the triple.
///
/// Computed at runtime from a build-emitted string rather than in `build.rs`, because hashing there
/// would need a `[build-dependencies]` entry, and adding one moves the resolved feature graph — which
/// is itself an ADR-0057 Gate B event. A disproportionate price for hashing one short string with a
/// crate we already depend on.
pub fn feature_digest() -> Option<String> {
    feature_preimage().map(|p| tessera_core::hash::digest(p.as_bytes()))
}

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
    /// and would not move when a decoder did.
    #[test]
    fn the_preimage_names_the_decoder_pins() {
        let p = feature_preimage().expect("a workspace build always emits the pre-image");
        assert!(p.starts_with("pins="), "{p}");
        assert!(p.contains("arrow-array=58."), "the arrow pin is named: {p}");
        assert!(p.contains("parquet=58."), "the parquet pin is named: {p}");
    }

    /// The pre-image must **not** name this build's feature selection or this crate's software version.
    ///
    /// Both were in an earlier derivation and both were wrong, for different reasons (see `build.rs`).
    /// This is a regression test rather than a style check: either one makes `manifest_hash` move when
    /// the *interpretation of the file* did not, and `manifest_hash` is the format's version identity.
    #[test]
    fn the_preimage_is_invariant_to_feature_selection_and_to_our_own_version() {
        let p = feature_preimage().expect("a workspace build emits the pre-image");
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
