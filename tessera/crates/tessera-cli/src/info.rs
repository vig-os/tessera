//! `tessera info` — what *this* build is (ADR-0057 §7).
//!
//! Feature selection may decide whether a format is **readable**; it may never change the **bytes**
//! produced for a readable one (ADR-0057 §7's guarantee, enforced by §5's gates). That makes the
//! feature set of a binary a support question rather than a correctness one — but only if someone
//! who did not build the binary can *see* it. Before this, `--version` printed a bare version and
//! every "why did this fail?" cost a round-trip.
//!
//! Three lines, one question each:
//!
//! ```text
//! tessera 0.1.0-alpha.1  (format tessera_version 0.0.0)
//! backends:   blob · dicom 0.9.7 · dicom-series 0.9.7 · hdf-compound 1.14.6 · nifti · raw
//! disabled:   (none)
//! build:      static-hdf5=no · cloud=no · sql=no
//! ```
//!
//! `--json` emits the same facts in a shape meant to be embedded in `aux/provenance.json`.
//!
//! **The flavor is deliberately NOT sealed.** ADR-0056 §6.2 seals `ingest_decoder` — the decoder
//! name + version, the thing that actually determined the bytes. A compile-time bundle label did
//! not, and sealing it would invite the false inference "same flavor ⇒ same bytes". ADR-0042's line
//! holds: sealed = what changed the bytes; `aux/` = who was in the room.

use std::io::Write;

use serde::Serialize;
use tessera_core::{Error, Result};
use tessera_ingest::backends::{backend_version, backends_disabled, BACKENDS_ENABLED};

/// A **build mode** — how this binary was assembled, as opposed to what it can decode.
///
/// ADR-0057 §4/§5 keeps these separate on purpose: `static-hdf5` is not a capability (it only
/// switches how libhdf5 links) and is deliberately excluded from the `full` capability set, while
/// `cloud` and `sql` add surfaces rather than decoders. All three are `cfg!`, so they cost nothing
/// at runtime and cannot disagree with the binary they were compiled into.
const BUILD_FLAGS: &[(&str, bool)] = &[
    ("static-hdf5", cfg!(feature = "static-hdf5")),
    ("cloud", cfg!(feature = "cloud")),
    ("sql", cfg!(feature = "sql")),
];

/// One compiled-in backend and, when there is one to report, its decoder version.
#[derive(Debug, Serialize)]
struct BackendInfo {
    name: &'static str,
    /// Omitted for the in-tree parsers, which have no third-party decoder to name.
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
}

/// The machine-readable form of `tessera info`, shaped for `aux/provenance.json`.
#[derive(Debug, Serialize)]
struct Info {
    /// The software version (ADR-0052 §1) — `CARGO_PKG_VERSION`, not the format version.
    tessera_version: &'static str,
    /// The format/spec version this build *writes* into a manifest's `tessera_version` field.
    format_version: &'static str,
    backends: Vec<BackendInfo>,
    /// Known backend names whose handler is not in this build. Empty until Phase 1's feature graph.
    backends_disabled: Vec<&'static str>,
    /// Build modes, by the same names their Cargo features carry.
    build: std::collections::BTreeMap<&'static str, bool>,
}

fn collect() -> Info {
    Info {
        tessera_version: env!("CARGO_PKG_VERSION"),
        format_version: tessera_core::manifest::TESSERA_VERSION,
        backends: BACKENDS_ENABLED
            .iter()
            .map(|&name| BackendInfo {
                name,
                version: backend_version(name),
            })
            .collect(),
        backends_disabled: backends_disabled(),
        build: BUILD_FLAGS.iter().copied().collect(),
    }
}

/// Render `name version` / `name` for the human `backends:` line.
fn render(b: &BackendInfo) -> String {
    match &b.version {
        Some(v) => format!("{} {v}", b.name),
        None => b.name.to_string(),
    }
}

