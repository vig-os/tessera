//! The backend name set — one string set for every surface that names a decoder (ADR-0057 §6/§7).
//!
//! ADR-0057 §6 keeps [`crate::spec::FormatOptions`] a **closed and total** enum: every variant
//! parses on every build, so a TOML spec's meaning — and therefore the `spec_hash` that ADR-0035
//! writes into each member's `ingested_via_spec` provenance edge — never depends on which binary
//! read it. What a feature gate may switch off is the *handler*, not the *name*.
//!
//! Two constants fall out of that, and they are what `tessera info` prints:
//!
//! - [`BACKENDS_ALL`] — every backend name, on every build. Build-invariant.
//! - [`BACKENDS_ENABLED`] — the subset whose handler is compiled into *this* build.
//!
//! The generic-ingest lanes (`arrow` / `parquet` / `csv`, ADR-0056 §11) are the first feature-gated
//! backends, so the two lists have genuinely diverged: a `--no-default-features` build knows all ten
//! names and can run seven. The vendor decoders (`dicom`, `hdf5-metno`) are still unconditional
//! dependencies of this crate — retro-gating them is ADR-0057 Phase 1's remaining half and moves every
//! vendor backend, so it is tracked as #454 rather than smuggled in alongside #386.
//!
//! Everything downstream reads these two lists and nothing else: `tessera info`, the missing-backend
//! error, and `aux/provenance.json`. Adding a gate is one `#[cfg]` attribute here.

/// Every backend name `FormatOptions` can carry — the `format = "<name>"` tag in an ingest spec, and
/// (per ADR-0056 §4) the `--from <name>` string the CLI will take. **Build-invariant**: this list is
/// identical in every build, including builds that cannot run half of it.
///
/// Sorted, so `tessera info` and every error message that enumerates it are stable.
///
/// Kept in sync with `FormatOptions` by [`backend_name`], whose `match` the compiler forces to be
/// total, and by `tests::backends_all_matches_the_serde_variant_list`, which reads the variant list
/// out of serde's own "unknown variant" error rather than a hand-written copy.
pub const BACKENDS_ALL: &[&str] = &[
    "arrow",
    "blob",
    "blob-series",
    "csv",
    "dicom",
    "dicom-series",
    "hdf-compound",
    "nifti",
    "parquet",
    "raw",
];

/// The backends whose handler is compiled into **this** build.
///
/// A gate is one `#[cfg(feature = "…")]` attribute per element, and [`BACKENDS_ALL`] is never touched
/// — which is the property that keeps a TOML spec's `spec_hash` portable across builds (ADR-0057 §6).
///
/// A name present here but absent from a given build is reported as `disabled`, and an ingest naming
/// it fails with [`tessera_core::Error::BackendNotCompiled`] (exit 3) rather than "unknown backend"
/// (exit 2) — the distinction that lets a cookbook recipe tell "wrong build" from "wrong invocation".
pub const BACKENDS_ENABLED: &[&str] = &[
    // `dep:arrow-ipc` — Arrow IPC / Feather via the ADR-0056 §2 type map (feature `arrow`)
    #[cfg(feature = "arrow")]
    "arrow",
    // in-tree: ADR-0038 opaque preservation tier. Never gated off in practice.
    "blob",
    // in-tree: block-per-file multi-file preservation (#301/#329).
    "blob-series",
    // `dep:csv` for RFC-4180 tokenising; every value is parsed by Rust std (feature `csv`)
    #[cfg(feature = "csv")]
    "csv",
    // `dep:dicom` + `dep:dicom-transfer-syntax-registry` (Phase 1: feature `dicom`)
    "dicom",
    "dicom-series",
    // `dep:hdf5-metno` + `dep:hdf5-metno-sys` — needs a native libhdf5 (Phase 1: feature `hdf5`)
    "hdf-compound",
    // in-tree parsers, no third-party decoder
    "nifti",
    // `dep:parquet` (+ the arrow lane it implies) — ADR-0056 §11's P1 table row (feature `parquet`)
    #[cfg(feature = "parquet")]
    "parquet",
    "raw",
];

/// Backend names that exist but whose handler is not in this build — `BACKENDS_ALL \ BACKENDS_ENABLED`.
///
/// This is the "known but disabled" line of ADR-0057 §7's missing-backend error, and the `disabled:`
/// line of `tessera info`. Empty on a default build; non-empty as soon as a generic lane is switched
/// off, which is what makes the two error classes distinguishable.
pub fn backends_disabled() -> Vec<&'static str> {
    BACKENDS_ALL
        .iter()
        .copied()
        .filter(|n| !BACKENDS_ENABLED.contains(n))
        .collect()
}

