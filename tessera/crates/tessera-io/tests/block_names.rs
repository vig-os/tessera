//! #685 / ADR-0063: a block name is validated at every door, and no name reaches a path outside the
//! base directory it is joined onto — the write session's `blocks/`, `unpack`'s output, `pack_dir`'s
//! input. Every refusal is asserted on the filesystem too: the whole tree under the test's root is
//! the same after the refused call as before it, so nothing was created anywhere, inside or out.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use tessera_core::block::array::ArraySpec;
use tessera_core::block::name::NameError;
use tessera_core::block::{BlockName, BlockRef};
use tessera_core::manifest::Manifest;
use tessera_core::{Error, ProductBuilder};
use tessera_io::array::{array_block, ArrayData};
use tessera_io::{
    blob_block, pack, pack_dir, unpack, write_aux_members, AuxMember, BlockPayload, Reader,
    WriteSession,
};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

const TS: &str = "2024-01-01T00:00:00Z";

/// Every bad shape #685 names. Each one, joined onto a base directory by `Path::join`, either
/// escapes it (`../`, an absolute path, a Windows drive), aliases it (`.`, empty, trailing `/`) or is
/// not a name a filesystem should be handed (NUL, `\`, a space, non-ASCII).
const BAD: &[&str] = &[
    "",
    ".",
    "..",
    "../escape",
    "../../escape",
    "a/../../escape",
    "a/./b",
    "/escape",
    "/",
    "trailing/",
    "a//b",
    "a\\b",
    "..\\escape",
    "nul\0byte",
    "C:",
    "C:\\escape",
    "has space",
    "é",
    "a*b",
];

fn too_long() -> String {
    "a".repeat(256)
}

fn all_bad() -> Vec<String> {
    BAD.iter()
        .map(|s| s.to_string())
        .chain([too_long()])
        .collect()
}

