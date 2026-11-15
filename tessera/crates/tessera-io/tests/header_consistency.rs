//! One archive, one reading: a `.tsra` that verifies must read the same under every zip reader
//! (GHSA-m665-v7pm-7grj, #689; SPEC §6).
//!
//! Every zip entry is described twice: by a **local file header** (LFH) in front of its bytes and by
//! a record in the **central directory** (CD) at the end. `tessera` reads through the central
//! directory, like Python's `zipfile`, `unzip` and `ZipArchive`. A streaming reader (`zip`'s own
//! `ZipStreamReader`, libarchive in streaming mode, Java's `ZipInputStream`) walks the local headers
//! front to back and never consults the central directory. When the two disagree, the same file is two
//! different archives, and the advisory shows how a gateway inspecting one view approves what a worker
//! extracting the other one writes to disk.
//!
//! For an integrity format that is the whole game: `tessera verify` attests what tessera reads, and a
//! verdict that holds for only one family of readers is not a verdict on the file. Each test below crafts
//! one disagreement from a genuinely sealed product, first proves it IS a disagreement (the stream view
//! differs from the central-directory view, with valid CRCs where the stream reader checks them), and
//! then asserts that `verify` refuses the file. Under zip 9.0.4 with no tessera check, every one of
//! them verified.
//!
//! Three of the four are what zip 9.0.5's opt-in header-consistency validation detects. The fourth, a
//! member hidden in bytes the central directory never references, it does not: no header disagrees and
//! nothing overlaps. That one is caught only by tessera's own layout rule, that the entries tile the
//! archive from byte 0 to the central directory with no gap.

use std::io::{Cursor, Read, Write};
use std::path::Path;

use tessera_core::{Error, ProductBuilder};
use tessera_io::conformance::fixtures;
use tessera_io::{
    add_aux_members, blob_block, pack, pack_dir, pack_streaming, pack_streaming_verified, unpack,
    verify_payloads_parallel, AuxMember, Reader,
};
use zip::read::{ZipFile, ZipFileEntry};
use zip::unstable::stream::{ZipStreamReader, ZipStreamVisitor};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

const TS: &str = "2024-01-01T00:00:00Z";

// APPNOTE 4.3.7 (local file header), 4.3.12 (central directory header), 4.3.14-16 (end records).
const LFH_METHOD: usize = 8;
const LFH_CRC: usize = 14;
const LFH_CSIZE: usize = 18;
const LFH_USIZE: usize = 22;
const LFH_NAME_LEN: usize = 26;
const LFH_EXTRA_LEN: usize = 28;
const LFH_NAME: usize = 30;
const CDH_NAME: usize = 46;
const EOCD_SIG: &[u8; 4] = b"PK\x05\x06";
const ZIP64_LOCATOR_SIG: &[u8; 4] = b"PK\x06\x07";
const ZIP64_EOCD_SIG: &[u8; 4] = b"PK\x06\x06";
const EOCD_CD_OFFSET: usize = 16;
const ZIP64_LOCATOR_RECORD_OFFSET: usize = 8;
const ZIP64_EOCD_CD_OFFSET: usize = 48;
const ZIP64_EXTRA_ID: u16 = 0x0001;
const METHOD_DEFLATE: u16 = 8;

// ── building blocks ─────────────────────────────────────────────────────────────────────────────

/// A sealed product of opaque blobs, packed by tessera's own writer, as raw bytes.
fn sealed_blobs(blobs: &[(&str, &[u8])]) -> Vec<u8> {
    let mut b = ProductBuilder::new("blob", "ghsa-m665", "header consistency", TS);
    let mut payloads = Vec::new();
    for (name, bytes) in blobs {
        let (r, p) = blob_block(name, &format!("{name}.bin"), None, bytes.to_vec()).unwrap();
        b.add_block_ref(r);
        payloads.push(p);
    }
    let manifest = b.seal().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("p.tsra");
    pack(&manifest, &payloads, &path).unwrap();
    std::fs::read(&path).unwrap()
}

/// Where each central-directory entry sits: (name, local header offset, central header offset).
fn cd_entries(bytes: &[u8]) -> Vec<(String, usize, usize)> {
    let mut a = ZipArchive::new(Cursor::new(bytes)).unwrap();
    (0..a.len())
        .map(|i| {
            let f = a.by_index_raw(i).unwrap();
            (
                f.name().unwrap().into_owned(),
                f.header_start() as usize,
                f.central_header_start() as usize,
            )
        })
        .collect()
}