/// The version of the **decoder** behind a backend, when there is a meaningful one to report.
///
/// ADR-0056 §6a seals the `ingest_decoder` triple in the provenance recipe bag. This is the **read**
/// side of the same fact — what a support round-trip needs when someone asks "why did my hash change?"
/// — and it reads the same `build.rs`-emitted pins [`crate::decoder`] seals, so `tessera info` and a
/// product's recipe cannot disagree.
///
/// (Note the correction §6a made to the older framing here: `content_hash` is *not* a function of the
/// decoder. It is a function of the extracted logical values and Tessera's own encoder, so a decoder
/// bump that extracts identical values moves no hash at all.)
///
/// Three sources, in decreasing order of how much they tell you:
///
/// - `hdf-compound` reports the **libhdf5 actually linked**, queried at runtime. That is the number
///   that differs between a nix build (system libhdf5 from the closure) and a released binary
///   (`static-hdf5`, vendored HDF5 built from source) — a compile-time pin could not tell them apart.
/// - `dicom` / `dicom-series` report the `dicom` crate pin captured from `Cargo.lock` by `build.rs`.
///   Absent (→ `None`) when the crate is built without the workspace lockfile, e.g. as a
///   crates.io dependency.
/// - `parquet` / `arrow` / `csv` report the `=`-pinned crate versions captured from `Cargo.lock` by
///   `build.rs` — the same pins that go into the sealed triple.
/// - the in-tree parsers (`nifti`, `raw`, `blob`) have no third-party decoder to name. Their version
///   *is* the tessera version already on the first line.
pub fn backend_version(name: &str) -> Option<String> {
    match name {
        "dicom" | "dicom-series" => option_env!("TESSERA_DEP_DICOM").map(str::to_owned),
        // The generic-ingest lanes (ADR-0056 §6a). These are the same pins the sealed
        // `ingest_decoder` triple carries, so `tessera info` and a product's recipe agree by
        // construction rather than by two lists staying in sync.
        "parquet" => option_env!("TESSERA_DEP_PARQUET").map(str::to_owned),
        "arrow" => option_env!("TESSERA_DEP_ARROW_IPC").map(str::to_owned),
        "csv" => option_env!("TESSERA_DEP_CSV").map(str::to_owned),
        "hdf-compound" => {
            let (major, minor, patch) = hdf5_metno::library_version();
            Some(format!("{major}.{minor}.{patch}"))
        }
        _ => None,
    }
}

/// The backend name of a parsed spec entry.
///
/// The `match` is exhaustive, so adding a variant to [`crate::spec::FormatOptions`] without giving it
/// a name here is a **compile error**, not a silently unnamed backend.
pub fn backend_name(opts: &crate::spec::FormatOptions) -> &'static str {
    use crate::spec::FormatOptions as F;
    match opts {
        F::Arrow { .. } => "arrow",
        F::Blob { .. } => "blob",
        F::BlobSeries { .. } => "blob-series",
        F::Csv { .. } => "csv",
        F::Dicom { .. } => "dicom",
        F::DicomSeries { .. } => "dicom-series",
        F::HdfCompound { .. } => "hdf-compound",
        F::Nifti { .. } => "nifti",
        F::Parquet { .. } => "parquet",
        F::Raw { .. } => "raw",
    }
}

