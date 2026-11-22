//! #720 round 5: a chunk index (kind `chunk_index`, or a spec with `indexes`, SPEC §5d) indexes
//! another block of its own product. One whose block is absent is a structural defect, in the class
//! of a repeated block name: the next block given that name inherits it, and `tessera stats` served
//! an orphan derived `ct.cidx` (min = max = 7) for an honest `ct` added later, labelled exact.
//!
//! Every reader door refuses it, every writer refuses to produce it, and the error names the index
//! and the block it names.

use std::io::{Cursor, Write};
use std::path::Path;

use tessera_core::block::{BlockClass, BlockRef};
use tessera_core::manifest::Manifest;
use tessera_core::{Error, ProductBuilder};
use tessera_io::{
    blob_block, pack, pack_streaming, verify_payloads_parallel, BlockPayload, Reader, Repository,
};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

const TS: &str = "2024-01-01T00:00:00Z";

/// A chunk-index block `name` whose spec says it `indexes` `target`, of `class`.
fn index(name: &str, target: serde_json::Value, class: BlockClass) -> (BlockRef, BlockPayload) {
    let (mut r, p) = blob_block(name, "x.bin", None, b"INDEX".to_vec()).unwrap();
    r.kind = tessera_core::block::BlockKind::ChunkIndex;
    r.class = class;
    r.spec = serde_json::json!({"recipe": "chunk_index@1", "indexes": target});
    (r, p)
}

/// A sealed manifest of `base` plus `index`, resealed by hand over the list with `ct` dropped (the
/// writers refuse to seal it), as a hostile producer would; and the two payloads.
fn orphan(class: BlockClass) -> (Manifest, Vec<BlockPayload>) {
    let (base, base_p) = blob_block("base", "base.bin", None, b"BASE".to_vec()).unwrap();
    let (ct, _) = blob_block("ct", "ct.bin", None, b"CT".to_vec()).unwrap();
    let (idx, idx_p) = index("ct.cidx", "ct".into(), class);
    let mut b = ProductBuilder::new("blob", "orphan", "orphan index", TS);
    b.add_block_ref(base);
    b.add_block_ref(ct);
    b.add_block_ref(idx);
    let mut m = b.seal().unwrap();
    m.blocks.retain(|b| b.name != "ct");
    m.content_hash = Some(m.recompute_content_hash().unwrap());
    m.manifest_hash = Some(m.compute_manifest_hash().unwrap());
    (m, vec![base_p, idx_p])
}

/// The archive tessera's writer would emit for `m`, entry by entry, so it passes every
/// canonical-layout check and only the block list is wrong.
fn canonical_tsra(m: &Manifest, payloads: &[BlockPayload]) -> Vec<u8> {
    let o = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .last_modified_time(zip::DateTime::default())
        .large_file(true)
        .system(zip::System::Unix);
    let mut w = ZipWriter::new(Cursor::new(Vec::new()));
    // The stored pair by hand: `to_stored` would refuse the dangling index (it verifies).
    let json = m.canonical_bytes().unwrap();
    let seal = tessera_core::hash::digest(&json);
    let mut entries: Vec<(String, Vec<u8>)> = vec![
        ("mimetype".into(), tessera_io::MIMETYPE.as_bytes().to_vec()),
        ("manifest.seal".into(), seal.into_bytes()),
        ("manifest.json".into(), json),
    ];
    for p in payloads {
        entries.push((format!("blocks/{}", p.name), p.bytes.clone()));
    }
    for (name, bytes) in entries {
        w.start_file(name, o).unwrap();
        w.write_all(&bytes).unwrap();
    }
    w.finish().unwrap().into_inner()
}

fn is_dangling(e: &Error, index: &str, target: Option<&str>) -> bool {
    matches!(e, Error::DanglingChunkIndex { index: i, indexes: t }
        if i == index && t.as_deref() == target)
}

