//! Every `tessera-io` door that writes a sealed manifest writes the ADR-0066 stored pair:
//! `manifest.json` is exactly its own JCS form with no `manifest_hash` key, and `manifest.seal` is
//! the 71 bytes `blake3(manifest.json)`, entry 2 of the `.tsra` (rules 1–3). Every reading door
//! refuses a missing or mismatched seal, and the alpha layout (rule 10, no migration).
//!
//! One test per writing door: `pack`, `pack_streaming`, `pack_streaming_verified`,
//! `WriteSession::seal`, `Repository::put_manifest` (`objects/`), `unpack` (the exploded form),
//! `pack_dir` and `add_aux_members` (the door `sign` and ingest provenance use). Each reads what the
//! door wrote from the raw zip members or files, not through `Reader` (whose verification is what
//! the refusal tests hold), and checks the seal is the one the product had: A′ moves where the seal
//! lives, never its value. The last test holds every tracked `.tsra` in the repository to the form.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use tessera_core::{canonical, Error};
use tessera_io::conformance::{fixtures, Fixture};
use tessera_io::{
    add_aux_members, pack, pack_dir, pack_streaming, pack_streaming_verified, unpack, AuxMember,
    Reader, Repository, WriteSession,
};

/// `bytes` and `seal_member` are the stored pair: `bytes` its own JCS form without an in-band
/// seal, and `seal_member` exactly `blake3(bytes)`, which is `seal`, the product's seal.
#[track_caller]
fn assert_stored_pair(door: &str, bytes: &[u8], seal_member: &[u8], seal: &str) {
    let object = canonical::check_canonical(bytes).unwrap_or_else(|e| {
        panic!(
            "{door}: the written manifest.json is not its own JCS form: {e}\n{}",
            String::from_utf8_lossy(&bytes[..bytes.len().min(200)])
        )
    });
    assert!(
        object.get("manifest_hash").is_none(),
        "{door}: the seal is in band"
    );
    assert_eq!(
        seal_member,
        tessera_core::hash::digest(bytes).as_bytes(),
        "{door}: manifest.seal is not blake3(manifest.json)"
    );
    assert_eq!(seal_member, seal.as_bytes(), "{door}: the seal moved");
}

/// The raw `(manifest.json, manifest.seal)` members of a `.tsra`, and that they are entries 2 and 3
/// (after `mimetype`), read with `zip` directly.
fn raw_pair(tsra: &Path) -> (Vec<u8>, Vec<u8>) {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(tsra).unwrap()).unwrap();
    let leading: Vec<String> = (0..3)
        .map(|i| zip.by_index(i).unwrap().name().unwrap().into_owned())
        .collect();
    assert_eq!(
        leading,
        ["mimetype", "manifest.seal", "manifest.json"],
        "{}: SPEC §6's leading entries",
        tsra.display()
    );
    let mut member = |name: &str| {
        let mut out = Vec::new();
        zip.by_name(name).unwrap().read_to_end(&mut out).unwrap();
        out
    };
    (member("manifest.json"), member("manifest.seal"))
}

#[track_caller]
fn assert_tsra(door: &str, tsra: &Path, seal: &str) {
    let (json, seal_member) = raw_pair(tsra);
    assert_stored_pair(door, &json, &seal_member, seal);
}

fn seal(f: &Fixture) -> &str {
    f.manifest
        .manifest_hash
        .as_deref()
        .expect("fixtures are sealed")
}

/// Fixtures with at least one block, so the streaming doors have fragments to stream, plus one with
/// none, so the empty product is covered too.
fn all() -> Vec<Fixture> {
    let all = fixtures();
    assert!(
        all.iter().any(|f| f.payloads.is_empty()) && all.iter().any(|f| !f.payloads.is_empty()),
        "the corpus no longer covers both an empty and a non-empty product"
    );
    all
}

