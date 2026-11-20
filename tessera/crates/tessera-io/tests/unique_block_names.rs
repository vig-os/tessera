//! #734: block names are unique within a product (ADR-0063 amendment, SPEC §5). A manifest that
//! lists one name twice is refused by verification before any digest lookup, and no writer seals or
//! packs one.
//!
//! The crafted archives below are what a hostile producer builds by hand: a canonical `.tsra` whose
//! manifest lists block `data` twice and is properly re-sealed over that list. Before the fix the
//! reader resolved a name to its first listed digest and collapsed the listed names to a set, so the
//! second digest was never checked against any bytes and `verify` reported `OK (2 blocks)`.

use std::io::{Cursor, Write};
use std::path::Path;

use tessera_core::block::BlockRef;
use tessera_core::manifest::Manifest;
use tessera_core::{Error, ProductBuilder};
use tessera_io::{
    blob_block, pack, pack_streaming, verify_payloads_parallel, BlockPayload, Reader,
};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

const TS: &str = "2024-01-01T00:00:00Z";

/// One blob block `data` and its payload.
fn data_block() -> (BlockRef, BlockPayload) {
    blob_block("data", "data.bin", None, b"THE BYTES".to_vec()).unwrap()
}

/// A sealed one-block manifest whose block list then gets `second` appended and is re-sealed over
/// the new list (content hash and seal both recomputed), as a hostile producer would.
fn resealed_with(second: BlockRef) -> (Manifest, BlockPayload) {
    let (r, p) = data_block();
    let mut b = ProductBuilder::new("blob", "dup", "duplicate listing", TS);
    b.add_block_ref(r);
    let mut m = b.seal().unwrap();
    m.blocks.push(second);
    m.content_hash = Some(m.recompute_content_hash().unwrap());
    m.manifest_hash = Some(m.compute_manifest_hash().unwrap());
    (m, p)
}

/// Case v01 of the #734 report: the second `data` carries the digest of bytes the archive does not hold.
fn listed_twice_with_a_foreign_digest() -> (Manifest, BlockPayload) {
    let (mut second, _) = data_block();
    second.digest = Some(tessera_core::hash::digest(b"NOT THE BYTES"));
    resealed_with(second)
}

/// The same name listed twice with the same, correct digest: nothing is unverified, but the list is
/// still not a set of names, and the Merkle root counts the block twice.
fn listed_twice_with_its_own_digest() -> (Manifest, BlockPayload) {
    resealed_with(data_block().0)
}

/// The archive tessera's writer would emit for `m` (STORED, zip64, Unix, epoch mtime), entry by
/// entry, so it passes every canonical-layout check and only the duplicate listing is wrong.
fn canonical_tsra(m: &Manifest, payload: &BlockPayload) -> Vec<u8> {
    let o = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .last_modified_time(zip::DateTime::default())
        .large_file(true)
        .system(zip::System::Unix);
    let mut w = ZipWriter::new(Cursor::new(Vec::new()));
    // The stored pair by hand: `to_stored` would refuse the duplicate listing (it verifies).
    let json = m.canonical_bytes().unwrap();
    let seal = tessera_core::hash::digest(&json);
    let entries: [(String, Vec<u8>); 4] = [
        ("mimetype".into(), tessera_io::MIMETYPE.as_bytes().to_vec()),
        ("manifest.seal".into(), seal.into_bytes()),
        ("manifest.json".into(), json),
        (format!("blocks/{}", payload.name), payload.bytes.clone()),
    ];
    for (name, bytes) in entries {
        w.start_file(name, o).unwrap();
        w.write_all(&bytes).unwrap();
    }
    w.finish().unwrap().into_inner()
}

fn is_duplicate(e: &Error, name: &str) -> bool {
    matches!(e, Error::DuplicateBlockName { name: n } if n == name)
}

/// Every reader door refuses both crafted archives with the typed error: the open (and so every
/// read), the serial verify and the parallel verify.
#[test]
fn verify_refuses_a_manifest_listing_a_block_twice() {
    for (case, (m, p)) in [
        ("foreign digest", listed_twice_with_a_foreign_digest()),
        ("own digest", listed_twice_with_its_own_digest()),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dup.tsra");
        std::fs::write(&path, canonical_tsra(&m, &p)).unwrap();

        let serial = Reader::open(&path).and_then(|mut r| r.verify_payloads("dup").map(|_| ()));
        let e = serial.expect_err(case);
        assert!(is_duplicate(&e, "data"), "{case}: {e}");

        let e = verify_payloads_parallel(&path, "dup", 4).expect_err(case);
        assert!(is_duplicate(&e, "data"), "{case}: {e}");

        let e = m.verify().expect_err(case);
        assert!(is_duplicate(&e, "data"), "{case}: {e}");
    }
}

/// The packers refuse a sealed manifest that lists a name twice, before creating the output: no
/// writer emits what verification refuses.
#[test]
fn pack_and_pack_streaming_refuse_a_manifest_listing_a_block_twice() {
    let (m, p) = listed_twice_with_its_own_digest();
    let dir = tempfile::tempdir().unwrap();

    let out = dir.path().join("packed.tsra");
    let twice = [
        BlockPayload::new(p.name.clone(), p.bytes.clone()),
        BlockPayload::new(p.name.clone(), p.bytes.clone()),
    ];
    let e = pack(&m, &twice, &out).unwrap_err();
    assert!(is_duplicate(&e, "data"), "{e}");
    assert!(
        !out.exists(),
        "pack must refuse before creating the archive"
    );

    let frag = dir.path().join("frag");
    std::fs::write(&frag, &p.bytes).unwrap();
    let out = dir.path().join("streamed.tsra");
    let sources = [
        (p.name.clone(), frag.as_path()),
        (p.name.clone(), frag.as_path()),
    ];
    let e = pack_streaming(&m, &sources, &out).unwrap_err();
    assert!(is_duplicate(&e, "data"), "{e}");
    assert!(!out.exists() && !Path::new(&format!("{}.part", out.display())).exists());
}
