//! **The exported edge-hash label must describe what the real producers actually store** (#525).
//!
//! `tessera-core`'s own tests check the label's *shape* against a list of role strings. That cannot catch
//! the defect this file exists for: two different producers write the **same role** with **different
//! constructions**, so a label derived from the role alone is false for one of them. A role list says
//! nothing about what any producer wrote.
//!
//! So these tests drive the producers, recompute each candidate construction independently, and assert
//! both that the stored value really is one of them and that the exported prose names that one.
//!
//! The ambiguity is concrete. Under role `ingested_from`:
//!
//! - the **blob** tier reuses its block digest, which is plain `blake3(file bytes)` — `b3sum` reproduces it;
//! - every **other** source hands over `merkle_root` over the files' digests, whose one-element case is
//!   `leaf_hash(d)` with a `0x00` domain prefix and therefore *not* `d`.
//!
//! And nothing on the edge separates them: a single-file generic ingest has the same one-file shape as a
//! blob and still takes the Merkle path. `the_two_constructions_are_genuinely_different_values` pins that,
//! so if they ever converge this file says so instead of quietly over-claiming.

use std::path::Path;

use tessera_core::export::ro_crate;
use tessera_core::hash::{digest, merkle_root};
use tessera_core::Manifest;

/// The exported PropertyValue description for the edge with `role`, read out of the flattened `@graph`
/// exactly as a consumer would.
fn exported_label(m: &Manifest, role: &str) -> String {
    let ro = ro_crate(m);
    let graph = ro["@graph"].as_array().expect("@graph").clone();
    let edge = m
        .sources
        .iter()
        .find(|s| s.role == role)
        .unwrap_or_else(|| panic!("no `{role}` edge on this product"));
    let urn_hash = edge
        .content_hash
        .as_deref()
        .expect("edge pins a hash")
        .trim_start_matches("blake3:");
    let entity = graph
        .iter()
        .find(|e| {
            e["@id"]
                .as_str()
                .is_some_and(|id| id.contains(urn_hash) || id.starts_with("urn:tessera:source:"))
                && e["identifier"]["@id"].is_string()
        })
        .unwrap_or_else(|| panic!("no edge entity carrying an identifier: {graph:#?}"));
    let pv_id = entity["identifier"]["@id"].as_str().unwrap();
    let pv = graph
        .iter()
        .find(|e| e["@id"] == pv_id)
        .expect("the referenced PropertyValue is a @graph child");
    format!("{} {}", pv["name"], pv["description"]).to_lowercase()
}

fn write(dir: &Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, bytes).unwrap();
    p
}

/// The premise this whole label rests on: the two constructions do **not** coincide, so one label cannot
/// silently stand for both by accident. If this ever fails, the label can and should become specific.
#[test]
fn the_two_constructions_are_genuinely_different_values() {
    let bytes = b"not a real study, just bytes";
    let plain = digest(bytes);
    let one_leaf = merkle_root(std::slice::from_ref(&plain));
    assert_ne!(
        plain, one_leaf,
        "a one-element Merkle root must differ from the digest itself (0x00 leaf domain prefix)"
    );
}

/// **Blob, in memory.** `to_blob_product` reuses the block digest, so the edge value is plain
/// `blake3(file bytes)` — recomputed here independently, which is the `b3sum` claim in the label.
#[test]
fn blob_in_memory_stores_a_plain_file_digest_and_the_label_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = b"\x00\x01\x02 opaque vendor payload \xff\xfe";
    let p = write(dir.path(), "vendor.bin", bytes);

    let (m, _payloads) = tessera_ingest::blob::to_blob_product(
        &p,
        "vendor",
        "2024-01-01T00:00:00Z",
        None,
        None,
        &[],
    )
    .unwrap();

    let stored = m
        .sources
        .iter()
        .find(|s| s.role == "ingested_from")
        .unwrap()
        .content_hash
        .clone()
        .unwrap();
    // Independent recomputation: this is exactly what `b3sum` would print.
    assert_eq!(
        stored,
        digest(bytes),
        "blob stores plain blake3(file bytes)"
    );
    assert_ne!(
        stored,
        merkle_root(&[digest(bytes)]),
        "and it is NOT the Merkle form"
    );

    let label = exported_label(&m, "ingested_from");
    assert!(
        label.contains("b3sum") || label.contains("file's bytes"),
        "the label must cover the plain-digest construction: {label}"
    );
}