/// Each fixture's payloads written as fragment files under `dir`, for the streaming doors.
fn fragments(f: &Fixture, dir: &Path) -> Vec<(tessera_core::block::BlockName, PathBuf)> {
    f.payloads
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let path = dir.join(format!("{}-{i}.frag", f.name));
            std::fs::write(&path, &p.bytes).unwrap();
            (p.name.clone(), path)
        })
        .collect()
}

#[test]
fn pack_writes_the_stored_pair() {
    let tmp = tempfile::tempdir().unwrap();
    for f in all() {
        let out = tmp.path().join(format!("{}.tsra", f.name));
        pack(&f.manifest, &f.payloads, &out).unwrap();
        assert_tsra(&format!("pack {}", f.name), &out, seal(&f));
    }
}

#[test]
fn pack_streaming_writes_the_stored_pair() {
    let tmp = tempfile::tempdir().unwrap();
    for f in all() {
        let frags = fragments(&f, tmp.path());
        let sources: Vec<_> = frags
            .iter()
            .map(|(n, p)| (n.clone(), p.as_path()))
            .collect();
        let plain = tmp.path().join(format!("{}-s.tsra", f.name));
        pack_streaming(&f.manifest, &sources, &plain).unwrap();
        assert_tsra(&format!("pack_streaming {}", f.name), &plain, seal(&f));
        let verified = tmp.path().join(format!("{}-v.tsra", f.name));
        pack_streaming_verified(&f.manifest, &sources, &verified).unwrap();
        assert_tsra(
            &format!("pack_streaming_verified {}", f.name),
            &verified,
            seal(&f),
        );
    }
}

#[test]
fn write_session_seal_writes_the_stored_pair() {
    let tmp = tempfile::tempdir().unwrap();
    for f in all() {
        let m = &f.manifest;
        let mut ws = WriteSession::create(
            &tmp.path().join(format!("{}-ws", f.name)),
            &m.product,
            &m.name,
            &m.description,
            &m.timestamp,
        )
        .unwrap();
        for (r, p) in m.blocks.iter().zip(&f.payloads) {
            ws.append_block(r.clone(), &p.bytes).unwrap();
        }
        let out = tmp.path().join(format!("{}-ws.tsra", f.name));
        let sealed = ws.seal(&out).unwrap();
        assert_tsra(
            &format!("WriteSession::seal {}", f.name),
            &out,
            sealed.manifest_hash.as_deref().unwrap(),
        );
    }
}

/// The file a repository keeps an object in: `objects/<alg>/<hex[..2]>/<hex[2..]>`.
fn object_file(root: &Path, address: &str) -> PathBuf {
    let (alg, hex) = address.split_once(':').unwrap();
    root.join("objects")
        .join(alg)
        .join(&hex[..2])
        .join(&hex[2..])
}

/// A manifest object is the stored bytes, filed under their own digest: the address is the seal
/// (ADR-0066 makes ADR-0036's content addressing literal). An edited object is refused on read.
#[test]
fn repository_objects_are_the_stored_bytes_under_their_seal() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    let repo = Repository::init(&root).unwrap();
    for f in all() {
        let mh = repo.put_manifest(&f.manifest).unwrap();
        assert_eq!(mh, seal(&f));
        let path = object_file(&root, &mh);
        let bytes = std::fs::read(&path).unwrap();
        assert_stored_pair(
            &format!("put_manifest {}", f.name),
            &bytes,
            mh.as_bytes(),
            seal(&f),
        );
        assert_eq!(repo.get_manifest(&mh).unwrap().manifest_hash, Some(mh));
    }
    // An object whose bytes no longer hash to its address is refused for the seal.
    let f = &all()[0];
    let path = object_file(&root, seal(f));
    // Still its own JCS form (one character of a string value changed), so the seal is what fails.
    let text = std::fs::read_to_string(&path).unwrap();
    let edited = text.replacen("\"name\":\"", "\"name\":\"x", 1);
    assert_ne!(edited, text);
    std::fs::write(&path, edited).unwrap();
    assert!(matches!(
        repo.get_manifest(seal(f)),
        Err(Error::Integrity {
            what: "manifest_hash",
            ..
        })
    ));
}

