//! STORED-only: every entry that every `.tsra` writer emits uses `CompressionMethod::Stored`, checked
//! in a build where a deflate codec IS compiled into `zip` (ADR-0022; #468, #588).
//!
//! The container's range-readability rests on its entries being stored verbatim. That used to hold
//! partly by accident: the workspace pinned `zip = { default-features = false }`, so no compressor
//! existed to pick. `tessera-ingest`'s `.npz` reader turns deflate on, and cargo unifies features
//! workspace-wide, so in any real build the writer *could* compress. This test reproduces that
//! condition on its own (the dev-dependency in `Cargo.toml` enables `deflate-flate2-zlib-rs`), proves
//! the codec really is present, and then checks every writer path at two levels: the method `zip`
//! reports from the central directory, and the raw method field of both the central-directory record
//! and the local file header in the bytes on disk (the two must agree; a reader trusts the former).

use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use tessera_core::block::BlockName;
use tessera_io::conformance::{fixtures, Fixture};
use tessera_io::{
    add_aux_members, pack, pack_dir, pack_streaming, pack_streaming_verified, unpack,
    write_aux_members, AuxMember,
};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

const LOCAL_HEADER_SIG: u32 = 0x0403_4b50;
const CENTRAL_HEADER_SIG: u32 = 0x0201_4b50;
/// Byte offset of the compression-method field (APPNOTE 4.3.7 local header, 4.3.12 central header).
const LOCAL_METHOD_AT: u64 = 8;
const CENTRAL_METHOD_AT: u64 = 10;
/// APPNOTE 4.4.5: method 0 = stored (no compression).
const METHOD_STORED: u16 = 0;

fn read_u16_at(f: &mut std::fs::File, at: u64) -> u16 {
    let mut b = [0u8; 2];
    f.seek(SeekFrom::Start(at)).unwrap();
    f.read_exact(&mut b).unwrap();
    u16::from_le_bytes(b)
}

fn read_u32_at(f: &mut std::fs::File, at: u64) -> u32 {
    let mut b = [0u8; 4];
    f.seek(SeekFrom::Start(at)).unwrap();
    f.read_exact(&mut b).unwrap();
    u32::from_le_bytes(b)
}

/// Assert every entry of the archive at `path` is STORED, from `zip`'s view and in the raw bytes.
/// Every `.tsra` carries at least `mimetype` and `manifest.json`, so an archive with fewer entries
/// means the check would pass vacuously and is refused.
fn assert_all_stored(path: &Path) {
    let mut archive = ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    assert!(
        archive.len() >= 2,
        "{}: only {} entries",
        path.display(),
        archive.len()
    );
    let mut raw = std::fs::File::open(path).unwrap();
    for i in 0..archive.len() {
        let e = archive.by_index_raw(i).unwrap();
        let name = e.name().unwrap().into_owned();
        assert_eq!(
            e.compression(),
            CompressionMethod::Stored,
            "{}: entry '{name}' is not STORED (central directory, as zip reads it)",
            path.display()
        );
        let (local, central) = (e.header_start(), e.central_header_start());
        drop(e);
        assert_eq!(
            read_u32_at(&mut raw, local),
            LOCAL_HEADER_SIG,
            "'{name}' local header"
        );
        assert_eq!(
            read_u16_at(&mut raw, local + LOCAL_METHOD_AT),
            METHOD_STORED,
            "{}: entry '{name}' local header method is not STORED",
            path.display()
        );
        assert_eq!(
            read_u32_at(&mut raw, central),
            CENTRAL_HEADER_SIG,
            "'{name}' central header"
        );
        assert_eq!(
            read_u16_at(&mut raw, central + CENTRAL_METHOD_AT),
            METHOD_STORED,
            "{}: entry '{name}' central-directory method is not STORED",
            path.display()
        );
    }
}

/// Write each payload of `f` to its own fragment file, for the streaming packers.
fn fragments(f: &Fixture, dir: &Path) -> Vec<(BlockName, PathBuf)> {
    std::fs::create_dir_all(dir).unwrap();
    f.payloads
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let frag = dir.join(format!("frag{i}"));
            std::fs::write(&frag, &p.bytes).unwrap();
            (p.name.clone(), frag)
        })
        .collect()
}

/// The precondition this test exists for: deflate is compiled into the `zip` this build links. If
/// this fails, the dev-dependency lost its feature and the STORED assertions below prove nothing
/// about the unified-feature build.
#[test]
fn deflate_is_compiled_into_this_build() {
    let mut buf = Cursor::new(Vec::new());
    let mut w = ZipWriter::new(&mut buf);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    w.start_file("x", deflated).unwrap();
    w.write_all(&[0u8; 4096]).unwrap();
    w.finish().unwrap();
    let mut a = ZipArchive::new(buf).unwrap();
    let mut e = a.by_index(0).unwrap();
    assert_eq!(e.compression(), CompressionMethod::Deflated);
    let mut out = Vec::new();
    e.read_to_end(&mut out).unwrap();
    assert_eq!(out, vec![0u8; 4096], "the deflate round-trip must decode");
}

#[test]
fn every_writer_emits_only_stored_entries() {
    let dir = tempfile::tempdir().unwrap();
    let all = fixtures();
    assert!(
        !all.is_empty(),
        "no conformance fixtures: the loop below would prove nothing"
    );
    for f in all {
        let d = dir.path().join(f.name);
        let frags = fragments(&f, &d.join("frags"));
        let sources: Vec<(BlockName, &Path)> = frags
            .iter()
            .map(|(n, p)| (n.clone(), p.as_path()))
            .collect();

        let packed = d.join("pack.tsra");
        pack(&f.manifest, &f.payloads, &packed).unwrap();
        assert_all_stored(&packed);

        let streamed = d.join("stream.tsra");
        pack_streaming(&f.manifest, &sources, &streamed).unwrap();
        assert_all_stored(&streamed);

        let verified = d.join("verified.tsra");
        pack_streaming_verified(&f.manifest, &sources, &verified).unwrap();
        assert_all_stored(&verified);

        // The aux rewrite re-emits every sealed entry, so it is a writer in its own right.
        let aux = d.join("aux.tsra");
        std::fs::copy(&packed, &aux).unwrap();
        add_aux_members(
            &aux,
            &[
                AuxMember::new("signatures/a.sig.json", b"{}".to_vec()),
                AuxMember::new("note.txt", vec![b'z'; 4096]),
            ],
        )
        .unwrap();
        assert_all_stored(&aux);
        write_aux_members(
            &aux,
            &[AuxMember::new("signatures/b.sig.json", b"{}".to_vec())],
            &["signatures/"],
        )
        .unwrap();
        assert_all_stored(&aux);

        let exploded = d.join("exploded");
        unpack(&aux, &exploded).unwrap();
        let repacked = d.join("pack_dir.tsra");
        pack_dir(&exploded, &repacked).unwrap();
        assert_all_stored(&repacked);
    }
}