/// ADR-0057 §7's missing-backend guidance — the block `tessera` prints under a
/// [`tessera_core::Error::BackendNotCompiled`].
///
/// Lives here rather than in the error's `Display` impl for two reasons: it needs
/// [`BACKENDS_ENABLED`] / [`backends_disabled`], which are this module's; and a library caller
/// embedding Tessera wants the one-line error, not a page of installer advice.
///
/// The requirement it satisfies is that the message be **diagnosable by someone who did not build the
/// binary** — hence naming what *is* compiled in, not only what is missing, and pointing at
/// `tessera info` as the general answer rather than repeating itself per backend.
pub fn missing_backend_help() -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "  compiled-in backends: {}\n",
        BACKENDS_ENABLED.join(", ")
    ));
    let disabled = backends_disabled();
    if !disabled.is_empty() {
        out.push_str(&format!(
            "  known but disabled:   {}\n",
            disabled.join(", ")
        ));
    }
    out.push_str(
        "\n  This is a lean build. The released binary carries every backend:\n    \
         curl -LsSf https://github.com/vig-os/tessera/releases/latest/download/tessera-cli-installer.sh | sh\n  \
         or build from source with it enabled:\n    \
         cargo install tessera-cli --features parquet,csv\n  \
         inspect any build:\n    \
         tessera info\n",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backends_all_is_sorted_and_unique() {
        let mut sorted = BACKENDS_ALL.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted, BACKENDS_ALL,
            "BACKENDS_ALL must stay sorted + duplicate-free — it is printed verbatim"
        );
    }

    #[test]
    fn enabled_is_a_subset_of_all() {
        for name in BACKENDS_ENABLED {
            assert!(
                BACKENDS_ALL.contains(name),
                "BACKENDS_ENABLED names '{name}', which is not a known backend"
            );
        }
        // The generic-ingest lanes (ADR-0057 §5) are the first feature-gated backends, so `ENABLED`
        // is no longer trivially `ALL` — which is the point of the split. On a default build every
        // name is present; on `--no-default-features` the three generic ones are `disabled`, and
        // `tessera info` says so instead of the ingest failing with a parse error.
        if cfg!(all(feature = "arrow", feature = "parquet", feature = "csv")) {
            assert_eq!(
                BACKENDS_ENABLED, BACKENDS_ALL,
                "a full build enables everything"
            );
            assert!(backends_disabled().is_empty());
        } else {
            assert!(
                !backends_disabled().is_empty(),
                "a build with a generic lane off must report it as disabled, not as unknown"
            );
        }
    }

    /// The anti-drift guard: serde derives the variant list from `FormatOptions` itself, and reports
    /// it verbatim when an unknown `format = "…"` tag is deserialised. Comparing against *that* —
    /// rather than a second hand-written list — means a new variant fails this test until it is
    /// registered in `BACKENDS_ALL`.
    #[test]
    fn backends_all_matches_the_serde_variant_list() {
        let err = toml::from_str::<crate::spec::FormatOptions>("format = \"__no_such_backend__\"")
            .expect_err("an unknown format tag must not parse")
            .to_string();
        let (_, list) = err
            .split_once("expected one of ")
            .unwrap_or_else(|| panic!("serde error text changed shape: {err}"));
        let mut variants: Vec<String> = list
            .split('`')
            .skip(1)
            .step_by(2)
            .map(str::to_owned)
            // `Blob` carries `#[serde(alias = "junk")]` — the cathartic alias is an input spelling,
            // not a backend. `tessera info` lists backends, so it stays out.
            .filter(|v| v != "junk")
            .collect();
        variants.sort();
        variants.dedup();
        assert_eq!(
            variants, BACKENDS_ALL,
            "FormatOptions and BACKENDS_ALL have drifted — add the new variant to BACKENDS_ALL \
             (and to BACKENDS_ENABLED if its handler is compiled in unconditionally)"
        );
    }

    /// `backend_name` is total by construction (the compiler checks the match); this pins the actual
    /// strings, so a rename shows up here rather than in a user's broken `--from` invocation.
    #[test]
    fn backend_name_agrees_with_the_serde_tag() {
        use crate::spec::FormatOptions as F;
        let samples = [
            F::Blob {
                input: "x".into(),
                media_type: None,
            },
            F::Dicom {
                input: "x".into(),
                deidentify: false,
                recipients: Vec::new(),
            },
            F::Nifti { input: "x".into() },
        ];
        for s in &samples {
            let tag = serde_json::to_value(s).unwrap()["format"]
                .as_str()
                .unwrap()
                .to_owned();
            assert_eq!(tag, backend_name(s));
            assert!(BACKENDS_ALL.contains(&tag.as_str()));
        }
    }

    #[test]
    fn linked_libhdf5_version_is_reportable() {
        let v = backend_version("hdf-compound").expect("libhdf5 is linked, so it has a version");
        assert!(
            v.split('.').count() == 3 && v.starts_with(|c: char| c.is_ascii_digit()),
            "expected a major.minor.patch libhdf5 version, got '{v}'"
        );
        assert_eq!(
            backend_version("raw"),
            None,
            "in-tree parsers have no decoder"
        );
    }
}