#[test]
fn unpack_and_pack_dir_write_the_stored_pair() {
    let tmp = tempfile::tempdir().unwrap();
    for f in all() {
        let tsra = tmp.path().join(format!("{}.tsra", f.name));
        pack(&f.manifest, &f.payloads, &tsra).unwrap();
        let exploded = tmp.path().join(format!("{}-exploded", f.name));
        unpack(&tsra, &exploded).unwrap();
        assert_stored_pair(
            &format!("unpack {}", f.name),
            &std::fs::read(exploded.join("manifest.json")).unwrap(),
            &std::fs::read(exploded.join("manifest.seal")).unwrap(),
            seal(&f),
        );
        let packed = tmp.path().join(format!("{}-dir.tsra", f.name));
        pack_dir(&exploded, &packed).unwrap();
        assert_tsra(&format!("pack_dir {}", f.name), &packed, seal(&f));
        // The round trip copies the members byte for byte.
        assert_eq!(raw_pair(&packed), raw_pair(&tsra), "{}", f.name);
    }
}

/// The door `sign` and the ingest provenance stamp use (#931 item 2): adding `aux/` members copies
/// the stored pair through byte for byte, so it can only re-emit what `Reader` accepts.
#[test]
fn add_aux_members_keeps_the_stored_pair() {
    let tmp = tempfile::tempdir().unwrap();
    for f in all() {
        let tsra = tmp.path().join(format!("{}.tsra", f.name));
        pack(&f.manifest, &f.payloads, &tsra).unwrap();
        let before = raw_pair(&tsra);
        add_aux_members(&tsra, &[AuxMember::new("provenance.json", b"{}".to_vec())]).unwrap();
        assert_tsra(&format!("add_aux_members {}", f.name), &tsra, seal(&f));
        assert_eq!(raw_pair(&tsra), before, "{}", f.name);
    }
}

/// `f` packed, then rewritten with `edit` applied to its members (name → bytes, `None` drops the
/// member), entry order kept, the way a hostile or broken writer would leave it.
fn rewritten(f: &Fixture, path: &Path, edit: impl Fn(&str, Vec<u8>) -> Option<Vec<u8>>) {
    use std::io::Write as _;
    pack(&f.manifest, &f.payloads, path).unwrap();
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let mut members = Vec::new();
    for i in 0..zip.len() {
        let mut m = zip.by_index(i).unwrap();
        let name = m.name().unwrap().into_owned();
        let mut bytes = Vec::new();
        m.read_to_end(&mut bytes).unwrap();
        if let Some(bytes) = edit(&name, bytes) {
            members.push((name, bytes));
        }
    }
    let stored = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .last_modified_time(zip::DateTime::default())
        .large_file(true)
        .system(zip::System::Unix);
    let mut zw = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    for (name, bytes) in members {
        zw.start_file(name, stored).unwrap();
        zw.write_all(&bytes).unwrap();
    }
    zw.finish().unwrap();
}

