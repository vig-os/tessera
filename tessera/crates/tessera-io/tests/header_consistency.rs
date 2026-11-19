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
        .large_file(true)
        .system(zip::System::Unix);
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

// ── every byte of the file: what follows the central directory ──────────────────────────────────

/// The offset of the (last) end-of-central-directory record.
fn eocd_at(bytes: &[u8]) -> usize {
    bytes
        .windows(4)
        .rposition(|w| w == EOCD_SIG)
        .expect("end of central directory")
}

/// `bytes` must be refused as a container problem whose message contains `needle`.
fn assert_refused_for(bytes: &[u8], why: &str, needle: &str) {
    match verdict(bytes) {
        None => panic!("verified a non-canonical archive ({why})"),
        Some(Error::Container(msg)) => assert!(
            msg.contains("not a canonical .tsra") && msg.contains(needle),
            "refused for the wrong reason ({why}): {msg}"
        ),
        Some(other) => panic!("refused, but not as a container problem ({why}): {other}"),
    }
}

/// What a reader that adjusts for a prefix reads as `manifest.json`. `unzip` and Python's `zipfile`
/// take the last end record in the file and shift every offset by however far the directory it names
/// sits from where the record says it starts (`zipfile`'s `concat = eocd - size_cd - offset_cd`).
/// Opening at that offset is exactly that reading.
fn prefix_adjusting_manifest(bytes: &[u8]) -> Vec<u8> {
    let eocd = eocd_at(bytes);
    let (size_cd, offset_cd) = (
        u32_at(bytes, eocd + 12),
        u32_at(bytes, eocd + EOCD_CD_OFFSET),
    );
    let concat = eocd as u64 - u64::from(size_cd) - u64::from(offset_cd);
    assert_ne!(
        concat, 0,
        "the last end record describes an archive that does not start at 0"
    );
    let config = zip::read::Config {
        archive_offset: zip::read::ArchiveOffset::Known(concat),
    };
    let mut a = ZipArchive::with_config(config, Cursor::new(bytes)).unwrap();
    let mut b = Vec::new();
    a.by_name("manifest.json")
        .unwrap()
        .read_to_end(&mut b)
        .unwrap();
    b
}

/// What tessera reads: the archive opened at byte 0 (SPEC §6), through its central directory.
fn cd_view_at_zero(bytes: &[u8]) -> View {
    let config = zip::read::Config {
        archive_offset: zip::read::ArchiveOffset::Known(0),
    };
    let mut a = ZipArchive::with_config(config, Cursor::new(bytes)).unwrap();
    (0..a.len())
        .map(|i| {
            let mut f = a.by_index(i).unwrap();
            let mut b = Vec::new();
            f.read_to_end(&mut b).unwrap();
            (f.name().unwrap().into_owned(), b)
        })
        .collect()
}

/// The reviewer's blocker on the first round: a second, ordinary zip appended after the genuine end
/// record. `tessera` opens at byte 0, so `zip` rejects the appended end record and falls back to the
/// genuine one, and the file verified. A reader that adjusts for a prefix (`unzip -p`, `zipfile.read`)
/// takes the appended archive as the whole file and reads the forged manifest, with no CRC error.
#[test]
fn a_second_archive_appended_after_the_end_record_is_refused() {
    let honest = sealed_blobs(&[("data", b"genuine payload")]);
    let forged = rezip(&[
        ("mimetype", b"application/vnd.tessera"),
        ("manifest.json", b"{\"forged\":true}"),
        ("blocks/data", b"EVIL"),
    ]);
    let mut bytes = honest.clone();
    bytes.extend_from_slice(&forged);

    assert_eq!(
        cd_view_at_zero(&bytes),
        cd_view(&honest),
        "tessera's own reading is the genuine archive"
    );
    assert_eq!(
        prefix_adjusting_manifest(&bytes),
        b"{\"forged\":true}",
        "the crafted archive is a real cloak: a prefix-adjusting reader reads the forged manifest"
    );
    assert_refused_for(&bytes, "appended archive", "follow the central directory");
}

