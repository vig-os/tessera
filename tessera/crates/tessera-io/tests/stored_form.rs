//! Every `tessera-io` door that writes a sealed `manifest.json` writes exactly `JCS(object)`
//! (ADR-0066 rule 1, step 2, #912), with the in-band `manifest_hash` kept and the seal unchanged.
//!
//! One test per door: `pack`, `pack_streaming`, `pack_streaming_verified`, `WriteSession::seal`,
//! `Repository::put_manifest` (`objects/`), `unpack` (the exploded form) and `pack_dir`. Each
//! asserts `bytes == JCS(parse(bytes))` on what the door wrote and that the bytes still verify to
//! the seal the product had. The last test holds every tracked `.tsra` in the repository to the
//! same form.

use std::path::{Path, PathBuf};

use tessera_core::canonical;
use tessera_core::manifest::{seal_of, Manifest};
use tessera_io::conformance::{fixtures, Fixture};
use tessera_io::{
    pack, pack_dir, pack_streaming, pack_streaming_verified, unpack, Reader, Repository,
    WriteSession,
};

/// `bytes` are their own JCS form, and they verify to `seal`.
#[track_caller]
fn assert_stored_form(door: &str, bytes: &[u8], seal: &str) {
    let text = std::str::from_utf8(bytes).unwrap_or_else(|e| panic!("{door}: not UTF-8: {e}"));
    let jcs = canonical::to_bytes(&canonical::parse_strict(text).unwrap()).unwrap();
    assert!(
        bytes == jcs.as_slice(),
        "{door}: the written document is not its own JCS form (first difference at byte {:?}):\n{}",
        bytes.iter().zip(&jcs).position(|(a, b)| a != b),
        text.chars().take(200).collect::<String>()
    );
    let m = Manifest::from_json_verified(text).unwrap_or_else(|e| panic!("{door}: {e}"));
    assert_eq!(
        m.manifest_hash.as_deref(),
        Some(seal),
        "{door}: the seal moved"
    );
    assert_eq!(
        seal_of(&canonical::parse_strict(text).unwrap()).unwrap(),
        seal
    );
}

fn stored_manifest(tsra: &Path) -> Vec<u8> {
    Reader::open(tsra)
        .unwrap()
        .manifest_json()
        .as_bytes()
        .to_vec()
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
fn pack_writes_the_jcs_form() {
    let tmp = tempfile::tempdir().unwrap();
    for f in all() {
        let out = tmp.path().join(format!("{}.tsra", f.name));
        pack(&f.manifest, &f.payloads, &out).unwrap();
        assert_stored_form(
            &format!("pack {}", f.name),
            &stored_manifest(&out),
            seal(&f),
        );
    }
}

#[test]
fn pack_streaming_writes_the_jcs_form() {
    let tmp = tempfile::tempdir().unwrap();
    for f in all() {
        let frags = fragments(&f, tmp.path());
        let sources: Vec<_> = frags
            .iter()
            .map(|(n, p)| (n.clone(), p.as_path()))
            .collect();
        let plain = tmp.path().join(format!("{}-s.tsra", f.name));
        pack_streaming(&f.manifest, &sources, &plain).unwrap();
        assert_stored_form(
            &format!("pack_streaming {}", f.name),
            &stored_manifest(&plain),
            seal(&f),
        );
        let verified = tmp.path().join(format!("{}-v.tsra", f.name));
        pack_streaming_verified(&f.manifest, &sources, &verified).unwrap();
        assert_stored_form(
            &format!("pack_streaming_verified {}", f.name),
            &stored_manifest(&verified),
            seal(&f),
        );
    }
}

#[test]
fn write_session_seal_writes_the_jcs_form() {
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
        assert_stored_form(
            &format!("WriteSession::seal {}", f.name),
            &stored_manifest(&out),
            sealed.manifest_hash.as_deref().unwrap(),
        );
    }
}

/// The file a repository keeps a manifest object in: `objects/<alg>/<hex[..2]>/<hex[2..]>`. Read
/// directly, because `Repository::read_object` is for block objects (it checks a byte digest, and a
/// manifest object is addressed by its seal).
fn object_file(root: &Path, address: &str) -> PathBuf {
    let (alg, hex) = address.split_once(':').unwrap();
    root.join("objects")
        .join(alg)
        .join(&hex[..2])
        .join(&hex[2..])
}