/// Every path under `root` (files and directories) with each file's bytes: the whole observable
/// state a refused call must leave untouched.
fn tree(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn walk(dir: &Path, root: &Path, out: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            let rel = p.strip_prefix(root).unwrap().to_path_buf();
            if p.is_dir() {
                out.insert(rel, None);
                walk(&p, root, out);
            } else {
                out.insert(rel, Some(std::fs::read(&p).unwrap()));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

fn volume() -> (ArraySpec, ArrayData) {
    let mut spec = ArraySpec::new(vec![4, 4, 4], "int16");
    spec.codec = "pcodec".into();
    (spec, ArrayData::I16((0..64).collect()))
}

fn good_block(name: &str) -> (BlockRef, BlockPayload) {
    let (spec, data) = volume();
    array_block(name, &spec, &data).unwrap()
}

fn is_invalid_name(e: &Error) -> bool {
    matches!(e, Error::InvalidBlockName { .. })
}

fn is_conflict(e: &Error, parent: &str, child: &str) -> bool {
    matches!(e, Error::BlockNameConflict { parent: p, child: c } if p == parent && c == child)
}

/// A bad name is refused where a `BlockRef` is made — every builder, and deserialization (a journal
/// line, a manifest, a caller's JSON) — so neither append path can be handed one: the session's
/// directory and everything around it stay untouched.
#[test]
fn a_bad_name_never_becomes_a_block_ref() {
    let root = tempfile::tempdir().unwrap();
    let stage = root.path().join("stage");
    let mut ws = WriteSession::create(&stage, "recon", "p", "d", TS).unwrap();
    let before = tree(root.path());
    let (spec, data) = volume();
    for bad in all_bad() {
        let e = array_block(&bad, &spec, &data).err().expect("refused");
        assert!(is_invalid_name(&e), "array_block({bad:?}): {e}");
        let e = blob_block(&bad, "f.bin", None, vec![1, 2, 3])
            .err()
            .expect("refused");
        assert!(is_invalid_name(&e), "blob_block({bad:?}): {e}");
        let e = BlockName::new(bad.clone()).unwrap_err();
        assert!(is_invalid_name(&e), "{bad:?}");
        let json =
            serde_json::json!({ "name": bad, "kind": "array", "digest": "blake3:00", "spec": {} });
        let e = serde_json::from_value::<BlockRef>(json).unwrap_err();
        assert!(e.to_string().contains("invalid block name"), "{bad:?}: {e}");
    }
    assert_eq!(
        tree(root.path()),
        before,
        "a refused name must create nothing"
    );
    // The session is unharmed: a good block still commits.
    let (r, p) = good_block("volume");
    ws.append_block(r, &p.bytes).unwrap();
}

/// The leaf rule at both append paths: a name that is the parent or the child of a committed one is
/// refused with the typed conflict, and leaves no fragment, no directory and no journal line.
#[test]
fn both_append_paths_refuse_a_leaf_rule_conflict() {
    let root = tempfile::tempdir().unwrap();
    let stage = root.path().join("stage");
    let mut ws = WriteSession::create(&stage, "recon", "p", "d", TS).unwrap();
    let (r, p) = good_block("volume");
    ws.append_block(r, &p.bytes).unwrap();
    let (r, p) = good_block("pyr/0");
    ws.append_block(r, &p.bytes).unwrap();
    let before = tree(root.path());

    // A child of a committed leaf, through the in-RAM path…
    let (r, p) = good_block("volume/1");
    let e = ws.append_block(r, &p.bytes).unwrap_err();
    assert!(is_conflict(&e, "volume", "volume/1"), "{e}");
    // …and through the streaming path (`append_block_from` → `append_staged`).
    let (r, p) = good_block("volume/1");
    let e = ws.append_block_from(r, p.bytes.as_slice()).unwrap_err();
    assert!(is_conflict(&e, "volume", "volume/1"), "{e}");
    // The parent of a committed hierarchical name, both paths.
    let (r, p) = good_block("pyr");
    let e = ws.append_block(r, &p.bytes).unwrap_err();
    assert!(is_conflict(&e, "pyr", "pyr/0"), "{e}");
    let (r, p) = good_block("pyr");
    let staged = {
        let mut s = ws.staging().create().unwrap();
        s.write_all(&p.bytes).unwrap();
        s
    };
    let e = ws.append_staged(r, staged).unwrap_err();
    assert!(is_conflict(&e, "pyr", "pyr/0"), "{e}");

    assert_eq!(
        tree(root.path()),
        before,
        "a refused append must leave no trace"
    );
    assert_eq!(ws.watermark(), 2);
}

/// A hierarchical name commits through both append paths, its fragment lands under `blocks/` as a
/// subdirectory, recovery finds it, and the seal is byte-identical to a batch pack.
#[test]
fn hierarchical_names_stream_recover_and_seal() {
    let root = tempfile::tempdir().unwrap();
    let stage = root.path().join("stage");
    let (r0, p0) = good_block("volume/0");
    let (r1, p1) = good_block("volume/1");
    {
        let mut ws = WriteSession::create(&stage, "recon", "p", "d", TS).unwrap();
        ws.append_block(r0.clone(), &p0.bytes).unwrap();
        ws.append_block_from(r1.clone(), p1.bytes.as_slice())
            .unwrap();
    }
    assert!(stage.join("blocks/volume/0").is_file());
    assert!(stage.join("blocks/volume/1").is_file());
    let ws = WriteSession::recover(&stage).unwrap();
    assert_eq!(ws.watermark(), 2);
    let streamed = root.path().join("streamed.tsra");
    let sealed = ws.seal(&streamed).unwrap();

    let mut b = ProductBuilder::new("recon", "p", "d", TS);
    b.add_block_ref(r0);
    b.add_block_ref(r1);
    let batch = b.seal().unwrap();
    assert_eq!(sealed.manifest_hash, batch.manifest_hash);
    let batch_path = root.path().join("batch.tsra");
    pack(&batch, &[p0, p1], &batch_path).unwrap();
    assert_eq!(
        std::fs::read(&streamed).unwrap(),
        std::fs::read(&batch_path).unwrap()
    );
}

/// A complete journal line with a bad name is refused with the typed error and never followed: the
/// stage dir (journal included) and everything around it stay byte-identical.
#[test]
fn recover_refuses_a_journal_line_with_a_bad_name() {
    for bad in all_bad() {
        let root = tempfile::tempdir().unwrap();
        let stage = root.path().join("stage");
        {
            let mut ws = WriteSession::create(&stage, "recon", "p", "d", TS).unwrap();
            let (r, p) = good_block("volume");
            ws.append_block(r, &p.bytes).unwrap();
        }
        // The line a hostile or corrupt journal would carry — a well-formed commit but for its name —
        // with a file where the name points, so a recover that followed it would find a "fragment".
        let journal = stage.join("journal.jsonl");
        let mut line: serde_json::Value = serde_json::from_str(
            std::fs::read_to_string(&journal)
                .unwrap()
                .lines()
                .next()
                .unwrap(),
        )
        .unwrap();
        line["name"] = serde_json::Value::String(bad.clone());
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&journal)
            .unwrap();
        writeln!(f, "{line}").unwrap();
        std::fs::write(root.path().join("escape"), b"outside").unwrap();
        let before = tree(root.path());

        let e = WriteSession::recover(&stage).err().expect("must refuse");
        match &e {
            Error::InvalidBlockName { name, .. } => assert_eq!(name, &bad),
            other => panic!("{bad:?}: expected InvalidBlockName, got {other}"),
        }
        assert_eq!(
            tree(root.path()),
            before,
            "{bad:?}: recover must change nothing"
        );
    }
}

/// A journal whose lines break the leaf rule is refused, not replayed.
#[test]
fn recover_refuses_a_leaf_rule_conflict_in_the_journal() {
    let root = tempfile::tempdir().unwrap();
    let stage = root.path().join("stage");
    {
        let mut ws = WriteSession::create(&stage, "recon", "p", "d", TS).unwrap();
        let (r, p) = good_block("volume");
        ws.append_block(r, &p.bytes).unwrap();
    }
    let journal = stage.join("journal.jsonl");
    let first = std::fs::read_to_string(&journal).unwrap();
    let child = first.replace("\"name\":\"volume\"", "\"name\":\"volume/1\"");
    std::fs::write(&journal, format!("{first}{child}")).unwrap();
    let before = tree(root.path());
    let e = WriteSession::recover(&stage).err().expect("must refuse");
    assert!(is_conflict(&e, "volume", "volume/1"), "{e}");
    assert_eq!(tree(root.path()), before);
}

/// A STORED `.tsra` written entry by entry, with the given manifest JSON — the way a hostile producer
/// (not this crate's writers, which can't emit a bad name) would build one.
fn raw_tsra(path: &Path, manifest_json: &str, entries: &[(String, Vec<u8>)]) {
    let mut zw = ZipWriter::new(std::fs::File::create(path).unwrap());
    let o = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    zw.start_file("mimetype", o).unwrap();
    zw.write_all(tessera_io::MIMETYPE.as_bytes()).unwrap();
    zw.start_file("manifest.json", o).unwrap();
    zw.write_all(manifest_json.as_bytes()).unwrap();
    for (name, bytes) in entries {
        zw.start_file(name.as_str(), o).unwrap();
        zw.write_all(bytes).unwrap();
    }
    zw.finish().unwrap();
}

/// A sealed manifest carrying one block, its JSON with the block's name replaced by `name`.
fn manifest_json_named(name: &str) -> (String, Vec<u8>) {
    let (r, p) = good_block("volume");
    let mut b = ProductBuilder::new("recon", "p", "d", TS);
    b.add_block_ref(r);
    let m = b.seal().unwrap();
    let mut v = serde_json::to_value(&m).unwrap();
    v["blocks"][0]["name"] = serde_json::Value::String(name.to_string());
    (serde_json::to_string_pretty(&v).unwrap(), p.bytes)
}

/// `unpack` of a product naming a block `../escape` (or any bad shape) is refused before anything is
/// written: no output directory, nothing beside it.
#[test]
fn unpack_refuses_a_bad_block_name_and_writes_nothing() {
    for bad in all_bad() {
        let root = tempfile::tempdir().unwrap();
        let tsra = root.path().join("hostile.tsra");
        let (mj, bytes) = manifest_json_named(&bad);
        raw_tsra(&tsra, &mj, &[(format!("blocks/{bad}"), bytes)]);
        let before = tree(root.path());
        let e = unpack(&tsra, &root.path().join("out")).unwrap_err();
        assert!(e.to_string().contains("invalid block name"), "{bad:?}: {e}");
        assert_eq!(
            tree(root.path()),
            before,
            "{bad:?}: unpack must write nothing"
        );
    }
}

/// An `aux/` member that climbs out of `aux/` is refused by `unpack` before anything is written, and
/// no writer of this crate can produce one.
#[test]
fn unpack_and_the_aux_writer_refuse_an_escaping_aux_member() {
    let root = tempfile::tempdir().unwrap();
    let (r, p) = good_block("volume");
    let mut b = ProductBuilder::new("recon", "p", "d", TS);
    b.add_block_ref(r);
    let m = b.seal().unwrap();
    let tsra = root.path().join("hostile.tsra");
    raw_tsra(
        &tsra,
        &m.to_json().unwrap(),
        &[
            ("blocks/volume".into(), p.bytes.clone()),
            ("aux/../../escape".into(), b"x".to_vec()),
        ],
    );
    let before = tree(root.path());
    let e = unpack(&tsra, &root.path().join("out")).unwrap_err();
    assert!(
        matches!(&e, Error::Container(msg) if msg.contains("aux member '../../escape'")),
        "{e}"
    );
    assert_eq!(tree(root.path()), before);

    let good = root.path().join("good.tsra");
    pack(&m, &[p], &good).unwrap();
    let before = tree(root.path());
    for bad in all_bad() {
        let e = write_aux_members(&good, &[AuxMember::new(bad.clone(), b"x".to_vec())], &[])
            .unwrap_err();
        assert!(matches!(e, Error::Container(_)), "{bad:?}: {e}");
    }
    assert_eq!(tree(root.path()), before);
}

/// `pack_dir` of an exploded directory whose manifest names `../secret` neither reads the file
/// outside the directory nor writes an archive.
#[test]
fn pack_dir_refuses_a_bad_block_name() {
    for bad in all_bad() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("exploded");
        std::fs::create_dir_all(dir.join("blocks")).unwrap();
        std::fs::write(root.path().join("secret"), b"not yours").unwrap();
        let (mj, _) = manifest_json_named(&bad);
        std::fs::write(dir.join("manifest.json"), mj).unwrap();
        let before = tree(root.path());
        let e = pack_dir(&dir, &root.path().join("out.tsra")).unwrap_err();
        assert!(e.to_string().contains("invalid block name"), "{bad:?}: {e}");
        assert_eq!(tree(root.path()), before, "{bad:?}");
    }
}

/// A pyramid sealed before ADR-0063 (`volume` + `volume/1`, one name the parent of the other):
/// hand-sealed here, since `ProductBuilder::seal` now refuses the pair.
fn old_style_pyramid(path: &Path) -> Manifest {
    let (spec, data) = volume();
    let (r0, p0) = array_block("volume", &spec, &data).unwrap();
    let (r1, p1) = array_block("volume/1", &spec, &data).unwrap();
    let mut m = Manifest::new("recon", "old-pyramid", "d", TS);
    m.blocks = vec![r0, r1];
    m.content_hash = Some(m.recompute_content_hash().unwrap());
    m.manifest_hash = Some(m.compute_manifest_hash().unwrap());
    pack(&m, &[p0, p1], path).unwrap();
    m
}

#[test]
fn seal_refuses_the_old_pyramid_names() {
    let (r0, _) = good_block("volume");
    let (r1, _) = good_block("volume/1");
    let mut b = ProductBuilder::new("recon", "p", "d", TS);
    b.add_block_ref(r0);
    b.add_block_ref(r1);
    let e = b.seal().unwrap_err();
    assert!(is_conflict(&e, "volume", "volume/1"), "{e}");
}

/// Compatibility: an old pyramid still opens, verifies and reads every block in-zip; exploding it is
/// refused with the typed conflict before anything is written, and so is packing a directory that
/// claims it.
#[test]
fn an_old_style_pyramid_still_reads_but_does_not_explode() {
    let root = tempfile::tempdir().unwrap();
    let tsra = root.path().join("old.tsra");
    let m = old_style_pyramid(&tsra);
    let mut r = Reader::open(&tsra).unwrap();
    r.verify_payloads("old").unwrap();
    for name in ["volume", "volume/1"] {
        r.read_block(name).unwrap();
    }
    assert_eq!(r.block_group("volume"), vec!["volume".to_string()]);
    drop(r);

    // It can still be re-packaged with aux members (the `tessera sign` path), seal untouched.
    write_aux_members(
        &tsra,
        &[AuxMember::new("provenance.json", b"{}".to_vec())],
        &[],
    )
    .unwrap();
    let mut r = Reader::open(&tsra).unwrap();
    assert_eq!(r.manifest().manifest_hash, m.manifest_hash);
    r.read_block("volume/1").unwrap();
    drop(r);

    let before = tree(root.path());
    let e = unpack(&tsra, &root.path().join("out")).unwrap_err();
    assert!(is_conflict(&e, "volume", "volume/1"), "{e}");
    assert_eq!(tree(root.path()), before);

    let dir = root.path().join("exploded");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("manifest.json"), m.to_json().unwrap()).unwrap();
    let e = pack_dir(&dir, &root.path().join("re.tsra")).unwrap_err();
    assert!(is_conflict(&e, "volume", "volume/1"), "{e}");
    assert!(!root.path().join("re.tsra").exists());
}