fn entry(bytes: &[u8], name: &str) -> (usize, usize) {
    cd_entries(bytes)
        .into_iter()
        .find(|(n, _, _)| n == name)
        .map(|(_, l, c)| (l, c))
        .unwrap_or_else(|| panic!("no entry {name}"))
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(b[at..at + 2].try_into().unwrap())
}
fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}
fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}
fn put_u16(b: &mut [u8], at: usize, v: u16) {
    b[at..at + 2].copy_from_slice(&v.to_le_bytes());
}
fn put_u32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}
fn put_u64(b: &mut [u8], at: usize, v: u64) {
    b[at..at + 8].copy_from_slice(&v.to_le_bytes());
}

/// Overwrite an entry's name in place, in its local header (`lfh`) or central record (`cdh`) —
/// same length only, so no offset moves.
fn rename_at(bytes: &mut [u8], header: usize, name_field: usize, new: &str) {
    let len_field = if name_field == LFH_NAME {
        LFH_NAME_LEN
    } else {
        28
    };
    let len = u16_at(bytes, header + len_field) as usize;
    assert_eq!(len, new.len(), "same-length rename only");
    bytes[header + name_field..header + name_field + len].copy_from_slice(new.as_bytes());
}

/// Set a local header's sizes, in the zip64 extra field when the 32-bit fields say "see zip64" (every
/// `.tsra` writer sets `large_file`, so they do).
fn set_local_sizes(bytes: &mut [u8], lfh: usize, uncompressed: u64, compressed: u64) {
    if u32_at(bytes, lfh + LFH_USIZE) != u32::MAX {
        put_u32(bytes, lfh + LFH_USIZE, uncompressed as u32);
        put_u32(bytes, lfh + LFH_CSIZE, compressed as u32);
        return;
    }
    let name_len = u16_at(bytes, lfh + LFH_NAME_LEN) as usize;
    let extra_len = u16_at(bytes, lfh + LFH_EXTRA_LEN) as usize;
    let mut at = lfh + LFH_NAME + name_len;
    let end = at + extra_len;
    while at + 4 <= end {
        let (id, len) = (u16_at(bytes, at), u16_at(bytes, at + 2) as usize);
        if id == ZIP64_EXTRA_ID {
            put_u64(bytes, at + 4, uncompressed);
            put_u64(bytes, at + 12, compressed);
            return;
        }
        at += 4 + len;
    }
    panic!("local header at {lfh} has no zip64 extra field");
}

/// Insert `inserted` right before the central directory, moving every end-record pointer past it.
fn insert_before_central_directory(bytes: &[u8], inserted: &[u8]) -> Vec<u8> {
    let cd_start = ZipArchive::new(Cursor::new(bytes))
        .unwrap()
        .central_directory_start() as usize;
    let k = inserted.len() as u64;
    let mut out = Vec::with_capacity(bytes.len() + inserted.len());
    out.extend_from_slice(&bytes[..cd_start]);
    out.extend_from_slice(inserted);
    out.extend_from_slice(&bytes[cd_start..]);

    let eocd = out
        .windows(4)
        .rposition(|w| w == EOCD_SIG)
        .expect("end of central directory");
    let cd_offset = u32_at(&out, eocd + EOCD_CD_OFFSET);
    if cd_offset != u32::MAX {
        put_u32(&mut out, eocd + EOCD_CD_OFFSET, cd_offset + k as u32);
    }
    if eocd >= 20 && &out[eocd - 20..eocd - 16] == ZIP64_LOCATOR_SIG {
        let loc = eocd - 20;
        let rec = u64_at(&out, loc + ZIP64_LOCATOR_RECORD_OFFSET) + k;
        put_u64(&mut out, loc + ZIP64_LOCATOR_RECORD_OFFSET, rec);
        let rec = rec as usize;
        assert_eq!(&out[rec..rec + 4], ZIP64_EOCD_SIG, "zip64 end record");
        let cd = u64_at(&out, rec + ZIP64_EOCD_CD_OFFSET) + k;
        put_u64(&mut out, rec + ZIP64_EOCD_CD_OFFSET, cd);
    }
    out
}