/// The same second archive carried as the genuine end record's comment, so every byte after the end
/// record is "accounted for" by the comment length. A `.tsra` carries no comment.
#[test]
fn an_archive_comment_is_refused() {
    let honest = sealed_blobs(&[("data", b"genuine payload")]);
    let comment = b"HIDDEN-UNHASHED-PAYLOAD ".repeat(40);
    let mut bytes = honest.clone();
    let eocd = eocd_at(&bytes);
    put_u16(&mut bytes, eocd + 20, u16::try_from(comment.len()).unwrap());
    bytes.extend_from_slice(&comment);

    let a = ZipArchive::new(Cursor::new(&bytes)).unwrap();
    assert_eq!(
        a.comment(),
        &comment[..],
        "zip reads it as the archive comment"
    );
    assert_eq!(cd_view(&bytes), cd_view(&honest));
    assert_refused_for(&bytes, "archive comment", "follow the central directory");
}

/// Bytes between the last central-directory record and the end record, which no record describes.
/// The reviewer hid a complete local entry there; plain junk is the same rule.
#[test]
fn bytes_between_the_central_directory_and_the_end_record_are_refused() {
    let honest = sealed_blobs(&[("data", b"genuine payload")]);
    let eocd = eocd_at(&honest);
    assert_ne!(
        &honest[eocd - 20..eocd - 16],
        ZIP64_LOCATOR_SIG,
        "this small archive has no zip64 end records"
    );
    let hidden_entry = {
        let z = rezip(&[("manifest.json", b"{\"forged\":true}")]);
        let cd = ZipArchive::new(Cursor::new(&z))
            .unwrap()
            .central_directory_start() as usize;
        z[..cd].to_vec()
    };
    for (what, inserted) in [
        ("a hidden local entry", hidden_entry),
        ("junk", b"HIDDEN-UNHASHED-PAYLOAD ".repeat(40)),
    ] {
        let mut bytes = honest[..eocd].to_vec();
        bytes.extend_from_slice(&inserted);
        bytes.extend_from_slice(&honest[eocd..]);
        assert_eq!(
            cd_view(&bytes),
            cd_view(&honest),
            "{what}: zip still opens it"
        );
        assert_refused_for(&bytes, what, "follow the central directory");
    }
}

/// Anything after the end record, even with no comment claiming it.
#[test]
fn bytes_after_the_end_record_are_refused() {
    let honest = sealed_blobs(&[("data", b"genuine payload")]);
    let mut bytes = honest.clone();
    bytes.extend_from_slice(b"trailing bytes no record describes");
    assert_eq!(cd_view(&bytes), cd_view(&honest), "zip still opens it");
    assert_refused_for(&bytes, "trailing bytes", "follow the central directory");
}