/// Not stricter than the format: every block name in the conformance corpus, the conformance
/// fixtures, and the names the writers and tests generate validates, and every corpus product obeys
/// the leaf rule (so no golden needs to move).
#[test]
fn every_corpus_and_fixture_name_validates() {
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/files");
    let mut seen = 0;
    for entry in std::fs::read_dir(&corpus).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("tsra") {
            continue;
        }
        let r = Reader::open(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        tessera_core::block::check_leaf_rule(r.manifest().blocks.iter().map(|b| &b.name)).unwrap();
        seen += 1;
    }
    assert!(seen >= 8, "the corpus has 8 products, read {seen}");
    for f in tessera_io::conformance::fixtures() {
        tessera_core::block::check_leaf_rule(f.manifest.blocks.iter().map(|b| &b.name)).unwrap();
    }
    // Names generated by the writers (partitioned tables, chunk-index sidecars, blob series, the
    // pyramid) and used across the tests and docs.
    let generated = [
        "events",
        "events_0000",
        "events_9999",
        "events_3p",
        "volume",
        "volume.cidx",
        "file_0000",
        "data",
        "roi",
        "labels",
        "mask",
        "probability",
        "spectrum",
        "gen-01",
        "DP06-ct",
        "en_0",
        "vol_pcodec",
        "recon/volume",
        "volume/0",
        "volume/12",
        "singles",
        "v",
        "t",
        "a",
    ];
    for n in generated {
        BlockName::new(n).unwrap_or_else(|e| panic!("{n}: {e}"));
    }
    assert_eq!(
        tessera_core::block::name::check_name(&"a".repeat(255)),
        Ok(())
    );
    assert!(matches!(
        tessera_core::block::name::check_name(&too_long()),
        Err(NameError::TooLong { len: 256 })
    ));
}

