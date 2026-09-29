//! The gate behind the per-lane decoder digest (ADR-0056 §6a, #477): **every candidate on a lane's decode
//! path must be classified**, either into the digest or out of it with a reason.
//!
//! This is the part of #477 that is worth more than either list. Before it, the decode-path crate list was
//! hand-maintained and additive, so forgetting a crate silently narrowed what the sealed digest claimed —
//! and three crates' worth of that had accumulated unnoticed: the Thrift reader that parses Parquet's
//! metadata, the FlatBuffers reader that parses Arrow IPC's, and every page codec. A digest that omits the
//! decompressor cannot answer the question it exists for.
//!
//! So the direction is inverted. The candidate set is *derived* from the lockfile, and an unclassified
//! candidate **fails this test**. A new decode-path dependency therefore stops the build until someone
//! decides whether it can change a decoded value — which is the moment that judgement is cheapest and
//! best-informed. Over-claiming is loud (visible churn); under-claiming used to be silent.
//!
//! Deliberately reads `Cargo.lock` rather than shelling out to `cargo metadata`: it needs no subprocess,
//! no network and no registry, so it has the same teeth in the hermetic flake check as it does locally. A
//! gate that silently degrades to vacuous in CI is worse than no gate.

use std::collections::{BTreeMap, BTreeSet};

use tessera_ingest::decode_path::{
    classification, closure, lane_preimage, lane_roots, parse_lock, Pkg, EXCLUDED, IN_DIGEST,
    PREIMAGE_VERSION,
};

fn workspace_files() -> (String, String) {
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let cargo_toml =
        std::fs::read_to_string(manifest_dir.join("Cargo.toml")).expect("our Cargo.toml");
    // The workspace lockfile: crates/tessera-ingest → crates → <workspace root>.
    let lock = manifest_dir
        .ancestors()
        .map(|d| d.join("Cargo.lock"))
        .find(|p| p.is_file())
        .and_then(|p| std::fs::read_to_string(p).ok())
        .expect("a workspace build has a lockfile");
    (cargo_toml, lock)
}

/// Every package on every lane's decode path, keyed by `(name, version)`.
fn all_candidates(cargo_toml: &str, lock: &str) -> Vec<(String, Pkg)> {
    let pkgs = parse_lock(lock);
    let mut out = Vec::new();
    for (lane, roots) in lane_roots(cargo_toml) {
        for p in closure(&pkgs, &roots) {
            out.push((lane.clone(), p.clone()));
        }
    }
    out
}

/// Every crate reachable on a lane's decode path is either in the digest or excluded with a reason.
///
/// The failing message is written to be actionable, because the person who trips this is mid-way through
/// adding a dependency and needs to know what the decision is, not merely that there is one.
#[test]
fn every_decode_path_candidate_is_classified() {
    let (cargo_toml, lock) = workspace_files();
    let candidates = all_candidates(&cargo_toml, &lock);
    assert!(!candidates.is_empty(), "no candidates derived");

    let mut unclassified: BTreeSet<String> = BTreeSet::new();
    for (lane, p) in &candidates {
        if classification(&p.name).is_none() {
            unclassified.insert(format!("  lane {lane}: {} {}", p.name, p.version));
        }
    }
    assert!(
        unclassified.is_empty(),
        "unclassified crate(s) on an ingest decode path — each must go into `IN_DIGEST` (it can change a \
         decoded value) or into `EXCLUDED` with the reason it cannot, in crates/tessera-ingest/src/\
         decode_path.rs:\n{}",
        unclassified.into_iter().collect::<Vec<_>>().join("\n")
    );
}