/// CRC-32 (IEEE), bitwise: a test needs it for a handful of bytes, and it keeps the crate's dependency
/// set (and its feature snapshot) unchanged.
fn crc32(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in data {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = if c & 1 != 0 {
                (c >> 1) ^ 0xEDB8_8320
            } else {
                c >> 1
            };
        }
    }
    !c
}

/// `inner` as one raw-deflate *stored* block (RFC 1951 §3.2.4): five header bytes, then `inner`
/// verbatim. Read as STORED it is these bytes; read as DEFLATE it decodes to `inner`.
fn deflate_stored_block(inner: &[u8]) -> Vec<u8> {
    let len = u16::try_from(inner.len()).unwrap();
    let mut out = vec![0x01];
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&(!len).to_le_bytes());
    out.extend_from_slice(inner);
    out
}

/// Re-emit `entries` as a `.tsra`-shaped zip (STORED, zip64, `mimetype` first at byte 0).
fn rezip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let opts = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .last_modified_time(zip::DateTime::default())
        .large_file(true);
    let mut w = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in entries {
        w.start_file(*name, opts).unwrap();
        w.write_all(bytes).unwrap();
    }
    w.finish().unwrap().into_inner()
}

// ── the two views ───────────────────────────────────────────────────────────────────────────────

type View = Vec<(String, Vec<u8>)>;

/// What a central-directory reader sees (tessera, `zipfile`, `unzip`).
fn cd_view(bytes: &[u8]) -> View {
    let mut a = ZipArchive::new(Cursor::new(bytes)).unwrap();
    (0..a.len())
        .map(|i| {
            let mut f = a.by_index(i).unwrap();
            let mut b = Vec::new();
            f.read_to_end(&mut b).unwrap();
            (f.name().unwrap().into_owned(), b)
        })
        .collect()
}

/// What a streaming reader sees: the local headers, front to back.
fn stream_view(bytes: &[u8]) -> View {
    struct Collect(View);
    impl ZipStreamVisitor for Collect {
        fn visit_file<R: Read>(&mut self, file: &mut ZipFile<'_, R>) -> zip::result::ZipResult<()> {
            let mut b = Vec::new();
            file.read_to_end(&mut b)?;
            self.0.push((file.name()?.into_owned(), b));
            Ok(())
        }
        fn visit_additional_metadata(
            &mut self,
            _: &ZipFileEntry<'_>,
        ) -> zip::result::ZipResult<()> {
            Ok(())
        }
    }
    let mut c = Collect(Vec::new());
    ZipStreamReader::new(Cursor::new(bytes))
        .visit(&mut c)
        .expect("the stream reader reads the crafted archive");
    c.0
}

fn names(v: &View) -> Vec<&str> {
    v.iter().map(|(n, _)| n.as_str()).collect()
}

// ── the verdict ─────────────────────────────────────────────────────────────────────────────────

/// How tessera judges `bytes`: the first refusal on the full `tessera verify` path (open, then both
/// payload verifiers), or `None` when the file verifies. These fixtures are far below the parallel
/// verifier's size knee, so it takes its serial branch; its fan-out branch shares the same container
/// check, which runs once before the branch is chosen.
fn verdict(bytes: &[u8]) -> Option<Error> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("crafted.tsra");
    std::fs::write(&path, bytes).unwrap();
    let serial = Reader::open(&path).and_then(|mut r| r.verify_payloads("crafted"));
    let parallel = verify_payloads_parallel(&path, "crafted", 4).map(|_| ());
    assert_eq!(
        serial.is_ok(),
        parallel.is_ok(),
        "the serial and parallel verifiers must agree (serial {serial:?}, parallel {parallel:?})"
    );
    serial.err()
}

/// `bytes` must be refused as a container problem, and the message must say what is wrong.
fn assert_refused(bytes: &[u8], why: &str) {
    match verdict(bytes) {
        None => panic!("verified an archive two zip readers read differently ({why})"),
        Some(Error::Container(msg)) => assert!(
            msg.contains("local header") || msg.contains("header inconsistency"),
            "refused for the wrong reason ({why}): {msg}"
        ),
        Some(other) => panic!("refused, but not as a container problem ({why}): {other}"),
    }
}