/// Every reading door refuses a product without its seal (the alpha layout) and one whose seal is
/// not the digest of its manifest: `Reader` (so every verb over a `.tsra`), `pack_dir` over an
/// exploded directory, and the doors built on them (`unpack`, `add_aux_members`).
#[test]
fn a_missing_or_mismatched_seal_is_refused_by_every_reading_door() {
    let tmp = tempfile::tempdir().unwrap();
    let f = all().into_iter().find(|f| !f.payloads.is_empty()).unwrap();
    let other = format!("blake3:{}", "0".repeat(64));
    let alpha = |e: Error| matches!(e, Error::UnsupportedLayout { .. });
    let mismatch = |e: Error| {
        matches!(
            e,
            Error::Integrity {
                what: "manifest_hash",
                ..
            }
        )
    };

    let no_seal = tmp.path().join("no-seal.tsra");
    rewritten(&f, &no_seal, |name, b| {
        (name != "manifest.seal").then_some(b)
    });
    let wrong_seal = tmp.path().join("wrong-seal.tsra");
    rewritten(&f, &wrong_seal, |name, b| {
        Some(if name == "manifest.seal" {
            other.clone().into_bytes()
        } else {
            b
        })
    });
    for (path, refused) in [
        (&no_seal, &alpha as &dyn Fn(Error) -> bool),
        (&wrong_seal, &mismatch),
    ] {
        let name = path.display().to_string();
        assert!(refused(Reader::open(path).err().unwrap()), "{name}: open");
        let out = tmp.path().join("exploded");
        assert!(refused(unpack(path, &out).unwrap_err()), "{name}: unpack");
        assert!(
            refused(add_aux_members(path, &[AuxMember::new("x", vec![])]).unwrap_err()),
            "{name}: add_aux_members"
        );
    }

    // The exploded form: `pack_dir` refuses a directory without manifest.seal, or with another.
    let tsra = tmp.path().join("ok.tsra");
    pack(&f.manifest, &f.payloads, &tsra).unwrap();
    let dir = tmp.path().join("dir");
    unpack(&tsra, &dir).unwrap();
    let out = tmp.path().join("out.tsra");
    std::fs::write(dir.join("manifest.seal"), &other).unwrap();
    assert!(
        mismatch(pack_dir(&dir, &out).unwrap_err()),
        "pack_dir: wrong"
    );
    std::fs::remove_file(dir.join("manifest.seal")).unwrap();
    assert!(
        alpha(pack_dir(&dir, &out).unwrap_err()),
        "pack_dir: missing"
    );
    assert!(!out.exists(), "a refused pack_dir writes nothing");
}

/// ADR-0066 rule 10: a product written before the cutover (this fixture is `empty_no_blocks.tsra`
/// as `origin/dev` stored it before #914: its `manifest.json` with the in-band `manifest_hash`, no
/// `manifest.seal`) is refused by `Reader`, with an error naming the format it found and the format
/// required. There is no migration verb.
#[test]
fn an_alpha_product_from_before_the_cutover_is_refused_naming_both_formats() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/alpha_layout.tsra");
    let e = Reader::open(&fixture)
        .err()
        .expect("an alpha product is refused");
    let msg = e.to_string();
    for needle in [
        "pre-ADR-0066 (alpha) layout",
        "tessera_version \"0.0.0\"",
        "there is no `manifest.seal`",
        "in-band `manifest_hash`",
        "requires the ADR-0066 layout",
        "stored as exactly its RFC 8785 JCS bytes",
        "no migration",
    ] {
        assert!(msg.contains(needle), "{needle:?} missing from: {msg}");
    }
}

/// Every `.tsra` tracked in the repository (the conformance corpus and the test fixtures) is stored
/// in the A′ layout, under the seal it verifies to.
#[test]
fn every_tracked_tsra_stores_the_stored_pair() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let tracked = [
        "../corpus/files/blob_opaque.tsra",
        "../corpus/files/empty_no_blocks.tsra",
        "../corpus/files/listmode_events.tsra",
        "../corpus/files/multiblock_study.tsra",
        "../corpus/files/recon_float32_mumap.tsra",
        "../corpus/files/recon_int16.tsra",
        "../corpus/files/segmentation_uint8.tsra",
        "../corpus/files/spectrum_lifetime.tsra",
        "tessera-cli/tests/cmd/logical_tables.in/scan.tsra",
        "tessera-cli/tests/cmd/persona_signing.in/study.tsra",
        "tessera-cli/tests/cmd/versioning.in/study.tsra",
        "tessera-io/tests/fixtures/legacy_flat_events.tsra",
        "tessera-py/tests/fixtures/partitioned_pairs.tsra",
    ];
    for rel in tracked {
        let path = crates.join(rel);
        let r = Reader::open(&path).unwrap_or_else(|e| panic!("{rel}: {e}"));
        let mh = r.manifest().manifest_hash.clone().unwrap();
        assert_tsra(rel, &path, &mh);
    }
}