/// Neither list may name a crate that is on no lane's path.
///
/// Both directions, because a stale entry rots differently in each: a stale `IN_DIGEST` name contributes
/// nothing and a reader cannot tell it from one that simply did not resolve, while a stale `EXCLUDED` name
/// is a decision recorded about a crate nobody depends on any more — which quietly grows the list a reviewer
/// is supposed to be able to read.
#[test]
fn neither_list_is_stale() {
    let (cargo_toml, lock) = workspace_files();
    let reachable: BTreeSet<String> = all_candidates(&cargo_toml, &lock)
        .into_iter()
        .map(|(_, p)| p.name)
        .collect();

    let stale_digest: Vec<&str> = IN_DIGEST
        .iter()
        .copied()
        .filter(|c| !reachable.contains(*c))
        .collect();
    assert!(
        stale_digest.is_empty(),
        "IN_DIGEST names crate(s) on no lane's decode path — drop them: {stale_digest:?}"
    );

    let stale_excluded: Vec<&str> = EXCLUDED
        .iter()
        .map(|(c, _)| *c)
        .filter(|c| !reachable.contains(*c))
        .collect();
    assert!(
        stale_excluded.is_empty(),
        "EXCLUDED names crate(s) on no lane's decode path — drop them: {stale_excluded:?}"
    );
}

/// A crate resolving at **two versions** on one decode path must contribute **two pins**.
///
/// The bug this guards is the one that reached review (#477): the lockfile was scanned by NAME, so
/// `base64` — present at 0.21.7 and 0.22.1, with `parquet` depending on 0.22.1 — sealed 0.21.7, and a real
/// parquet `base64` bump would not have moved the digest. That is the same false sameness #477 exists to
/// remove, reproduced inside its own fix.
#[test]
fn a_crate_at_two_versions_contributes_two_pins() {
    let (cargo_toml, lock) = workspace_files();
    let pkgs = parse_lock(&lock);
    for (lane, roots) in lane_roots(&cargo_toml) {
        let preimage = lane_preimage(&lock, &roots).expect("a lane pre-image");
        let mut versions: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for p in closure(&pkgs, &roots) {
            if IN_DIGEST.contains(&p.name.as_str()) {
                versions
                    .entry(p.name.clone())
                    .or_default()
                    .insert(p.version.clone());
            }
        }
        for (name, vs) in versions {
            for v in &vs {
                assert!(
                    preimage.contains(&format!("{name}={v}")),
                    "lane {lane}: {name} resolves at {v} on this path but the pre-image does not pin it \
                     ({} version(s) resolved): {preimage}",
                    vs.len()
                );
            }
        }
    }
    // And the property in the abstract, so it holds regardless of what this workspace happens to resolve.
    const TWO: &str = r#"
[[package]]
name = "csv"
version = "1.4.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
dependencies = [
 "base64 0.21.7",
 "base64 0.22.1",
]

[[package]]
name = "base64"
version = "0.21.7"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "base64"
version = "0.22.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
"#;
    let p = lane_preimage(TWO, &["csv".to_string()]).expect("pre-image");
    assert!(p.contains("base64=0.21.7"), "{p}");
    assert!(p.contains("base64=0.22.1"), "both versions are pinned: {p}");
}

/// No in-digest package may be a **path** dependency.
///
/// `Cargo.lock` records no path for one, so its pin would be `name=version@path` — an identity that cannot
/// distinguish two checkouts of the same version at different paths. Rather than pretend otherwise, the gate
/// refuses the situation, so the first decode-path crate vendored by path forces a deliberate decision.
#[test]
fn no_in_digest_package_is_a_path_dependency() {
    let (cargo_toml, lock) = workspace_files();
    let offenders: Vec<String> = all_candidates(&cargo_toml, &lock)
        .into_iter()
        .filter(|(_, p)| IN_DIGEST.contains(&p.name.as_str()) && p.source.is_none())
        .map(|(lane, p)| format!("  lane {lane}: {} {}", p.name, p.version))
        .collect();
    assert!(
        offenders.is_empty(),
        "in-digest path dependency/ies — the lockfile records no path, so the pin cannot identify which \
         checkout was used. Decide how to pin it before relying on the digest:\n{}",
        offenders.join("\n")
    );
}

