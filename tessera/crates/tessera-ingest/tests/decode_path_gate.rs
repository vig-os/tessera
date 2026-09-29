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

use std::collections::BTreeSet;

use tessera_ingest::decode_path::{
    classification, lane_preimage, lane_roots, lock_closure, IN_DIGEST, PREIMAGE_VERSION,
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

/// Every crate reachable on a lane's decode path is either in the digest or excluded with a reason.
///
/// The failing message is written to be actionable, because the person who trips this is mid-way through
/// adding a dependency and needs to know what the decision is, not merely that there is one.
#[test]
fn every_decode_path_candidate_is_classified() {
    let (cargo_toml, lock) = workspace_files();
    let lanes = lane_roots(&cargo_toml);
    assert!(!lanes.is_empty(), "no ingest lanes derived from [features]");

    let mut unclassified: Vec<(String, String)> = Vec::new();
    for (lane, roots) in &lanes {
        for c in lock_closure(&lock, roots) {
            if classification(&c).is_none() {
                unclassified.push((lane.clone(), c));
            }
        }
    }
    assert!(
        unclassified.is_empty(),
        "unclassified crate(s) on an ingest decode path — each must go into `IN_DIGEST` (it can change a \
         decoded value) or into `EXCLUDED` with the reason it cannot, in crates/tessera-ingest/src/\
         decode_path.rs:\n{}",
        unclassified
            .iter()
            .map(|(l, c)| format!("  lane {l}: {c}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// No `IN_DIGEST` entry names a crate that is on no lane's path.
///
/// The mirror of the test above, and the reason the list cannot quietly rot: a decoder that is dropped or
/// renamed leaves a stale entry that contributes nothing, and a reader would have no way to tell a stale
/// name from one that simply did not resolve on this build.
#[test]
fn no_digest_entry_is_stale() {
    let (cargo_toml, lock) = workspace_files();
    let mut reachable: BTreeSet<String> = BTreeSet::new();
    for (_, roots) in lane_roots(&cargo_toml) {
        reachable.extend(lock_closure(&lock, &roots));
    }
    let stale: Vec<&str> = IN_DIGEST
        .iter()
        .copied()
        .filter(|c| !reachable.contains(*c))
        .collect();
    assert!(
        stale.is_empty(),
        "IN_DIGEST names crate(s) on no lane's decode path — drop them: {stale:?}"
    );
}

/// The lanes are read from `[features]`, not restated, so this pins what that derivation currently yields.
///
/// It is a drift guard rather than a tautology: renaming or removing a capability feature silently changes
/// which digests exist, and a lane that stops being derived would seal no decoder at all.
#[test]
fn the_lanes_are_derived_from_the_capability_features() {
    let (cargo_toml, _) = workspace_files();
    let lanes: Vec<String> = lane_roots(&cargo_toml)
        .into_iter()
        .map(|(l, _)| l)
        .collect();
    assert_eq!(
        lanes,
        vec![
            "arrow".to_string(),
            "csv".to_string(),
            "parquet".to_string()
        ],
        "the derived lane set changed — if a capability feature was added or renamed, the decoder \
         constants in `decoder.rs` need the matching `TESSERA_DECODE_PINS_<LANE>` env var"
    );
    // `default` forwards to all three, and `static-hdf5` only forwards `hdf5-metno/…`, so neither is a
    // lane. Treating `default` as one would rebuild exactly the global list #477 removed.
    assert!(!lanes.contains(&"default".to_string()));
    assert!(!lanes.contains(&"static-hdf5".to_string()));
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
        let names: Vec<&str> = body
            .split(',')
            .map(|e| e.split('=').next().unwrap())
            .collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted, "lane {lane} pre-image is not sorted: {p}");
        assert!(seen.insert(p.clone()), "two lanes share a pre-image: {p}");
    }
}
