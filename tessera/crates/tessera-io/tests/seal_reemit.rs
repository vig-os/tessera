//! A product whose manifest the typed view can't reproduce (a newer-format field, `"study": null`)
//! survives every tessera path that re-emits a product under its seal, and is refused by every path
//! that would re-emit the view instead (#573, SPEC §4, ADR-0066).

use serde_json::Value;
use tessera_core::{Error, Manifest};
use tessera_io::{pack, pack_dir, unpack, Reader, Repository};

/// `empty_no_blocks` from the corpus, with `edit` applied, as a stored pair sealed over its JCS
/// bytes (a writer that models the edit would store exactly this).
fn resealed(edit: impl FnOnce(&mut Value)) -> (Vec<u8>, String) {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../corpus/files/empty_no_blocks.tsra"
    );
    let mut v: Value = serde_json::from_str(Reader::open(path.as_ref()).unwrap().manifest_json())
        .expect("corpus manifest parses");
    edit(&mut v);
    let bytes = tessera_core::canonical::to_bytes(&v).unwrap();
    let seal = tessera_core::hash::digest(&bytes);
    (bytes, seal)
}

fn round_trip((bytes, seal): (Vec<u8>, String)) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("exploded");
    std::fs::create_dir_all(dir.join("blocks")).unwrap();
    tessera_io::stored::write_pair(&dir.join("manifest.json"), &bytes, &seal).unwrap();

    // Both doors copy the stored bytes they verified: every key they arrived with is kept — the
    // newer field, the explicit `null` — so they verify under their seal.
    let packed = tmp.path().join("p.tsra");
    pack_dir(&dir, &packed).unwrap();
    let r = Reader::open(&packed).unwrap();
    assert_eq!(r.manifest_json().as_bytes(), bytes.as_slice());
    assert_eq!(r.manifest().manifest_hash.as_deref(), Some(seal.as_str()));

    let out = tmp.path().join("unpacked");
    unpack(&packed, &out).unwrap();
    assert_eq!(
        std::fs::read(out.join("manifest.json")).unwrap(),
        bytes,
        "unpack must write the stored bytes"
    );
    assert_eq!(
        std::fs::read(out.join("manifest.seal")).unwrap(),
        seal.as_bytes()
    );
}

#[test]
fn newer_field_and_null_products_round_trip_through_pack_dir_and_unpack() {
    round_trip(resealed(|v| {
        v["actors"] = serde_json::json!([{"role": "r"}])
    }));
    round_trip(resealed(|v| v["study"] = Value::Null));
}

#[test]
fn the_view_of_a_newer_product_is_not_packed_or_stored() {
    let (bytes, seal) = resealed(|v| v["actors"] = serde_json::json!([]));
    let m = Manifest::from_stored(&bytes, &seal).unwrap();
    assert_eq!(m.newer_fields(), ["actors"]);
    let tmp = tempfile::tempdir().unwrap();
    let err = pack(&m, &[], &tmp.path().join("x.tsra")).unwrap_err();
    assert!(matches!(err, Error::NewerFormatFields { .. }), "{err}");
    let repo = Repository::init(&tmp.path().join("repo")).unwrap();
    let err = repo.put_manifest(&m).unwrap_err();
    assert!(matches!(err, Error::NewerFormatFields { .. }), "{err}");
}

/// A manifest edited after sealing is not packed under its old seal: a writer can't emit a product
/// that fails its own verification.
#[test]
fn a_manifest_edited_after_sealing_is_not_packed() {
    let (bytes, seal) = resealed(|_| {});
    let mut m = Manifest::from_stored(&bytes, &seal).unwrap();
    m.description = "edited".into();
    let tmp = tempfile::tempdir().unwrap();
    let err = pack(&m, &[], &tmp.path().join("x.tsra")).unwrap_err();
    assert!(matches!(err, Error::Integrity { .. }), "{err}");
    assert!(!tmp.path().join("x.tsra").exists());
}