/// `tessera info [--json]` — the version, the resolved backend set, and the build-mode flags.
pub fn info(json: bool, out: &mut dyn Write) -> Result<()> {
    let i = collect();

    if json {
        let text = serde_json::to_string_pretty(&i)
            .map_err(|e| Error::Invalid(format!("info: serialize: {e}")))?;
        return writeln!(out, "{text}").map_err(Error::from);
    }

    writeln!(
        out,
        "tessera {}  (format tessera_version {})",
        i.tessera_version, i.format_version
    )
    .map_err(Error::from)?;
    let backends: Vec<String> = i.backends.iter().map(render).collect();
    writeln!(out, "backends:   {}", backends.join(" · ")).map_err(Error::from)?;
    let disabled = if i.backends_disabled.is_empty() {
        "(none)".to_string()
    } else {
        i.backends_disabled.join(" · ")
    };
    writeln!(out, "disabled:   {disabled}").map_err(Error::from)?;
    // Declaration order here, not the map's sorted order: ADR-0057 §7 reads build mode first
    // (`static-hdf5=yes · cloud=yes · sql=no`). The JSON form keeps the map, whose sorted keys are
    // what the repo's JCS canonicalisation wants anyway.
    let flags: Vec<String> = BUILD_FLAGS
        .iter()
        .map(|(name, on)| format!("{name}={}", if *on { "yes" } else { "no" }))
        .collect();
    writeln!(out, "build:      {}", flags.join(" · ")).map_err(Error::from)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tessera_ingest::backends::BACKENDS_ALL;

    fn human() -> String {
        let mut buf = Vec::new();
        info(false, &mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn human_form_reports_version_backends_and_build_modes() {
        let out = human();
        let mut lines = out.lines();

        let first = lines.next().unwrap();
        assert!(
            first.starts_with(&format!("tessera {} ", env!("CARGO_PKG_VERSION"))),
            "first line must lead with the software version: {first}"
        );
        assert!(
            first.contains(tessera_core::manifest::TESSERA_VERSION),
            "first line must name the format version too: {first}"
        );

        let backends = lines.next().unwrap();
        assert!(backends.starts_with("backends:   "));
        for name in BACKENDS_ENABLED {
            assert!(
                backends.contains(name),
                "compiled-in backend '{name}' missing from: {backends}"
            );
        }

        assert_eq!(
            lines.next().unwrap(),
            "disabled:   (none)",
            "no decoder feature gate exists yet, so nothing is disabled (Phase 1 changes this)"
        );

        let build = lines.next().unwrap();
        for (flag, _) in BUILD_FLAGS {
            assert!(
                build.contains(&format!("{flag}=")),
                "build mode '{flag}' missing from: {build}"
            );
        }
        assert!(lines.next().is_none(), "info is exactly four lines");
    }

    /// The build-mode line must describe *this* binary, not a hard-coded guess. The test binary is
    /// compiled with the same features as the crate under test, so `cfg!` is the ground truth to
    /// compare against.
    #[test]
    fn build_modes_track_the_compiled_features() {
        let out = human();
        let expect = |flag: &str, on: bool| {
            let want = format!("{flag}={}", if on { "yes" } else { "no" });
            assert!(out.contains(&want), "expected '{want}' in: {out}");
        };
        expect("static-hdf5", cfg!(feature = "static-hdf5"));
        expect("cloud", cfg!(feature = "cloud"));
        expect("sql", cfg!(feature = "sql"));
    }

    #[test]
    fn json_form_is_parseable_and_carries_the_same_facts() {
        let mut buf = Vec::new();
        info(true, &mut buf).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();

        assert_eq!(v["tessera_version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(v["format_version"], tessera_core::manifest::TESSERA_VERSION);
        assert_eq!(
            v["backends"].as_array().unwrap().len(),
            BACKENDS_ENABLED.len()
        );
        assert!(v["backends_disabled"].as_array().unwrap().is_empty());
        assert_eq!(v["build"]["cloud"], cfg!(feature = "cloud"));

        // hdf-compound reports the libhdf5 actually linked; the in-tree parsers report no decoder.
        let by_name = |n: &str| {
            v["backends"]
                .as_array()
                .unwrap()
                .iter()
                .find(|b| b["name"] == n)
                .unwrap()
                .clone()
        };
        assert!(by_name("hdf-compound")["version"].is_string());
        assert!(by_name("raw").get("version").is_none());
    }

    /// ADR-0057 §6: the *name* set is build-invariant. Today no decoder feature exists, so the
    /// compiled-in set is the whole set — and `info` says so rather than implying a lean build.
    #[test]
    fn every_known_backend_is_compiled_in_today() {
        assert_eq!(BACKENDS_ENABLED, BACKENDS_ALL);
        assert!(human().contains("disabled:   (none)"));
    }
}