/// Every reader door refuses the sealed orphan, derived or primary, with the typed error naming
/// both blocks: the open (and so every read), the serial and the parallel verify, the stored
/// manifest and the struct's own verify.
#[test]
fn every_reader_refuses_an_orphan_chunk_index() {
    for class in [BlockClass::Derived, BlockClass::Primary] {
        let (m, payloads) = orphan(class);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("orphan.tsra");
        std::fs::write(&path, canonical_tsra(&m, &payloads)).unwrap();
        let want = |e: &Error| is_dangling(e, "ct.cidx", Some("ct"));

        let e = Reader::open(&path).err().expect("open refuses");
        assert!(want(&e), "{class:?}: {e}");
        assert!(
            e.to_string().contains("'ct.cidx'") && e.to_string().contains("'ct'"),
            "{e}"
        );
        let e = verify_payloads_parallel(&path, "orphan", 4).unwrap_err();
        assert!(want(&e), "{class:?}: {e}");
        let json = m.canonical_bytes().unwrap();
        let seal = tessera_core::hash::digest(&json);
        let e = Manifest::from_stored(&json, &seal).unwrap_err();
        assert!(want(&e), "{class:?}: {e}");
        let e = m.verify().unwrap_err();
        assert!(want(&e), "{class:?}: {e}");
    }
}

/// An index that names itself, or names nothing, indexes no other block: refused the same way.
#[test]
fn an_index_of_itself_or_of_nothing_is_refused() {
    for (target, want) in [
        (serde_json::json!("ct.cidx"), Some("ct.cidx")),
        (serde_json::json!(5), None),
        (serde_json::Value::Null, None),
    ] {
        let (idx, _) = index("ct.cidx", target.clone(), BlockClass::Derived);
        let mut b = ProductBuilder::new("blob", "self", "self index", TS);
        b.add_block_ref(idx);
        let e = b.seal().unwrap_err();
        assert!(is_dangling(&e, "ct.cidx", want), "{target}: {e}");
    }
    // A kind-`chunk_index` block with no `indexes` at all.
    let (mut idx, _) = index("notes", "x".into(), BlockClass::Primary);
    idx.spec = serde_json::json!({"recipe": "chunk_index@1"});
    let mut b = ProductBuilder::new("blob", "none", "no target", TS);
    b.add_block_ref(idx);
    assert!(is_dangling(&b.seal().unwrap_err(), "notes", None));
}

/// No writer produces one: the seal, both packers (before creating the output) and the
/// repository (before writing any object or ref). A look-alike under another name and class is a
/// chunk index by its `indexes` key, so it is held to the same rule.
#[test]
fn no_writer_produces_an_orphan_chunk_index() {
    let (base, _) = blob_block("base", "base.bin", None, b"BASE".to_vec()).unwrap();
    let (mut look_alike, _) = index("notes", "ct".into(), BlockClass::Primary);
    look_alike.kind = tessera_core::block::BlockKind::Blob;
    let mut b = ProductBuilder::new("blob", "w", "writer", TS);
    b.add_block_ref(base);
    b.add_block_ref(look_alike);
    let e = b.seal().unwrap_err();
    assert!(is_dangling(&e, "notes", Some("ct")), "{e}");

    let (m, payloads) = orphan(BlockClass::Derived);
    let dir = tempfile::tempdir().unwrap();
    let want = |e: &Error| is_dangling(e, "ct.cidx", Some("ct"));

    let out = dir.path().join("packed.tsra");
    let e = pack(&m, &payloads, &out).unwrap_err();
    assert!(want(&e), "{e}");
    assert!(!out.exists(), "pack refuses before creating the archive");

    let frags: Vec<_> = payloads
        .iter()
        .map(|p| {
            let f = dir.path().join(p.name.as_str().replace('.', "_"));
            std::fs::write(&f, &p.bytes).unwrap();
            (p.name.clone(), f)
        })
        .collect();
    let sources: Vec<_> = frags
        .iter()
        .map(|(n, f)| (n.clone(), f.as_path()))
        .collect();
    let out = dir.path().join("streamed.tsra");
    let e = pack_streaming(&m, &sources, &out).unwrap_err();
    assert!(want(&e), "{e}");
    assert!(!out.exists() && !Path::new(&format!("{}.part", out.display())).exists());

    let repo = Repository::init(&dir.path().join("repo")).unwrap();
    let e = repo.put_manifest(&m).unwrap_err();
    assert!(want(&e), "{e}");
    let e = repo.commit(&m.id, &m, &payloads).unwrap_err();
    assert!(want(&e), "{e}");
    assert_eq!(repo.read_ref(&m.id).unwrap(), None);
    let objects = walk(&dir.path().join("repo/objects"));
    assert_eq!(objects, 0, "no object is written");
}

/// The number of files under `dir`.
fn walk(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|e| {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(&p)
            } else {
                1
            }
        })
        .sum()
}