// ── the four disagreements ──────────────────────────────────────────────────────────────────────

/// The advisory's own shape: a block whose local header carries another name. A streaming reader sees
/// a second `manifest.json` (the block's bytes), next to the real one.
#[test]
fn a_block_whose_local_header_names_another_member_is_refused() {
    let honest = sealed_blobs(&[("report", b"quarterly numbers")]);
    assert!(verdict(&honest).is_none(), "the honest product verifies");

    let mut bytes = honest;
    let (lfh, _) = entry(&bytes, "blocks/report");
    rename_at(&mut bytes, lfh, LFH_NAME, "manifest.json"); // 13 bytes, like "blocks/report"

    assert_eq!(
        names(&cd_view(&bytes)),
        ["mimetype", "manifest.json", "blocks/report"]
    );
    assert_eq!(
        names(&stream_view(&bytes)),
        ["mimetype", "manifest.json", "manifest.json"],
        "the crafted archive is a real cloak: a streaming reader sees a second manifest"
    );
    assert_refused(&bytes, "name cloak");

    // The per-member read refuses too, with no `verify` asked for.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("c.tsra");
    std::fs::write(&path, &bytes).unwrap();
    let err = Reader::open(&path)
        .and_then(|mut r| r.read_block("report"))
        .expect_err("read_block must refuse a member whose local header disagrees");
    assert!(matches!(err, Error::Container(_)), "{err}");
}

/// The "zombie zip" shape: the central directory says STORED and the sealed digest covers the stored
/// bytes, which tessera verifies. The local header says DEFLATE, with the CRC and size of what those
/// same bytes inflate to, so a streaming reader extracts a different file with a valid checksum.
#[test]
fn a_stored_block_whose_local_header_claims_deflate_is_refused() {
    let hidden: &[u8] = b"#!/bin/sh\necho 'not what was verified'\n";
    let sealed_payload = deflate_stored_block(hidden);
    let mut bytes = sealed_blobs(&[("data", &sealed_payload)]);
    assert!(verdict(&bytes).is_none(), "the honest product verifies");

    let (lfh, _) = entry(&bytes, "blocks/data");
    put_u16(&mut bytes, lfh + LFH_METHOD, METHOD_DEFLATE);
    put_u32(&mut bytes, lfh + LFH_CRC, crc32(hidden));
    set_local_sizes(
        &mut bytes,
        lfh,
        hidden.len() as u64,
        sealed_payload.len() as u64,
    );

    let cd = cd_view(&bytes);
    let stream = stream_view(&bytes);
    assert_eq!(cd[2], ("blocks/data".to_string(), sealed_payload.clone()));
    assert_eq!(
        stream[2],
        ("blocks/data".to_string(), hidden.to_vec()),
        "the crafted archive is a real cloak: same name, other bytes, valid CRC"
    );
    assert_refused(&bytes, "codec cloak");
}

/// Two central-directory records with one name. zip resolves a name to the LAST record, so tessera
/// reads the genuine block and its digest checks; a first-match reader (and a streaming reader's first
/// extraction) gets the forged one. Refused earliest of the four, at open: a duplicated name blocks
/// every member read, `mimetype` included.
#[test]
fn a_central_directory_naming_one_member_twice_is_refused() {
    let honest = sealed_blobs(&[("report", b"quarterly numbers")]);
    let members = cd_view(&honest);
    let get = |n: &str| members.iter().find(|(m, _)| m == n).unwrap().1.clone();
    let (mime, manifest, report) = (get("mimetype"), get("manifest.json"), get("blocks/report"));
    let mut bytes = rezip(&[
        ("mimetype", &mime),
        ("manifest.json", &manifest),
        ("blocks/r3port", b"forged numbers"),
        ("blocks/report", &report),
    ]);
    let (lfh, cdh) = entry(&bytes, "blocks/r3port");
    rename_at(&mut bytes, lfh, LFH_NAME, "blocks/report");
    rename_at(&mut bytes, cdh, CDH_NAME, "blocks/report");

    let stream = stream_view(&bytes);
    assert_eq!(
        names(&stream),
        [
            "mimetype",
            "manifest.json",
            "blocks/report",
            "blocks/report"
        ]
    );
    assert_ne!(
        stream[2].1, stream[3].1,
        "two members, one name, two contents"
    );
    assert_refused(&bytes, "duplicate name");
}