/// **Blob, streamed.** A different code path (`blob_ref_streaming`) that must store the same thing — a
/// second producer under the same role, which is the shape that broke the role-only label.
#[test]
fn blob_streaming_stores_the_same_plain_digest() {
    let dir = tempfile::tempdir().unwrap();
    let bytes: Vec<u8> = (0..4096u32).map(|i| (i % 251) as u8).collect();
    let p = write(dir.path(), "big.bin", &bytes);

    // Returns the Manifest alone — the payload is streamed, not held.
    let m = tessera_ingest::blob::to_blob_product_streaming(
        &p,
        "streamed",
        "2024-01-01T00:00:00Z",
        None,
        None,
        &[],
    )
    .unwrap();

    let stored = m
        .sources
        .iter()
        .find(|s| s.role == "ingested_from")
        .unwrap()
        .content_hash
        .clone()
        .unwrap();
    assert_eq!(
        stored,
        digest(&bytes),
        "the streamed path must store the same plain digest as the in-memory one"
    );
    let label = exported_label(&m, "ingested_from");
    assert!(
        label.contains("b3sum") || label.contains("file's bytes"),
        "{label}"
    );
}

/// **A generic source, one file.** The counterexample that rules out picking the label from the number of
/// files: this has a single source file exactly like a blob, yet stores the Merkle form.
#[test]
fn a_single_file_generic_source_still_stores_the_merkle_form() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = b"one file, generic lane";
    let p = write(dir.path(), "one.raw", bytes);

    let stored = tessera_ingest::provenance::source_digest(&[p.as_path()]).unwrap();
    assert_eq!(
        stored,
        merkle_root(&[digest(bytes)]),
        "Merkle over one leaf"
    );
    assert_ne!(
        stored,
        digest(bytes),
        "so a one-file source is NOT interchangeable with the blob construction — which is why the \
         label names both rather than choosing by file count"
    );
}

/// **A generic source, several files.** The Merkle root is over the files' digests in order.
#[test]
fn a_multi_file_generic_source_stores_a_merkle_root_over_file_digests() {
    let dir = tempfile::tempdir().unwrap();
    let a = write(dir.path(), "a.raw", b"first");
    let b = write(dir.path(), "b.raw", b"second");

    let stored = tessera_ingest::provenance::source_digest(&[a.as_path(), b.as_path()]).unwrap();
    assert_eq!(
        stored,
        merkle_root(&[digest(b"first"), digest(b"second")]),
        "Merkle root over the per-file digests, in order"
    );

    let edge =
        tessera_ingest::provenance::ingested_from(&[a.as_path(), b.as_path()], "a.raw,b.raw")
            .unwrap();
    let mut pb = tessera_core::ProductBuilder::new("recon", "s", "d", "2024-01-01T00:00:00Z");
    pb.add_source(edge);
    let m = pb.seal().unwrap();
    let label = exported_label(&m, "ingested_from");
    assert!(
        label.contains("merkle") && label.contains("source files"),
        "the label must cover the Merkle construction: {label}"
    );
}

/// **A lineage edge.** `derived_from` / `supersedes` pin the parent's `manifest_hash`, which is a digest of
/// canonical JSON — recomputed here with the parent's own method, so the label's claim is checked against
/// the real seal rather than against the role name.
#[test]
fn a_lineage_edge_pins_the_parents_manifest_hash_and_the_label_says_manifest() {
    let parent = {
        let mut pb = tessera_core::ProductBuilder::new("raw", "p", "d", "2024-01-01T00:00:00Z");
        pb.add_source(
            tessera_core::provenance::Source::new("ingested_from", "x")
                .with_content_hash("blake3:aa"),
        );
        pb.seal().unwrap()
    };
    let mh = parent.manifest_hash.clone().unwrap();
    assert_eq!(
        mh,
        parent.compute_manifest_hash().unwrap(),
        "the parent's seal is a digest over its canonical bytes"
    );

    let mut pb = tessera_core::ProductBuilder::new("recon", "c", "d", "2024-01-02T00:00:00Z");
    pb.add_source(
        tessera_core::provenance::Source::new("derived_from", &parent.id).with_content_hash(&mh),
    );
    let child = pb.seal().unwrap();

    let label = exported_label(&child, "derived_from");
    assert!(label.contains("manifest"), "{label}");
    assert!(
        !label.contains("merkle"),
        "a lineage edge is not a Merkle root: {label}"
    );
}