/// The lanes are read from `[features]`, not restated, so this pins what that derivation currently yields —
/// including the exact root crates, which a single-line array scan silently truncated.
#[test]
fn the_lanes_are_derived_from_the_capability_features() {
    let (cargo_toml, _) = workspace_files();
    let lanes = lane_roots(&cargo_toml);
    let names: Vec<&str> = lanes.iter().map(|(l, _)| l.as_str()).collect();
    assert_eq!(
        names,
        vec!["arrow", "csv", "parquet"],
        "the derived lane set changed — if a capability feature was added or renamed, the decoder \
         constants in `decoder.rs` need the matching `TESSERA_DECODE_PINS_<LANE>` env var"
    );
    let roots: BTreeMap<&str, Vec<&str>> = lanes
        .iter()
        .map(|(l, r)| (l.as_str(), r.iter().map(String::as_str).collect()))
        .collect();
    assert_eq!(
        roots["arrow"],
        vec!["arrow-array", "arrow-buffer", "arrow-ipc", "arrow-schema"]
    );
    assert_eq!(roots["csv"], vec!["csv"]);
    // `parquet` implies `arrow`, so its roots are arrow's plus the Parquet reader.
    assert_eq!(
        roots["parquet"],
        vec![
            "arrow-array",
            "arrow-buffer",
            "arrow-ipc",
            "arrow-schema",
            "parquet"
        ]
    );
    // `default` forwards to all three, and `static-hdf5` only forwards `hdf5-metno/…`, so neither is a
    // lane. Treating `default` as one would rebuild exactly the global list #477 removed.
    assert!(!names.contains(&"default"));
    assert!(!names.contains(&"static-hdf5"));
}

/// Each lane's pre-image is distinct, version-prefixed, and sorted — the three properties the seal rests
/// on. Sorted matters: an unsorted pre-image would depend on iteration order, so the same build could seal
/// two different digests.
#[test]
fn every_lane_preimage_is_versioned_distinct_and_sorted() {
    let (cargo_toml, lock) = workspace_files();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for (lane, roots) in lane_roots(&cargo_toml) {
        let p =
            lane_preimage(&lock, &roots).unwrap_or_else(|| panic!("lane {lane} has no pre-image"));
        let body = p
            .strip_prefix(&format!("{PREIMAGE_VERSION};pins="))
            .unwrap_or_else(|| {
                panic!("lane {lane} pre-image is not {PREIMAGE_VERSION}-prefixed: {p}")
            });
        // Sorted by (name, version), which is the identity — not by name alone.
        let keys: Vec<(&str, &str)> = body
            .split(',')
            .map(|e| {
                let (n, rest) = e.split_once('=').expect("name=version");
                (n, rest.split('@').next().unwrap_or(rest))
            })
            .collect();
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        assert_eq!(keys, sorted, "lane {lane} pre-image is not sorted: {p}");
        assert!(seen.insert(p.clone()), "two lanes share a pre-image: {p}");
    }
}

/// A fork is a different pin from the registry release at the same version, and a **private registry** is
/// too — otherwise a company-internal fork published at 58.3.0 reads as the crates.io release.
#[test]
fn source_distinguishes_a_fork_and_a_private_registry_at_the_same_version() {
    fn one(source: &str) -> String {
        let lock =
            format!("\n[[package]]\nname = \"csv\"\nversion = \"1.4.0\"\nsource = \"{source}\"\n");
        lane_preimage(&lock, &["csv".to_string()]).expect("pre-image")
    }
    let crates_io = one("registry+https://github.com/rust-lang/crates.io-index");
    let private = one("registry+https://internal.example/index");
    let fork = one("git+https://github.com/example/csv?rev=deadbeef#deadbeef");
    // The default registry keeps the bare shape, so adopting `source` moved nothing on existing builds.
    assert_eq!(crates_io, "v2;pins=csv=1.4.0");
    assert!(
        private.contains("@registry+https://internal.example/index"),
        "{private}"
    );
    assert!(fork.contains("@git+"), "{fork}");
    assert_eq!(
        BTreeSet::from([&crates_io, &private, &fork]).len(),
        3,
        "same version, three different decoders — none may hash alike"
    );
}