/// A member the central directory never mentions, hidden in a gap before it. Every local header the
/// central directory points at agrees with it and nothing overlaps, so zip 9.0.5's
/// `validate_header_consistency` accepts the file; a streaming reader still sees an extra
/// `manifest.json`. Only the layout rule, that entries tile the archive with no gap, catches it.
#[test]
fn a_member_hidden_before_the_central_directory_is_refused() {
    let honest = sealed_blobs(&[("report", b"quarterly numbers")]);
    let forged = {
        let z = rezip(&[("manifest.json", b"{\"forged\":true}")]);
        let cd = ZipArchive::new(Cursor::new(&z))
            .unwrap()
            .central_directory_start() as usize;
        z[..cd].to_vec() // one local header + its bytes, no central record
    };
    let bytes = insert_before_central_directory(&honest, &forged);

    assert_eq!(
        cd_view(&bytes),
        cd_view(&honest),
        "the CD view is unchanged"
    );
    assert_eq!(
        names(&stream_view(&bytes)),
        [
            "mimetype",
            "manifest.json",
            "blocks/report",
            "manifest.json"
        ],
        "the crafted archive is a real cloak: a streaming reader sees a hidden manifest"
    );
    assert!(
        ZipArchive::new(Cursor::new(&bytes))
            .unwrap()
            .validate_header_consistency()
            .is_ok(),
        "zip's own check accepts this archive; the gap is tessera's rule to enforce"
    );
    assert_refused(&bytes, "hidden member");
}

// ── the honest side: nothing tessera writes trips the rule ──────────────────────────────────────

/// Every committed corpus vector, and every writer path over every fixture, still verifies: the rule
/// describes what tessera has always written, so no existing `.tsra` is newly refused.
#[test]
fn every_tessera_writer_and_corpus_file_satisfies_the_layout_rule() {
    let corpus = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus/files"));
    let mut checked = 0;
    for e in std::fs::read_dir(corpus).unwrap() {
        let p = e.unwrap().path();
        if p.extension().is_some_and(|x| x == "tsra") {
            let bytes = std::fs::read(&p).unwrap();
            assert_eq!(cd_view(&bytes), stream_view(&bytes), "{}", p.display());
            assert!(verdict(&bytes).is_none(), "{} is refused", p.display());
            checked += 1;
        }
    }
    assert_eq!(checked, 8, "the eight committed corpus vectors");

    let dir = tempfile::tempdir().unwrap();
    let all = fixtures();
    assert_eq!(all.len(), 8, "the eight conformance fixtures");
    for f in all {
        let d = dir.path().join(f.name);
        std::fs::create_dir_all(&d).unwrap();
        let frags: Vec<_> = f
            .payloads
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let frag = d.join(format!("frag{i}"));
                std::fs::write(&frag, &p.bytes).unwrap();
                (p.name.clone(), frag)
            })
            .collect();
        let sources: Vec<_> = frags
            .iter()
            .map(|(n, p)| (n.clone(), p.as_path()))
            .collect();

        let packed = d.join("pack.tsra");
        pack(&f.manifest, &f.payloads, &packed).unwrap();
        let streamed = d.join("stream.tsra");
        pack_streaming(&f.manifest, &sources, &streamed).unwrap();
        let verified = d.join("verified.tsra");
        pack_streaming_verified(&f.manifest, &sources, &verified).unwrap();
        let aux = d.join("aux.tsra");
        std::fs::copy(&packed, &aux).unwrap();
        add_aux_members(&aux, &[AuxMember::new("note.txt", vec![b'z'; 4096])]).unwrap();
        let exploded = d.join("exploded");
        unpack(&aux, &exploded).unwrap();
        let repacked = d.join("pack_dir.tsra");
        pack_dir(&exploded, &repacked).unwrap();

        for p in [&packed, &streamed, &verified, &aux, &repacked] {
            let bytes = std::fs::read(p).unwrap();
            assert_eq!(cd_view(&bytes), stream_view(&bytes), "{}", p.display());
            assert!(verdict(&bytes).is_none(), "{} is refused", p.display());
        }
    }
}