/// A `blocks/` member the manifest does not list carries bytes no digest covers under a name every
/// reader takes for part of the product, so it is refused. An `aux/` member is the one exception, by
/// design (ADR-0042): carried, deliberately outside the seal, and read identically by every reader.
/// Both archives here are well formed, rebuilt from scratch as a zip appender would leave them.
#[test]
fn a_member_the_manifest_does_not_list_is_refused_outside_aux() {
    let honest = sealed_blobs(&[("data", b"genuine payload")]);
    let with = |extra: &str| {
        let mut entries = cd_view(&honest);
        entries.push((extra.to_string(), b"unsealed bytes".to_vec()));
        let refs: Vec<(&str, &[u8])> = entries
            .iter()
            .map(|(n, b)| (n.as_str(), b.as_slice()))
            .collect();
        rezip(&refs)
    };

    let unlisted_block = with("blocks/evil");
    assert_eq!(
        cd_view(&unlisted_block),
        stream_view(&unlisted_block),
        "an honest zip: both readers agree, and both list the unsealed block"
    );
    assert_refused_for(
        &unlisted_block,
        "unlisted block",
        "a block the manifest lists",
    );
    for stray in ["evil", "manifest.json.bak", "Blocks/data"] {
        assert_refused_for(&with(stray), stray, "a block the manifest lists");
    }

    let aux = with("aux/note.txt");
    assert_eq!(cd_view(&aux), stream_view(&aux));
    assert!(
        verdict(&aux).is_none(),
        "an aux/ member is carried outside the seal by design (ADR-0042)"
    );
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

// ── every byte of every record: the canonical encoding ──────────────────────────────────────────
//
// The second round of review. Everything above holds each record to the others; nothing held a
// record to what it says. A field the check ignores is a field some reader interprets, and the
// reviewer found five: a data-descriptor flag, an unknown extra field that the tiling counts as part
// of its entry, an encryption flag, a per-entry disk number, and a symlink mode. So each record is
// held, byte for byte, to the one tessera's writer emits (SPEC §6). The builder below writes that
// encoding by hand, proven identical to `pack`'s output, and each case changes exactly one thing.

/// One entry for [`hand_zip`]: by default, exactly what tessera's writer emits for it.
#[derive(Clone)]
struct Hand {
    name: Vec<u8>,
    data: Vec<u8>,
    /// The CRC both headers carry, when it is not the data's.
    crc: Option<u32>,
    flags: u16,
    /// The local header's CRC and zip64 sizes, when they differ from the data's (a data descriptor
    /// lets a writer leave them zero).
    local_crc: Option<u32>,
    local_size: Option<u64>,
    /// Bytes appended to the local header's (or the central record's) extra field, after its zip64
    /// field.
    local_extra: Vec<u8>,
    central_extra: Vec<u8>,
    made_by: u16,
    date: u16,
    disk: u16,
    external: u32,
}

impl Hand {
    fn new(name: &[u8], data: &[u8]) -> Self {
        Hand {
            name: name.to_vec(),
            data: data.to_vec(),
            crc: None,
            flags: 0,
            local_crc: None,
            local_size: None,
            local_extra: Vec::new(),
            central_extra: Vec::new(),
            made_by: 0x032d,
            date: 0x0021,
            disk: 0,
            external: 0o100_644 << 16,
        }
    }
}

/// The local header [`hand_zip`] writes for `e`.
fn hand_local_header(e: &Hand) -> Vec<u8> {
    let size = e.local_size.unwrap_or(e.data.len() as u64);
    let mut extra = Vec::new();
    extra.extend_from_slice(&ZIP64_EXTRA_ID.to_le_bytes());
    extra.extend_from_slice(&16u16.to_le_bytes());
    extra.extend_from_slice(&size.to_le_bytes());
    extra.extend_from_slice(&size.to_le_bytes());
    extra.extend_from_slice(&e.local_extra);
    let mut h = b"PK\x03\x04".to_vec();
    for v in [45u16, e.flags, 0, 0, e.date] {
        h.extend_from_slice(&v.to_le_bytes());
    }
    let crc = e.crc.unwrap_or_else(|| crc32(&e.data));
    h.extend_from_slice(&e.local_crc.unwrap_or(crc).to_le_bytes());
    h.extend_from_slice(&u32::MAX.to_le_bytes());
    h.extend_from_slice(&u32::MAX.to_le_bytes());
    h.extend_from_slice(&u16::try_from(e.name.len()).unwrap().to_le_bytes());
    h.extend_from_slice(&u16::try_from(extra.len()).unwrap().to_le_bytes());
    h.extend_from_slice(&e.name);
    h.extend_from_slice(&extra);
    h
}

/// A `.tsra`-shaped zip written by hand, entry by entry: the local entries back to back from byte 0,
/// the central directory, the end record. Small archives only (no offset past 32 bits).
fn hand_zip(entries: &[Hand]) -> Vec<u8> {
    let order: Vec<usize> = (0..entries.len()).collect();
    hand_zip_listed(entries, &order)
}

/// [`hand_zip`], with the central directory listing the entries in `order` (indices into `entries`,
/// which sit in the file in their own order).
fn hand_zip_listed(entries: &[Hand], order: &[usize]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut records = Vec::new();
    for e in entries {
        let offset = u32::try_from(out.len()).unwrap();
        out.extend_from_slice(&hand_local_header(e));
        out.extend_from_slice(&e.data);
        let mut cd = Vec::new();

        let size = e.data.len() as u64;
        let mut extra = Vec::new();
        extra.extend_from_slice(&ZIP64_EXTRA_ID.to_le_bytes());
        extra.extend_from_slice(&16u16.to_le_bytes());
        extra.extend_from_slice(&size.to_le_bytes());
        extra.extend_from_slice(&size.to_le_bytes());
        extra.extend_from_slice(&e.central_extra);
        cd.extend_from_slice(b"PK\x01\x02");
        for v in [e.made_by, 45, e.flags, 0, 0, e.date] {
            cd.extend_from_slice(&v.to_le_bytes());
        }
        cd.extend_from_slice(&e.crc.unwrap_or_else(|| crc32(&e.data)).to_le_bytes());
        cd.extend_from_slice(&u32::MAX.to_le_bytes());
        cd.extend_from_slice(&u32::MAX.to_le_bytes());
        let name_len = u16::try_from(e.name.len()).unwrap();
        let extra_len = u16::try_from(extra.len()).unwrap();
        for v in [name_len, extra_len, 0, e.disk, 0] {
            cd.extend_from_slice(&v.to_le_bytes());
        }
        cd.extend_from_slice(&e.external.to_le_bytes());
        cd.extend_from_slice(&offset.to_le_bytes());
        cd.extend_from_slice(&e.name);
        cd.extend_from_slice(&extra);
        records.push(cd);
    }
    let cd: Vec<u8> = order.iter().flat_map(|&i| records[i].clone()).collect();
    let (cd_start, n) = (out.len(), u16::try_from(order.len()).unwrap());
    out.extend_from_slice(&cd);
    out.extend_from_slice(EOCD_SIG);
    for v in [0u16, 0, n, n] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&u32::try_from(cd.len()).unwrap().to_le_bytes());
    out.extend_from_slice(&u32::try_from(cd_start).unwrap().to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

/// One extra field: `id`, its length, `body`.
fn extra_field(id: u16, body: &[u8]) -> Vec<u8> {
    let mut f = id.to_le_bytes().to_vec();
    f.extend_from_slice(&u16::try_from(body.len()).unwrap().to_le_bytes());
    f.extend_from_slice(body);
    f
}

/// A data descriptor (`PK\7\8`, APPNOTE 4.3.9) for `consumed`: what a streaming reader that is
/// looking for the end of a bit-3 member accepts as that end.
fn fake_descriptor(consumed: &[u8]) -> Vec<u8> {
    let mut d = b"PK\x07\x08".to_vec();
    let len = u32::try_from(consumed.len()).unwrap();
    for v in [crc32(consumed), len, len] {
        d.extend_from_slice(&v.to_le_bytes());
    }
    d
}

const FORGED: &[u8] = b"{\"forged\":true}";

/// The honest product the cases start from, as [`Hand`] entries in writer order.
fn honest_entries() -> Vec<Hand> {
    cd_view(&sealed_blobs(&[("data", b"genuine payload")]))
        .into_iter()
        .map(|(n, b)| Hand::new(n.as_bytes(), &b))
        .collect()
}

/// `honest_entries` with `aux` inserted before the block, as the reviewer placed it.
fn with_aux(aux: Hand) -> Vec<Hand> {
    let mut es = honest_entries();
    es.insert(2, aux);
    es
}

/// The builder writes what `pack` and `add_aux_members` write, to the byte: every case below differs
/// from a real `.tsra` in exactly the one thing it names.
#[test]
fn the_hand_builder_is_the_writer() {
    let honest = sealed_blobs(&[("data", b"genuine payload")]);
    assert_eq!(hand_zip(&honest_entries()), honest);

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("aux.tsra");
    std::fs::write(&path, &honest).unwrap();
    add_aux_members(&path, &[AuxMember::new("note.txt", b"hello".to_vec())]).unwrap();
    let mut es = honest_entries();
    es.push(Hand::new(b"aux/note.txt", b"hello"));
    assert_eq!(hand_zip(&es), std::fs::read(&path).unwrap());
    assert!(verdict(&hand_zip(&es)).is_none());
}

/// Each case, refused with the field that differs named in the message. Before the canonical
/// encoding every one of them verified: `zip`'s own consistency check accepts them all (asserted),
/// as did the member set, the tiling and the end records. Every case is judged before any failure is
/// reported, so one run lists each case that is not refused, not only the first.
fn assert_each_refused(cases: Vec<(&str, Vec<u8>, &str)>) {
    let mut failures = Vec::new();
    for (what, bytes, needle) in cases {
        let mut a = ZipArchive::new(Cursor::new(&bytes))
            .unwrap_or_else(|e| panic!("{what}: zip no longer opens it: {e}"));
        assert!(
            a.validate_header_consistency().is_ok(),
            "{what}: zip's own check accepts it; the encoding is tessera's rule to enforce"
        );
        match verdict(&bytes) {
            None => failures.push(format!("{what}: verified a non-canonical archive")),
            Some(Error::Container(msg))
                if msg.contains("not a canonical .tsra") && msg.contains(needle) => {}
            Some(other) => failures.push(format!("{what}: refused for the wrong reason: {other}")),
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

/// The blocker (review n20, n03, n02). General-purpose bit 3 says "the CRC and sizes follow the data,
/// in a descriptor", and lets the local header carry zeros. A streaming reader then looks for the
/// member's end, and the first `PK\7\8` that matches what it has read so far ends it. In n20 that is
/// a forged descriptor inside the next member's local extra field, followed there by a complete
/// forged `manifest.json`, which the reviewer's `cat f | bsdtar -xf -` extracted; in n03 the same sits
/// in an `aux/` member's bytes. The flag alone (n02) already makes the reading reader-dependent.
#[test]
fn a_data_descriptor_flag_is_refused() {
    let dd = |mut es: Vec<Hand>| {
        es[1].flags = 0x0008;
        es[1].local_crc = Some(0);
        es[1].local_size = Some(0);
        es
    };

    let flag_only = dd(honest_entries());

    let in_local_extra = {
        let mut es = dd(honest_entries());
        let forged = {
            let mut f = hand_local_header(&Hand::new(b"manifest.json", FORGED));
            f.extend_from_slice(FORGED);
            f
        };
        // The descriptor matches everything after manifest.json's local header up to itself: the
        // genuine manifest bytes and the block's local header as far as the extra field's body.
        let body_len = 16 + forged.len();
        let placeholder = extra_field(0xcafe, &vec![0; body_len]);
        es[2].local_extra = placeholder;
        let head = hand_local_header(&es[2]);
        let mut consumed = es[1].data.clone();
        consumed.extend_from_slice(&head[..head.len() - body_len]);
        let mut body = fake_descriptor(&consumed);
        body.extend_from_slice(&forged);
        es[2].local_extra = extra_field(0xcafe, &body);
        es
    };

    let in_aux = {
        let forged = {
            let mut f = hand_local_header(&Hand::new(b"manifest.json", FORGED));
            f.extend_from_slice(FORGED);
            f.extend_from_slice(&hand_local_header(&Hand::new(b"blocks/data", b"EVIL")));
            f.extend_from_slice(b"EVIL");
            f
        };
        let mut aux = fake_descriptor(
            b"(the reviewer's harness solves this CRC; the flag is what is refused)",
        );
        aux.extend_from_slice(&forged);
        dd(with_aux(Hand::new(b"aux/x", &aux)))
    };

    assert_each_refused(vec![
        (
            "n02: the flag alone",
            hand_zip(&flag_only),
            "general-purpose flags",
        ),
        (
            "n20: a forged entry in a block's local extra field",
            hand_zip(&in_local_extra),
            "general-purpose flags",
        ),
        (
            "n03: a forged entry in an aux member",
            hand_zip(&in_aux),
            "general-purpose flags",
        ),
    ]);
}

/// Every other field a reader interprets, one case each. An extra field other than zip64 is where
/// n20 hid its forged entry, and also how a Unicode-path field (0x7075) renames a member for the
/// readers that honour it; the encryption flag makes a reader decrypt; a disk number makes a reader
/// look on another disk; a symlink mode makes an extracting reader write a link; a FAT host makes a
/// reader take `\` for a separator; a modification time is pinned for writer-determinism.
#[test]
fn every_field_a_reader_interprets_is_the_writers() {
    // Review n09: the field's CRC does not match the name it replaces, so `zip` ignores it, but a
    // reader that skips that check renames the member to `manifest.json`. (With a matching CRC, n10,
    // `zip` honours it, sees `manifest.json` twice and refuses the archive on its own.)
    let unicode_path = {
        let mut body = vec![1];
        body.extend_from_slice(&0xdead_beef_u32.to_le_bytes());
        body.extend_from_slice(b"manifest.json");
        extra_field(0x7075, &body)
    };
    let hidden_entry = {
        let mut f = hand_local_header(&Hand::new(b"manifest.json", FORGED));
        f.extend_from_slice(FORGED);
        extra_field(0xcafe, &f)
    };
    let aux = |f: &dyn Fn(&mut Hand)| {
        let mut h = Hand::new(b"aux/note", b"carried bytes");
        f(&mut h);
        hand_zip(&with_aux(h))
    };
    let block = |f: &dyn Fn(&mut Hand)| {
        let mut es = honest_entries();
        f(&mut es[2]);
        hand_zip(&es)
    };

    assert_each_refused(vec![
        (
            "n15: an encrypted aux member",
            aux(&|h| h.flags = 0x0001),
            "general-purpose flags",
        ),
        (
            "an aux member flagged UTF-8 with an ASCII name",
            aux(&|h| h.flags = 0x0800),
            "general-purpose flags",
        ),
        (
            "n16: a forged entry in a local extra field",
            aux(&|h| h.local_extra = hidden_entry.clone()),
            "extra-field length",
        ),
        (
            "n17: a forged entry in a central extra field",
            aux(&|h| h.central_extra = hidden_entry.clone()),
            "extra-field length",
        ),
        (
            "n09: a Unicode-path field naming manifest.json",
            aux(&|h| {
                h.name = b"aux/zzzzzzzzzzzzz".to_vec();
                h.local_extra = unicode_path.clone();
                h.central_extra = unicode_path.clone();
            }),
            "extra-field length",
        ),
        (
            "an unknown extra field on a sealed block's central record only",
            block(&|h| h.central_extra = extra_field(0xcafe, b"unhashed")),
            "extra-field length",
        ),
        (
            "n08: a block whose central record says it starts on disk 1",
            block(&|h| h.disk = 1),
            "disk number start",
        ),
        (
            "n14: an aux member with a symlink mode",
            aux(&|h| {
                h.data = b"../../../../etc/passwd".to_vec();
                h.external = 0o120_777 << 16;
            }),
            "external attributes",
        ),
        (
            "an aux member marked as a directory",
            aux(&|h| h.external = (0o040_755 << 16) | 0x10),
            "external attributes",
        ),
        (
            "an aux member made on a FAT host",
            aux(&|h| h.made_by = 0x0014),
            "version made by",
        ),
        (
            "a block stamped with a modification date other than the epoch",
            block(&|h| h.date = 0x5a21),
            "modification date",
        ),
    ]);
}

/// Review n11-n14: an `aux/` name is a safe relative path, by the block-name grammar the writers
/// already apply (ADR-0063). Each of these is a name some extracting reader writes outside its
/// target directory, or reads as a different name than another reader does.
#[test]
fn an_aux_member_name_must_be_a_safe_relative_path() {
    let name = |n: &[u8]| hand_zip(&with_aux(Hand::new(n, FORGED)));
    let mut utf8 = Hand::new("aux/caf\u{e9}".as_bytes(), FORGED);
    utf8.flags = 0x0800;
    let mut dir = Hand::new(b"aux/sub/", b"");
    dir.external = (0o040_755 << 16) | 0x10;
    assert_each_refused(vec![
        (
            "n11: a .. segment",
            name(b"aux/../manifest.json"),
            "not a safe relative path",
        ),
        (
            "n12: a backslash",
            name(b"aux/..\\manifest.json"),
            "not a safe relative path",
        ),
        (
            "n13: a NUL",
            name(b"aux/\0manifest.json"),
            "not a safe relative path",
        ),
        (
            "a control character",
            name(b"aux/\x1bnote"),
            "not a safe relative path",
        ),
        (
            "an absolute path below aux/",
            name(b"aux//etc/passwd"),
            "not a safe relative path",
        ),
        (
            "an empty segment",
            name(b"aux/a//b"),
            "not a safe relative path",
        ),
        (
            "a . segment",
            name(b"aux/./note"),
            "not a safe relative path",
        ),
        ("the bare prefix", name(b"aux/"), "not a safe relative path"),
        (
            "a directory entry",
            hand_zip(&with_aux(dir)),
            "not a safe relative path",
        ),
        (
            "a non-ASCII name",
            hand_zip(&with_aux(utf8)),
            "not a safe relative path",
        ),
        (
            "bytes that are not UTF-8",
            name(b"aux/\xffnote"),
            "not a safe relative path",
        ),
        (
            "a Windows drive",
            name(b"aux/C:note"),
            "not a safe relative path",
        ),
    ]);
}

/// Review n05/n06: zip64 end records on an archive that needs none. Every reader reaches the same
/// directory through them, but they are not what the writer emits, and the rule is the writer's
/// encoding, not "harmless enough".
#[test]
fn zip64_end_records_on_a_small_archive_are_refused() {
    let honest = sealed_blobs(&[("data", b"genuine payload")]);
    let eocd = eocd_at(&honest);
    let (cd_size, cd_start) = (
        u64::from(u32_at(&honest, eocd + 12)),
        u64::from(u32_at(&honest, eocd + EOCD_CD_OFFSET)),
    );
    let n = u64::from(u16_at(&honest, eocd + 10));
    for (what, sentinels) in [("n05: sentinels", true), ("n06: true values", false)] {
        let mut bytes = honest[..eocd].to_vec();
        let record_at = bytes.len() as u64;
        bytes.extend_from_slice(ZIP64_EOCD_SIG);
        bytes.extend_from_slice(&44u64.to_le_bytes());
        bytes.extend_from_slice(&45u16.to_le_bytes());
        bytes.extend_from_slice(&45u16.to_le_bytes());
        bytes.extend_from_slice(&[0; 8]);
        for v in [n, n, cd_size, cd_start] {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        bytes.extend_from_slice(ZIP64_LOCATOR_SIG);
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&record_at.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        let mut eocd_rec = honest[eocd..].to_vec();
        if sentinels {
            put_u16(&mut eocd_rec, 8, u16::MAX);
            put_u16(&mut eocd_rec, 10, u16::MAX);
            put_u32(&mut eocd_rec, 12, u32::MAX);
            put_u32(&mut eocd_rec, 16, u32::MAX);
        }
        bytes.extend_from_slice(&eocd_rec);
        assert_eq!(
            cd_view(&bytes),
            cd_view(&honest),
            "{what}: zip reads the same archive"
        );
        assert_refused_for(&bytes, what, "follow the central directory");
    }
}

/// Review round 2b: the archive's members as a set, and the bytes no check read. Each archive here is
/// canonical record by record, yet readers left different trees on disk: `unzip`, `zipfile` and 7z
/// (directory order) against `cat f | bsdtar -xf -` (file order) when the directory lists `aux/a`
/// before `aux/a/b` but the file holds them the other way round; a case-insensitive file system for
/// `aux/SIG` next to `aux/sig`; and an `aux/` member whose bytes do not match its CRC, which
/// `zipfile` truncates to nothing while the others write it with a warning.
#[test]
fn the_member_set_reads_the_same_in_every_order() {
    let reordered = {
        let es = with_aux(Hand::new(b"aux/b", b"second"));
        let mut es2 = es.clone();
        es2.insert(2, Hand::new(b"aux/a", b"first"));
        // File: mimetype, manifest.json, aux/a, aux/b, blocks/data. Directory: aux/b before aux/a.
        hand_zip_listed(&es2, &[0, 1, 3, 2, 4])
    };
    let leaf = {
        let mut es = with_aux(Hand::new(b"aux/a/b", b"below"));
        es.insert(2, Hand::new(b"aux/a", b"a file"));
        hand_zip(&es)
    };
    let case = {
        let mut es = with_aux(Hand::new(b"aux/sig", b"lower"));
        es.insert(2, Hand::new(b"aux/SIG", b"upper"));
        hand_zip(&es)
    };
    let bad_crc = {
        let mut h = Hand::new(b"aux/x", b"payload\n");
        h.crc = Some(0xdead_beef);
        hand_zip(&with_aux(h))
    };
    assert_each_refused(vec![
        (
            "a directory that lists members in another order than the file",
            reordered,
            "another order",
        ),
        ("aux/a next to aux/a/b", leaf, "leaf rule"),
        ("aux/SIG next to aux/sig", case, "letter case"),
        (
            "an aux member with a wrong CRC-32",
            bad_crc,
            "CRC-32 records",
        ),
    ]);
}

/// The aux writers refuse to produce what verification refuses: a member below an existing one, a
/// case variant of one, and the same pair packed from an exploded directory.
#[test]
fn the_aux_writers_hold_the_member_set_to_the_same_rule() {
    let honest = sealed_blobs(&[("data", b"genuine payload")]);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("p.tsra");
    std::fs::write(&path, &honest).unwrap();
    add_aux_members(&path, &[AuxMember::new("notes", b"a file".to_vec())]).unwrap();
    let before = std::fs::read(&path).unwrap();
    for (what, name, needle) in [
        ("below an existing member", "notes/later", "leaf rule"),
        ("a case variant of one", "NOTES", "letter case"),
    ] {
        let err = add_aux_members(&path, &[AuxMember::new(name, b"x".to_vec())]).expect_err(what);
        assert!(
            matches!(&err, Error::Container(m) if m.contains(needle)),
            "{what}: {err}"
        );
        assert_eq!(
            std::fs::read(&path).unwrap(),
            before,
            "{what}: nothing written"
        );
    }
    // Replacing a member by name is not a conflict with itself.
    add_aux_members(&path, &[AuxMember::new("notes", b"replaced".to_vec())]).unwrap();
    assert!(verdict(&std::fs::read(&path).unwrap()).is_none());

    let exploded = dir.path().join("exploded");
    unpack(&path, &exploded).unwrap();
    std::fs::write(exploded.join("aux").join("NOTES"), b"case twin").unwrap();
    let err = pack_dir(&exploded, &dir.path().join("out.tsra")).expect_err("case twins");
    assert!(
        matches!(&err, Error::Container(m) if m.contains("letter case")),
        "{err}"
    );
}