/// `pack_dir` checks the `aux/` subtree before it writes `out`: an aux file whose relative path the
/// grammar refuses fails the whole call, and no half-packed archive (sealed, but missing its aux
/// members) is left behind.
#[test]
fn pack_dir_refuses_a_bad_aux_name_before_writing_the_archive() {
    let root = tempfile::tempdir().unwrap();
    let (r, p) = good_block("volume");
    let mut b = ProductBuilder::new("recon", "p", "d", TS);
    b.add_block_ref(r);
    let m = b.seal().unwrap();
    let tsra = root.path().join("good.tsra");
    pack(&m, &[p], &tsra).unwrap();
    let dir = root.path().join("exploded");
    unpack(&tsra, &dir).unwrap();
    std::fs::create_dir_all(dir.join("aux")).unwrap();
    std::fs::write(dir.join("aux/my notes.txt"), b"x").unwrap();
    let out = root.path().join("out.tsra");
    let e = pack_dir(&dir, &out).unwrap_err();
    assert!(
        matches!(&e, Error::Container(msg) if msg.contains("aux member 'my notes.txt'")),
        "{e}"
    );
    assert!(
        !out.exists(),
        "a refused pack_dir must not leave an archive"
    );
}

/// Rewriting an archive (`write_aux_members`, which `tessera sign` uses) copies its `blocks/` and
/// `aux/` entries through; a hostile input whose entries climb out of their prefix is refused, not
/// re-emitted, and the input is left as it was.
#[test]
fn the_aux_rewrite_refuses_to_copy_an_escaping_entry_through() {
    let (r, p) = good_block("volume");
    let mut b = ProductBuilder::new("recon", "p", "d", TS);
    b.add_block_ref(r);
    let m = b.seal().unwrap();
    for hostile in [
        "aux/../../escape",
        "blocks/../escape",
        "aux/a//b",
        "blocks/x y",
    ] {
        let root = tempfile::tempdir().unwrap();
        let tsra = root.path().join("hostile.tsra");
        raw_tsra(
            &tsra,
            &m.to_json().unwrap(),
            &[
                ("blocks/volume".into(), p.bytes.clone()),
                (hostile.to_string(), b"x".to_vec()),
            ],
        );
        let before = tree(root.path());
        let e = write_aux_members(
            &tsra,
            &[AuxMember::new("provenance.json", b"{}".to_vec())],
            &[],
        )
        .unwrap_err();
        assert!(
            matches!(&e, Error::Container(msg) if msg.contains(&format!("entry '{hostile}'"))),
            "{hostile}: {e}"
        );
        assert_eq!(tree(root.path()), before, "{hostile}");
    }
}