#[test]
fn repository_objects_hold_the_jcs_form() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    let repo = Repository::init(&root).unwrap();
    for f in all() {
        let mh = repo.put_manifest(&f.manifest).unwrap();
        assert_eq!(mh, seal(&f));
        assert_stored_form(
            &format!("put_manifest {}", f.name),
            &std::fs::read(object_file(&root, &mh)).unwrap(),
            seal(&f),
        );
    }
}

/// `f` as a `.tsra` at `path` whose `manifest.json` is the pretty form every tessera writer stored
/// before #912 (`Manifest::to_sealed_json`), packed by hand: no tessera writer emits it any more.
fn pretty_tsra(f: &Fixture, path: &Path) {
    use std::io::Write as _;
    let stored = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .last_modified_time(zip::DateTime::default())
        .large_file(true)
        .system(zip::System::Unix);
    let mut zw = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    zw.start_file("mimetype", stored).unwrap();
    zw.write_all(tessera_io::MIMETYPE.as_bytes()).unwrap();
    zw.start_file("manifest.json", stored).unwrap();
    zw.write_all(f.manifest.to_sealed_json().unwrap().as_bytes())
        .unwrap();
    for p in &f.payloads {
        zw.start_file(format!("blocks/{}", p.name), stored).unwrap();
        zw.write_all(&p.bytes).unwrap();
    }
    zw.finish().unwrap();
}

/// `unpack` copies a document rather than serialise a typed value: from a `.tsra` whose stored
/// manifest is pretty-printed, it writes the JCS form of what it read, under the same seal.
#[test]
fn unpack_writes_the_jcs_form_of_a_pretty_stored_manifest() {
    let tmp = tempfile::tempdir().unwrap();
    for f in all() {
        let pretty = tmp.path().join(format!("{}-pretty.tsra", f.name));
        pretty_tsra(&f, &pretty);
        assert_ne!(
            stored_manifest(&pretty),
            canonical::to_bytes(
                &canonical::parse_strict(&f.manifest.to_sealed_json().unwrap()).unwrap()
            )
            .unwrap(),
            "{}: the input must be pretty, or this test proves nothing",
            f.name
        );
        let exploded = tmp.path().join(format!("{}-exploded", f.name));
        unpack(&pretty, &exploded).unwrap();
        assert_stored_form(
            &format!("unpack {}", f.name),
            &std::fs::read(exploded.join("manifest.json")).unwrap(),
            seal(&f),
        );
    }
}

/// `pack_dir` copies a document too: from an exploded directory whose `manifest.json` is
/// pretty-printed, it packs the JCS form of what it read, under the same seal. (Unpacking what it
/// wrote checks the round trip; the failing-input case for `unpack` is the test above.)
#[test]
fn unpack_and_pack_dir_write_the_jcs_form_of_what_they_copy() {
    let tmp = tempfile::tempdir().unwrap();
    for f in all() {
        let pretty_dir = tmp.path().join(format!("{}-pretty", f.name));
        std::fs::create_dir_all(pretty_dir.join("blocks")).unwrap();
        for (r, p) in f.manifest.blocks.iter().zip(&f.payloads) {
            let path = r.name.path_under(&pretty_dir.join("blocks")).unwrap();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, &p.bytes).unwrap();
        }
        let pretty = f.manifest.to_sealed_json().unwrap();
        std::fs::write(pretty_dir.join("manifest.json"), &pretty).unwrap();

        let packed = tmp.path().join(format!("{}-dir.tsra", f.name));
        pack_dir(&pretty_dir, &packed).unwrap();
        assert_stored_form(
            &format!("pack_dir {}", f.name),
            &stored_manifest(&packed),
            seal(&f),
        );

        let exploded = tmp.path().join(format!("{}-exploded", f.name));
        unpack(&packed, &exploded).unwrap();
        assert_stored_form(
            &format!("unpack {}", f.name),
            &std::fs::read(exploded.join("manifest.json")).unwrap(),
            seal(&f),
        );
    }
}

/// Every `.tsra` tracked in the repository (the conformance corpus and the test fixtures) stores
/// its manifest in the JCS form, under the seal it verifies to. A fixture written before #912 fails
/// here until it is re-packed.
#[test]
fn every_tracked_tsra_stores_the_jcs_form() {
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
        assert_stored_form(rel, r.manifest_json().as_bytes(), &mh);
    }
}
