//! The `.tsra` container — a STORED zip64 archive (ADR-0022).
//!
//! Layout: `mimetype` (first, uncompressed, magic) · `manifest.seal` · `manifest.json` ·
//! `blocks/<name>` payloads · `aux/<name>` (ADR-0042). `manifest.json` is stored as exactly its JCS
//! bytes and `manifest.seal` holds `blake3:<hex>` over them (ADR-0066), so one ranged read of the
//! front of the file gets the magic, the seal and the manifest.
//! Entries are **STORED** (payloads are already compressed by their codec) so every block byte
//! range is directly addressable via the zip central directory — a cloud reader range-reads just
//! the manifest + the blocks it needs, no whole-archive download. The reader verifies the magic,
//! the manifest against its seal ([`Manifest::from_stored_members`]: a product without
//! `manifest.seal`, the alpha layout, is refused naming both formats), and (on access) each stored
//! block's bytes against its recorded digest.

use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tessera_core::block::name::join_checked;
use tessera_core::block::partition::is_legacy_flat_member;
use tessera_core::block::{
    check_block_refs, check_leaf_rule, check_partitions, BlockName, TableGroup,
};
use tessera_core::manifest::Manifest;
use tessera_core::{Error, Result};
use zip::read::{ArchiveOffset, Config, ZipFile};
use zip::result::ZipError;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, System, ZipArchive, ZipReadOptions, ZipWriter};

/// Container MIME magic — the first, uncompressed entry (EPUB/ODF trick) so `file(1)` and magic
/// sniffers identify a `.tsra` without unzipping.
pub const MIMETYPE: &str = "application/vnd.tessera";
const MIMETYPE_ENTRY: &str = "mimetype";
const MANIFEST_ENTRY: &str = tessera_core::stored::MANIFEST_MEMBER;
const SEAL_ENTRY: &str = tessera_core::stored::MANIFEST_SEAL_MEMBER;
const BLOCKS_PREFIX: &str = "blocks/";
/// Reserved prefix for **non-sealed auxiliary members** (ADR-0042). The manifest lists exactly what
/// is sealed (its `blocks`); anything under `aux/` is carried inside the one shareable file but
/// **ignored by the seal** — the `.gitignore` for `content_hash` / `manifest_hash`. Adding or
/// removing an `aux/` member never changes either sealed hash, so the embedded signature (which must
/// sit outside the thing it signs) and the ingest-time provenance stamp can ride here without
/// perturbing writer-determinism or the conformance corpus.
pub const AUX_PREFIX: &str = "aux/";
/// The embedded-signature aux path: `aux/signatures/<key_id>.sig.json`. Reader falls back to the
/// detached `<file>.tsra.sig.json` when the embedded form is absent (ADR-0037 + ADR-0042).
pub const AUX_SIGNATURES_PREFIX: &str = "aux/signatures/";
/// The ingest-time provenance stamp path: `aux/provenance.json` (RFC-3339 wall-clock + producer +
/// host). Determinism-safe by construction — it's an aux member, so it never enters the seal.
pub const AUX_PROVENANCE_ENTRY: &str = "aux/provenance.json";

/// A block's encoded bytes, to be stored at `blocks/<name>`. `bytes` MUST be the exact bytes the
/// block's recorded digest was computed over (so the reader can verify integrity on access). The
/// name is a validated [`BlockName`] (ADR-0063), so no writer can emit a zip member outside
/// `blocks/`.
pub struct BlockPayload {
    pub name: BlockName,
    pub bytes: Vec<u8>,
}

impl BlockPayload {
    pub fn new(name: BlockName, bytes: Vec<u8>) -> Self {
        BlockPayload { name, bytes }
    }
}

fn cz(e: impl std::fmt::Display) -> Error {
    Error::Container(e.to_string())
}

/// The shared zip-entry options every `.tsra` writer (batch + streaming) uses — STORED + zip64 +
/// the pinned 1980-01-01 mtime. Extracted as the SSoT so [`pack`] and [`pack_streaming`] cannot
/// drift: identical options → identical archive bytes for identical inputs (the writer-determinism
/// release gate).
fn tsra_entry_options() -> SimpleFileOptions {
    SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .last_modified_time(DateTime::default())
        .large_file(true)
        // Unix on every host: on Windows `zip` would otherwise mark the entry as made on DOS, a
        // different "version made by" for the same product (SPEC §6's canonical encoding).
        .system(System::Unix)
}

/// Open a `.tsra` as a zip archive whose first byte is byte 0 of the source — the one way every
/// `.tsra` reader in this crate opens the container.
///
/// A `.tsra` never carries a prefix: its first local header is the `mimetype` entry, which is what lets
/// a sniffer identify it without unzipping (SPEC §6), and every writer here starts there. Saying so
/// matters for more than strictness. With the default `ArchiveOffset::Detect`, zip ≥ 9 locates the
/// archive by **scanning forward from byte 0** for the first central-directory header — a read of the
/// whole file before the first entry is touched, which turns the cloud reader's ranged GETs of the
/// central directory, the manifest and one block into a full download (#588; zip 2 did a single
/// arithmetic check instead). A known offset makes zip check the one position the end-of-central-
/// directory record names, so opening costs the tail of the file and nothing else
/// (`range::tests::reading_one_block_does_not_read_the_whole_archive` pins that).
///
/// `Known(0)` alone does not enforce "no prefix". It refuses a prefix whose central directory still
/// counts from the zip's own start, but a prefix whose offsets were rewritten to include it (what
/// `zip -A` does to a self-extractor, or what a shell/zip polyglot carries) is a well-formed zip at
/// offset 0 and opens. So the rule is checked here, against the central directory: entry 0 must be
/// `mimetype`, its local header must sit at byte 0, and it must be STORED. That makes `mimetype` both
/// the first entry and the first bytes of the file, which is exactly what a sniffer relies on. The check
/// reads entry metadata that `zip` already parsed from the central directory, so it costs no I/O and
/// no extra ranged GET.
fn open_tsra_archive<R: Read + Seek>(reader: R) -> Result<ZipArchive<R>> {
    let config = Config {
        archive_offset: ArchiveOffset::Known(0),
    };
    let archive = ZipArchive::with_config(config, reader).map_err(cz)?;
    require_mimetype_at_byte_zero(&archive)?;
    require_seal_then_manifest(&archive)?;
    Ok(archive)
}

/// SPEC §6's next two entries, from central-directory metadata alone: `manifest.seal` is entry 2
/// and `manifest.json` entry 3 (ADR-0066 rule 3), so one ranged read of the front of the file gets
/// the magic, the seal and the manifest. An archive with no `manifest.seal` at all passes here: it
/// is the alpha layout, which [`Manifest::from_stored_members`] refuses with the evidence it finds.
fn require_seal_then_manifest<R: Read + Seek>(archive: &ZipArchive<R>) -> Result<()> {
    if archive.index_for_name(SEAL_ENTRY).is_none() {
        return Ok(());
    }
    for (index, want) in [(1, SEAL_ENTRY), (2, MANIFEST_ENTRY)] {
        let found = if index < archive.len() {
            archive
                .by_index_data(index)
                .map_err(cz)?
                .name()
                .map_err(cz)?
                .into_owned()
        } else {
            "(none)".to_owned()
        };
        if found != want {
            return Err(Error::Container(format!(
                "not a canonical .tsra: entry {} is '{found}', but SPEC §6 orders `{MIMETYPE_ENTRY}`, \
                 `{SEAL_ENTRY}`, `{MANIFEST_ENTRY}` first",
                index + 1
            )));
        }
    }
    Ok(())
}

/// SPEC §6's leading-entry rule, from central-directory metadata alone (see [`open_tsra_archive`]).
fn require_mimetype_at_byte_zero<R: Read + Seek>(archive: &ZipArchive<R>) -> Result<()> {
    if archive.is_empty() {
        return Err(Error::Container(
            "not a .tsra (empty archive, no mimetype entry)".into(),
        ));
    }
    let first = archive.by_index_data(0).map_err(cz)?;
    let name = first.name().map_err(cz)?;
    if name != MIMETYPE_ENTRY {
        return Err(Error::Container(format!(
            "not a .tsra: the first entry is '{name}', but SPEC §6 requires '{MIMETYPE_ENTRY}' first"
        )));
    }
    if first.header_start() != 0 {
        return Err(Error::Container(format!(
            "not a .tsra: '{MIMETYPE_ENTRY}' starts at byte {}, so the archive carries a prefix; SPEC §6 \
             requires it to begin at byte 0",
            first.header_start()
        )));
    }
    if first.compression() != CompressionMethod::Stored {
        return Err(Error::Container(format!(
            "entry '{MIMETYPE_ENTRY}' is {:?}, but a .tsra must be STORED throughout",
            first.compression()
        )));
    }
    Ok(())
}

/// Open the member `entry` for reading, refusing it unless its **local header agrees with its
/// central-directory record** — the one way every `.tsra` member read in this crate opens an entry
/// (GHSA-m665-v7pm-7grj, #689; SPEC §6).
///
/// `zip` locates and decodes a member from its central-directory record and treats the local header
/// in front of the bytes as a pointer to skip. A streaming reader does the opposite: it never sees the
/// central directory and takes name, method, sizes and CRC from the local header. So a member whose two
/// headers disagree is two different members, and the digest tessera checks covers only one of them.
/// With the check on, `zip` (≥ 9.0.5) parses the local header and refuses the entry unless both
/// describe the same name, method, sizes, CRC and flags, and refuses any read at all from an archive
/// whose central directory names one member twice (`zip` would silently resolve the name to the last
/// record, and a first-match reader to the first).
///
/// The cost is small. `zip` reads that local header anyway to find where the bytes start; the check
/// also reads the header's name and extra field, the bytes directly in front of the payload. So a
/// ranged cloud read touches the same region as before, though an unbuffered range reader may issue
/// up to two more small reads per member.
///
/// `missing` builds the error for an entry that is simply absent, so each caller keeps its own wording;
/// every other failure (an inconsistent or unreadable header) is [`Error::Container`] naming the entry.
fn open_member<'a, R: Read + Seek>(
    archive: &'a mut ZipArchive<R>,
    entry: &str,
    missing: impl FnOnce() -> String,
) -> Result<ZipFile<'a, R>> {
    let opts = ZipReadOptions::new().enforce_header_consistency(true);
    archive
        .by_name_with_options(entry, opts)
        .map_err(|e| match e {
            ZipError::FileNotFound => Error::Container(missing()),
            // A truncated file or a failed ranged read is an I/O problem, not a header disagreement.
            ZipError::Io(e) => Error::Io(e),
            e => Error::Container(format!(
                "entry '{entry}': {e} — its local header and its central-directory record must \
                 describe the same member (SPEC §6)"
            )),
        })
}

/// The byte source a [`Reader`] hands to `zip`, shared with the reader itself.
///
/// `zip` interprets only the bytes its records point at, and a [`ZipArchive`] never hands its source
/// back. The deep verify must also read the bytes that no record points at: the end records and
/// anything around them (SPEC §6, [`check_central_directory`]). So the reader keeps a second handle to the
/// same source. The two never interleave: the check runs only while no member is open, and `zip`
/// seeks to an entry before it reads one.
struct SharedSource<R>(Arc<Mutex<R>>);

impl<R> SharedSource<R> {
    fn lock(&self) -> MutexGuard<'_, R> {
        // A panic mid-read leaves at worst a stale cursor, and every reader seeks before it reads.
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl<R: Read> Read for SharedSource<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.lock().read(buf)
    }
}

impl<R: Seek> Seek for SharedSource<R> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.lock().seek(pos)
    }
}

/// SPEC §6's **canonical layout**, over every byte of the file: whichever way a zip reader finds its
/// way into a `.tsra` (the central directory, the local headers front to back, or the end record
/// found by scanning back from the end of the file), it must see exactly the same members with
/// exactly the same bytes and the same meaning, and those members must be the ones the manifest
/// seals plus `aux/`.
///
/// Six parts, each closing a cloak the others leave open:
///
/// 1. `zip`'s [`ZipArchive::validate_header_consistency`]: every local header agrees with its
///    central-directory record, no name appears twice, no two entries share a byte.
/// 2. [`check_members`]: every entry is `mimetype`, `manifest.seal`, `manifest.json`, a block the
///    manifest lists, or
///    an `aux/` member whose name obeys the block-name grammar (ADR-0042, ADR-0063), and the `aux/`
///    names are distinct up to case with none below another ([`check_aux_name_set`]). Anything else
///    is bytes no digest covers under a name a reader would take for part of the product, or a name
///    an extracting reader would resolve outside its target directory or into another tree.
/// 3. [`check_entries_tile`]: in central-directory order, the entries cover `[0, central directory)`
///    with no gap, so no complete local entry hides in bytes no record points at (a streaming reader
///    would list it) and every reader meets the members in one order.
/// 4. [`check_local_headers`]: every local header is, byte for byte, the one tessera's writer emits
///    for that member ([`canonical_local_header`]). The tiling counts a header's extra field as part
///    of its entry, and a reader may interpret any field the check does not pin: a data-descriptor
///    flag makes a streaming reader look for the member's end somewhere else, an unknown extra field
///    can carry a whole forged entry, an encryption flag or a Unicode-path field changes what a
///    reader decodes or calls the member. Pinning every byte leaves no field to interpret.
/// 5. [`check_central_directory`]: the same for every central-directory record
///    ([`canonical_central_record`]: no comment, disk 0, a regular file's attributes), then the end
///    records exactly as the writer emits them ([`canonical_end_records`]: no archive comment, true
///    counts and offsets), then the end of the file. A second archive appended after the end record
///    (or carried in its comment) is what `unzip` and Python's `zipfile` open instead, while `zip`
///    falls back to the genuine one.
/// 6. [`check_aux_bytes`]: every `aux/` member's bytes match its CRC-32. The payload pass reads every
///    sealed member; nothing else would read an `aux/` one.
///
/// Parts 1 and 3 follow from 4 and 5 for an archive `zip` opened; they stay because each names the
/// disagreement it finds (a renamed member, a gap) where a byte comparison names only a field.
///
/// It reads every local header and the whole central directory, so it belongs to the deep verify,
/// not to [`Reader::from_reader`]: a cloud open still costs the archive's tail only, and every member
/// read is already held to its own header by [`open_member`].
fn check_one_reading<A: Read + Seek, S: Read + Seek>(
    archive: &mut ZipArchive<A>,
    raw: &mut S,
    manifest: &Manifest,
    label: &str,
) -> Result<()> {
    let refuse = |e: Error| match e {
        Error::Container(why) => Error::Container(format!("{label}: not a canonical .tsra: {why}")),
        e => e,
    };
    archive
        .validate_header_consistency()
        .map_err(|e| refuse(Error::Container(e.to_string())))?;
    check_members(archive, manifest).map_err(refuse)?;
    check_entries_tile(archive).map_err(refuse)?;
    check_local_headers(archive, raw).map_err(refuse)?;
    check_central_directory(archive, raw).map_err(refuse)?;
    check_aux_bytes(archive).map_err(refuse)
}

/// Part 2 of [`check_one_reading`]: the archive holds only what SPEC §6 permits. Duplicates are
/// already refused, so `mimetype`, `manifest.seal` and `manifest.json` occur at most once (and the
/// open already placed them first, [`require_seal_then_manifest`]); a listed block that is
/// missing fails later, when its payload is verified. The manifest must list each block name once
/// ([`Error::DuplicateBlockName`], #734). The open's [`Manifest::verify`] already refuses a repeat,
/// so through [`Reader`] this never fires; it is defence in depth, kept next to the one place that
/// collapses the listed names into a set, where a repeat would otherwise vanish.
///
/// An `aux/` member's name below the prefix obeys the block-name grammar
/// ([`tessera_core::block::name::check_name`]), the same rule [`write_aux_members`] and [`pack_dir`]
/// apply when they write one: relative, `/`-separated, no empty, `.` or `..` segment, only
/// `[A-Za-z0-9._-]`. So no `..`, no leading `/`, no `\`, no NUL or control character, no non-ASCII
/// byte, and no directory entry (its trailing `/` is an empty segment). Without it, a name like
/// `aux/../manifest.json` lands outside the target directory in an extracting reader, and a name
/// with a NUL reads as `aux/` in one reader and as the full name in another.
fn check_members<A: Read + Seek>(archive: &ZipArchive<A>, manifest: &Manifest) -> Result<()> {
    check_block_refs(&manifest.blocks)?;
    let listed: HashSet<String> = manifest
        .blocks
        .iter()
        .map(|b| format!("{BLOCKS_PREFIX}{}", b.name))
        .collect();
    let mut aux = Vec::new();
    for i in 0..archive.len() {
        let entry = archive.by_index_data(i).map_err(cz)?;
        let name = entry.name().map_err(cz)?;
        if let Some(rest) = name.strip_prefix(AUX_PREFIX) {
            tessera_core::block::name::check_name(rest).map_err(|reason| {
                Error::Container(format!(
                    "aux member {name:?} is not a safe relative path (ADR-0063): {reason}; an \
                     extracting zip reader would write it somewhere other than under `{AUX_PREFIX}`, \
                     or read a different name"
                ))
            })?;
            aux.push(rest.to_string());
            continue;
        }
        let permitted = name == MIMETYPE_ENTRY
            || name == SEAL_ENTRY
            || name == MANIFEST_ENTRY
            || listed.contains(name.as_ref());
        if !permitted {
            return Err(Error::Container(format!(
                "entry '{name}' is not `mimetype`, `manifest.seal`, `manifest.json`, a block the \
                 manifest lists, or \
                 an `{AUX_PREFIX}` member; no digest covers its bytes, and outside `{AUX_PREFIX}` a \
                 reader would take it for part of the sealed product"
            )));
        }
    }
    check_aux_name_set(aux.iter().map(String::as_str))
}

/// The `aux/` member names of one archive (below the prefix), as a set: compared with ASCII case
/// folded, no two are equal and none is a parent prefix of another (the leaf rule, ADR-0063). An
/// extracting reader writes `aux/a` as a file and needs `aux/a/b` to make it a directory, and on a
/// case-insensitive file system `aux/SIG` and `aux/sig` are one file; either way the tree on disk
/// depends on which member a reader writes first. Sealed `blocks/` names are not held to this here:
/// the leaf rule applies to them when a product is sealed, not when it is read (ADR-0063).
///
/// Every aux writer ([`write_aux_members`], [`pack_dir`]) applies it to the set it would write, and
/// the deep verify applies it to the set it finds, so no writer emits what verification refuses.
fn check_aux_name_set<'a>(names: impl IntoIterator<Item = &'a str>) -> Result<()> {
    let mut folded: Vec<(String, &str)> = names
        .into_iter()
        .map(|n| (n.to_ascii_lowercase(), n))
        .collect();
    folded.sort_unstable();
    for pair in folded.windows(2) {
        if pair[0].0 == pair[1].0 {
            return Err(Error::Container(format!(
                "aux members '{AUX_PREFIX}{}' and '{AUX_PREFIX}{}' have one name up to letter case; \
                 a case-insensitive file system holds only one of them, so what an extracting \
                 reader leaves depends on which it writes last",
                pair[0].1, pair[1].1
            )));
        }
    }
    let by_folded: std::collections::BTreeMap<&str, &str> =
        folded.iter().map(|(f, n)| (f.as_str(), *n)).collect();
    for (f, child) in &folded {
        for (i, _) in f.match_indices('/') {
            if let Some(parent) = by_folded.get(&f[..i]) {
                return Err(Error::Container(format!(
                    "aux member '{AUX_PREFIX}{child}' sits below aux member \
                     '{AUX_PREFIX}{parent}', which is a file; an extracting reader can write only \
                     one of them, so the tree it leaves depends on the order it writes them \
                     (the leaf rule, ADR-0063)"
                )));
            }
        }
    }
    Ok(())
}

/// Part 6 of [`check_one_reading`]: every `aux/` member's bytes match the CRC-32 its records carry.
/// Sealed members are read in full by the payload pass (and `manifest.json` by the open), so a CRC
/// mismatch there is already refused; an `aux/` member is not sealed and would otherwise never be
/// read. A reader that checks the CRC refuses or truncates such a member while another extracts it,
/// so the file would read differently. Each member is streamed through a fixed buffer, held to its
/// local header like every member read ([`open_member`]).
fn check_aux_bytes<A: Read + Seek>(archive: &mut ZipArchive<A>) -> Result<()> {
    for i in 0..archive.len() {
        let name = archive
            .by_index_data(i)
            .map_err(cz)?
            .name()
            .map_err(cz)?
            .into_owned();
        if !name.starts_with(AUX_PREFIX) {
            continue;
        }
        let opts = ZipReadOptions::new().enforce_header_consistency(true);
        let mut f = archive.by_index_with_options(i, opts).map_err(cz)?;
        std::io::copy(&mut f, &mut std::io::sink()).map_err(|e| {
            if e.kind() == std::io::ErrorKind::InvalidData {
                Error::Container(format!(
                    "aux member '{name}': {e}: its bytes are not the ones its CRC-32 records, so \
                     a reader that checks the CRC refuses or truncates it while another extracts it"
                ))
            } else {
                Error::Io(e)
            }
        })?;
    }
    Ok(())
}

/// Part 3 of [`check_one_reading`]: **in central-directory order**, the first entry starts at byte 0,
/// each starts exactly where the previous one's bytes end, and the last ends exactly where the
/// central directory starts. A complete local entry hidden in a gap disagrees with no record, so every
/// header check passes, yet a streaming reader walking the local headers reads it as one more member.
/// And the central directory lists the members in the order they sit in the file, so a reader
/// walking the directory and one walking the local headers meet the members in the same order: an
/// extracting reader that writes them in turn ends with the same tree either way. Every tessera writer
/// has always produced this layout (no prefix, no data descriptor, no padding, the directory written
/// in the order the entries were).
fn check_entries_tile<A: Read + Seek>(archive: &mut ZipArchive<A>) -> Result<()> {
    let mut spans = Vec::with_capacity(archive.len());
    for i in 0..archive.len() {
        let f = archive.by_index_raw(i).map_err(cz)?;
        let name = f.name().map_err(cz)?.into_owned();
        let data_start = f.data_start().ok_or_else(|| {
            Error::Container(format!(
                "entry '{name}' has no resolved data offset after its local header was read"
            ))
        })?;
        let end = data_start.checked_add(f.compressed_size()).ok_or_else(|| {
            Error::Container(format!("entry '{name}' ends past the largest file offset"))
        })?;
        spans.push((f.header_start(), end, name));
    }
    if let Some(w) = spans.windows(2).find(|w| w[1].0 < w[0].0) {
        return Err(Error::Container(format!(
            "the central directory lists '{}' (at byte {}) after '{}' (at byte {}): it lists the \
             members in another order than the file holds them, so a reader walking the directory \
             and one walking the local headers meet them in different orders",
            w[1].2, w[1].0, w[0].2, w[0].0
        )));
    }
    let hidden =
        "bytes no central-directory record covers can hide a member that a reader walking \
                  the local headers would see";
    let mut cursor = 0u64;
    for (start, end, name) in &spans {
        if *start != cursor {
            return Err(Error::Container(format!(
                "entry '{name}' starts at byte {start}, but the previous entry ends at byte \
                 {cursor}; {hidden}"
            )));
        }
        cursor = *end;
    }
    let cd_start = archive.central_directory_start();
    if cursor != cd_start {
        return Err(Error::Container(format!(
            "the last entry ends at byte {cursor}, but the central directory starts at byte \
             {cd_start}; {hidden}"
        )));
    }
    Ok(())
}

// ── The canonical encoding (SPEC §6) ────────────────────────────────────────────────────────────
//
// Every value below is what `zip` writes under [`tsra_entry_options`] for a member whose name obeys
// the block-name grammar (ASCII, so no UTF-8 flag) — the encoding every tessera writer has always
// produced. The reader holds each record to it byte for byte, so a field is either pinned here or
// derived from the member's name, CRC, size and offset; none is left for a reader to interpret.

/// Signature of a local file header (`PK\3\4`).
const LOCAL_HEADER_SIG: [u8; 4] = *b"PK\x03\x04";
/// Signature of a central-directory record (`PK\1\2`).
const CENTRAL_RECORD_SIG: [u8; 4] = *b"PK\x01\x02";
/// Signature of the end-of-central-directory record (`PK\5\6`), and its size with no comment.
const EOCD_SIG: [u8; 4] = *b"PK\x05\x06";
const EOCD_LEN: usize = 22;
/// Signature of the zip64 end-of-central-directory record (`PK\6\6`).
const ZIP64_EOCD_SIG: [u8; 4] = *b"PK\x06\x06";
/// Signature of the zip64 end-of-central-directory locator (`PK\6\7`).
const ZIP64_LOCATOR_SIG: [u8; 4] = *b"PK\x06\x07";
/// "Version needed to extract": 4.5, zip64 (every entry carries a zip64 extra field).
const VERSION_NEEDED: u16 = 45;
/// "Version made by": host 3 (Unix, so the external attributes are a Unix mode) and version 4.5.
const VERSION_MADE_BY: u16 = (3 << 8) | VERSION_NEEDED;
/// General-purpose flags: none. No encryption (bit 0), no data descriptor (bit 3), no patch data,
/// no strong encryption (bit 6), no UTF-8 flag (bit 11; every name is ASCII), nothing reserved.
const FLAGS: u16 = 0;
/// Compression method 0, STORED.
const METHOD_STORED: u16 = 0;
/// DOS time and date of the zip epoch, 1980-01-01 00:00:00 (the pinned modification time).
const EPOCH_TIME: u16 = 0;
const EPOCH_DATE: u16 = (1 << 5) | 1;
/// External attributes: a regular file, mode `0644` (`S_IFREG | 0644` in the high 16 bits). Not a
/// symlink, not a directory, so an extracting reader writes a plain file.
const EXTERNAL_ATTRS: u32 = 0o100_644 << 16;
/// The zip64 extended-information extra field's header id.
const ZIP64_EXTRA_ID: u16 = 0x0001;
/// The 32-bit "see the zip64 field" sentinel (`zip`'s zip64 byte threshold).
const SENTINEL32: u32 = u32::MAX;
/// The 16-bit entry-count sentinel (`zip`'s zip64 entry threshold).
const SENTINEL16: u16 = u16::MAX;

/// A record's canonical bytes, and where each of its fields starts (to name the one that differs).
struct Canonical {
    bytes: Vec<u8>,
    /// (start, field name, whether the field is a little-endian integer rather than raw bytes).
    fields: Vec<(usize, &'static str, bool)>,
}

impl Canonical {
    fn new() -> Self {
        Canonical {
            bytes: Vec::with_capacity(128),
            fields: Vec::with_capacity(24),
        }
    }

    fn push(&mut self, name: &'static str, bytes: &[u8], int: bool) -> &mut Self {
        self.fields.push((self.bytes.len(), name, int));
        self.bytes.extend_from_slice(bytes);
        self
    }

    fn field(&mut self, name: &'static str, bytes: &[u8]) -> &mut Self {
        self.push(name, bytes, false)
    }

    fn u16(&mut self, name: &'static str, v: u16) -> &mut Self {
        self.push(name, &v.to_le_bytes(), true)
    }

    fn u32(&mut self, name: &'static str, v: u32) -> &mut Self {
        self.push(name, &v.to_le_bytes(), true)
    }

    fn u64(&mut self, name: &'static str, v: u64) -> &mut Self {
        self.push(name, &v.to_le_bytes(), true)
    }

    fn len(&self) -> usize {
        self.bytes.len()
    }

    /// `found` must be these bytes; otherwise [`Error::Container`] naming `record` and the first field
    /// that differs, with both values. A `found` shorter than the record (the file ended) differs
    /// where it stops.
    fn require(&self, found: &[u8], record: &str) -> Result<()> {
        if found == self.bytes.as_slice() {
            return Ok(());
        }
        let at = found
            .iter()
            .zip(&self.bytes)
            .position(|(a, b)| a != b)
            .unwrap_or(found.len().min(self.bytes.len()));
        let i = self
            .fields
            .iter()
            .rposition(|(start, _, _)| *start <= at)
            .unwrap_or(0);
        let (start, field, int) = self.fields[i];
        let end = self.fields.get(i + 1).map_or(self.bytes.len(), |f| f.0);
        let width = end - start;
        let show = |b: &[u8]| -> String {
            if b.len() < width {
                format!("cut short by the end of the file after {} bytes", b.len())
            } else if int {
                let v = b.iter().rev().fold(0u64, |v, &x| (v << 8) | u64::from(x));
                format!("{v:#0w$x}", w = 2 + 2 * width)
            } else {
                format!("\"{}\"", b.escape_ascii())
            }
        };
        Err(Error::Container(format!(
            "{record}: its {field} is {} where tessera's writer emits {}; every record of a .tsra is \
             byte for byte the one the writer emits, so no zip reader finds a meaning in it that \
             verification did not check",
            show(&found[start.min(found.len())..end.min(found.len())]),
            show(&self.bytes[start..end]),
        )))
    }
}

/// The local file header tessera's writer emits for a member (APPNOTE 4.3.7): sizes in a zip64
/// extra field, the 32-bit size fields holding the sentinel, and nothing else in the extra field.
fn canonical_local_header(name: &[u8], crc: u32, size: u64) -> Result<Canonical> {
    let name_len = u16::try_from(name.len())
        .map_err(|_| Error::Container("an entry name is longer than 65,535 bytes".into()))?;
    let mut c = Canonical::new();
    c.field("signature", &LOCAL_HEADER_SIG)
        .u16("version needed to extract", VERSION_NEEDED)
        .u16("general-purpose flags", FLAGS)
        .u16("compression method", METHOD_STORED)
        .u16("modification time", EPOCH_TIME)
        .u16("modification date", EPOCH_DATE)
        .u32("CRC-32", crc)
        .u32("32-bit compressed size", SENTINEL32)
        .u32("32-bit uncompressed size", SENTINEL32)
        .u16("name length", name_len)
        .u16("extra-field length", 20)
        .field("name", name)
        .u16("extra-field id", ZIP64_EXTRA_ID)
        .u16("zip64 field length", 16)
        .u64("zip64 uncompressed size", size)
        .u64("zip64 compressed size", size);
    Ok(c)
}

/// The central-directory record tessera's writer emits for a member (APPNOTE 4.3.12): the local
/// header's fields, plus no comment, disk 0, no internal attributes and a regular file's mode. The
/// zip64 extra field carries the sizes, and the local header's offset too once that offset no longer
/// fits 32 bits (the 32-bit offset field then holds the sentinel), exactly as `zip` decides it.
fn canonical_central_record(name: &[u8], crc: u32, size: u64, offset: u64) -> Result<Canonical> {
    let name_len = u16::try_from(name.len())
        .map_err(|_| Error::Container("an entry name is longer than 65,535 bytes".into()))?;
    let offset_in_zip64 = offset >= u64::from(SENTINEL32);
    let zip64_len: u16 = if offset_in_zip64 { 24 } else { 16 };
    let mut c = Canonical::new();
    c.field("signature", &CENTRAL_RECORD_SIG)
        .u16("version made by", VERSION_MADE_BY)
        .u16("version needed to extract", VERSION_NEEDED)
        .u16("general-purpose flags", FLAGS)
        .u16("compression method", METHOD_STORED)
        .u16("modification time", EPOCH_TIME)
        .u16("modification date", EPOCH_DATE)
        .u32("CRC-32", crc)
        .u32("32-bit compressed size", SENTINEL32)
        .u32("32-bit uncompressed size", SENTINEL32)
        .u16("name length", name_len)
        .u16("extra-field length", 4 + zip64_len)
        .u16("comment length", 0)
        .u16("disk number start", 0)
        .u16("internal attributes", 0)
        .u32("external attributes", EXTERNAL_ATTRS)
        .u32(
            "32-bit local header offset",
            u32::try_from(offset).unwrap_or(SENTINEL32),
        )
        .field("name", name)
        .u16("extra-field id", ZIP64_EXTRA_ID)
        .u16("zip64 field length", zip64_len)
        .u64("zip64 uncompressed size", size)
        .u64("zip64 compressed size", size);
    if offset_in_zip64 {
        c.u64("zip64 local header offset", offset);
    }
    Ok(c)
}

/// The end records tessera's writer emits after a central directory of `entries` records, `cd_size`
/// bytes long, starting at byte `cd_start` (APPNOTE 4.3.14-16, decided as `zip` decides it): the
/// zip64 end record and its locator only when the archive needs them (more than 65,535 entries, or a
/// directory size or offset past 32 bits), then the end-of-central-directory record with no comment.
/// Every count, size and offset is the true value, or its sentinel when that value lives in the zip64
/// record.
fn canonical_end_records(entries: u64, cd_start: u64, cd_size: u64) -> Canonical {
    let zip64 = entries > u64::from(SENTINEL16) || cd_size.max(cd_start) > u64::from(SENTINEL32);
    let mut c = Canonical::new();
    if zip64 {
        c.field("zip64 end record signature", &ZIP64_EOCD_SIG)
            .u64("zip64 end record size", 44)
            .u16("zip64 end record version made by", VERSION_NEEDED)
            .u16("zip64 end record version needed", VERSION_NEEDED)
            .u32("zip64 end record disk number", 0)
            .u32("zip64 end record directory disk", 0)
            .u64("zip64 end record entries on this disk", entries)
            .u64("zip64 end record entry count", entries)
            .u64("zip64 end record directory size", cd_size)
            .u64("zip64 end record directory offset", cd_start)
            .field("zip64 locator signature", &ZIP64_LOCATOR_SIG)
            .u32("zip64 locator's zip64 record disk", 0)
            .u64("zip64 locator's zip64 record offset", cd_start + cd_size)
            .u32("zip64 locator's disk count", 1);
    }
    let count = u16::try_from(entries).unwrap_or(SENTINEL16);
    let size32 = if zip64 {
        SENTINEL32
    } else {
        u32::try_from(cd_size).unwrap_or(SENTINEL32)
    };
    c.field("end record signature", &EOCD_SIG)
        .u16("end record disk number", 0)
        .u16("end record directory disk", 0)
        .u16("end record entries on this disk", count)
        .u16("end record entry count", count)
        .u32("end record directory size", size32)
        .u32(
            "end record directory offset",
            u32::try_from(cd_start).unwrap_or(SENTINEL32),
        )
        .u16("end record comment length", 0);
    c
}

/// Up to `n` bytes from `r`'s current position: fewer only where the file ends.
fn read_up_to<S: Read>(r: &mut S, n: usize) -> Result<Vec<u8>> {
    let mut b = Vec::with_capacity(n);
    r.take(n as u64).read_to_end(&mut b)?;
    Ok(b)
}

/// Part 4 of [`check_one_reading`]: every local header is the canonical one for its member, read
/// from where its central-directory record says it is.
fn check_local_headers<A: Read + Seek, S: Read + Seek>(
    archive: &ZipArchive<A>,
    raw: &mut S,
) -> Result<()> {
    for i in 0..archive.len() {
        let entry = archive.by_index_data(i).map_err(cz)?;
        let want =
            canonical_local_header(entry.name_raw(), entry.crc32(), entry.compressed_size())?;
        raw.seek(SeekFrom::Start(entry.header_start()))?;
        let found = read_up_to(raw, want.len())?;
        want.require(
            &found,
            &format!(
                "the local header of '{}' at byte {}",
                String::from_utf8_lossy(entry.name_raw()),
                entry.header_start()
            ),
        )?;
    }
    Ok(())
}

/// Part 5 of [`check_one_reading`]: every byte from the central directory to the end of the file is
/// accounted for. The central directory is exactly its canonical records back to back, in `zip`'s
/// order; then come exactly the canonical end records; then the end of the file. So the end record a
/// reader finds by scanning back from the end of the file is the one `zip` uses, and no reader can be
/// steered to other bytes.
fn check_central_directory<A: Read + Seek, S: Read + Seek>(
    archive: &ZipArchive<A>,
    raw: &mut S,
) -> Result<()> {
    let file_len = raw.seek(SeekFrom::End(0))?;
    let cd_start = archive.central_directory_start();
    raw.seek(SeekFrom::Start(cd_start))?;
    let mut tail = std::io::BufReader::new(&mut *raw);
    let mut pos = cd_start;
    for i in 0..archive.len() {
        let entry = archive.by_index_data(i).map_err(cz)?;
        let want = canonical_central_record(
            entry.name_raw(),
            entry.crc32(),
            entry.compressed_size(),
            entry.header_start(),
        )?;
        let found = read_up_to(&mut tail, want.len())?;
        want.require(
            &found,
            &format!(
                "central-directory record #{i} ('{}') at byte {pos}",
                String::from_utf8_lossy(entry.name_raw())
            ),
        )?;
        pos += want.len() as u64;
    }
    let (cd_end, cd_size) = (pos, pos - cd_start);
    let want = canonical_end_records(archive.len() as u64, cd_start, cd_size);
    let rest = file_len.saturating_sub(cd_end);
    if rest != want.len() as u64 {
        return Err(Error::Container(format!(
            "{rest} bytes follow the central directory (which ends at byte {cd_end}), but this \
             archive's end records are exactly {} bytes (the end-of-central-directory record{}), \
             with no archive comment and nothing after them. Extra bytes there can be a second \
             archive that other zip readers open instead",
            want.len(),
            if want.len() == EOCD_LEN {
                ""
            } else {
                ", after the zip64 end record and its locator"
            },
        )));
    }
    let found = read_up_to(&mut tail, want.len())?;
    want.require(&found, &format!("the end records at byte {cd_end}"))
}

/// What both packers require of the manifest, before they create any file: it is sealed, it lists
/// each block name once ([`Error::DuplicateBlockName`], ADR-0063 §2a), and each chunk index indexes
/// another of its blocks ([`Error::DanglingChunkIndex`], #720). Readers refuse either, so no writer
/// packs one, however a sealed manifest was made (its fields are public). Table groups must
/// also share column names, dtypes and nullability ([`Error::LogicalTable`], #978).
fn check_packable(manifest: &Manifest) -> Result<()> {
    if !manifest.is_sealed() {
        return Err(Error::Container(
            "refusing to pack an unsealed manifest".into(),
        ));
    }
    check_block_refs(&manifest.blocks)?;
    tessera_core::block::partition::check_table_contracts(&manifest.blocks)
}

/// Pack a **sealed** manifest + its block payloads into a `.tsra` at `path` (STORED zip64).
///
/// Every payload name is a [`BlockName`], so every entry lands under `blocks/` (ADR-0063). A
/// manifest listing a name twice is refused with [`Error::DuplicateBlockName`] before `path` is
/// created. The leaf rule is not re-checked: the manifest is already sealed
/// ([`tessera_core::ProductBuilder::seal`] enforces it), and a product sealed before ADR-0063 (a
/// `volume` + `volume/1` pyramid) is still a valid zip that readers open, so re-packaging it (e.g.
/// to add an `aux/` member) keeps working.
///
/// The manifest is written as [`crate::stored::manifest`] (ADR-0066): `manifest.json` is exactly
/// its JCS bytes and `manifest.seal` their digest, from [`Manifest::to_stored`], so a manifest whose
/// bytes don't carry its own seal (edited after sealing, or a parsed view with newer fields) is
/// refused, not packed.
pub fn pack(manifest: &Manifest, payloads: &[BlockPayload], path: &Path) -> Result<()> {
    check_packable(manifest)?;
    let (json, seal) = crate::stored::manifest(manifest)?;
    pack_manifest_json(&json, &seal, payloads, path)
}

/// The leading entries every `.tsra` writer emits, in SPEC §6's order: `mimetype`, then
/// `manifest.seal`, then `manifest.json`. The one place the order is spelled, so [`pack`],
/// [`pack_streaming`] and [`write_aux_members`] can't drift.
fn write_leading_entries<W: Write + Seek>(
    zw: &mut ZipWriter<W>,
    manifest_json: &[u8],
    seal: &str,
) -> Result<()> {
    let stored = tsra_entry_options();
    zw.start_file(MIMETYPE_ENTRY, stored).map_err(cz)?; // first + uncompressed = magic
    zw.write_all(MIMETYPE.as_bytes())?;
    zw.start_file(SEAL_ENTRY, stored).map_err(cz)?;
    zw.write_all(seal.as_bytes())?;
    zw.start_file(MANIFEST_ENTRY, stored).map_err(cz)?;
    zw.write_all(manifest_json)?;
    Ok(())
}

/// [`pack`]'s body, over a stored manifest pair ([`crate::stored`]) the caller has already sealed
/// or read through [`Manifest::from_stored_members`].
fn pack_manifest_json(
    manifest_json: &[u8],
    seal: &str,
    payloads: &[BlockPayload],
    path: &Path,
) -> Result<()> {
    let mut zw = ZipWriter::new(File::create(path)?);
    // STORED + force zip64 so a large study is never silently truncated at 4 GiB / 65 k entries.
    // Pin the entry mtime to the zip epoch (1980-01-01) so the same product packs to byte-identical
    // bytes — writer-determinism is a release gate (FEATURE-MATRIX §C); a wall-clock mtime breaks it.
    let stored = tsra_entry_options();
    write_leading_entries(&mut zw, manifest_json, seal)?;

    for p in payloads {
        zw.start_file(format!("{BLOCKS_PREFIX}{}", p.name), stored)
            .map_err(cz)?;
        zw.write_all(&p.bytes)?;
    }
    zw.finish().map_err(cz)?;
    Ok(())
}

/// **Constant-memory peer of [`pack`]**: pack a sealed manifest by **streaming** each block fragment
/// **from disk** into the zip via [`std::io::copy`], never holding a block payload in RAM. Required
/// for the multi-block listmode path where a single block can be hundreds of MiB; sealing the whole
/// product through [`pack`] would defeat the bounded-memory write engine the entire pipeline exists
/// to provide.
///
/// `sources` is the in-order list of `(block-name, fragment-path)` pairs — `name` becomes the zip
/// entry suffix (`blocks/<name>`), `path` is opened + buffered + streamed. The names + order MUST
/// match the manifest's `blocks` list (the caller passes them straight from the session's committed
/// refs). The output is **byte-identical** to `pack(manifest, &Vec<BlockPayload>::from(sources), out)`
/// — proven by `pack_streaming_equals_pack` below — because both writers share [`tsra_entry_options`]
/// and emit the mimetype/seal/manifest/blocks in the same order. Like [`pack`], it refuses an unsealed
/// manifest, one listing a block name twice ([`Error::DuplicateBlockName`]) or one holding a chunk
/// index without its block ([`Error::DanglingChunkIndex`]), or a table group's mismatched column
/// contracts ([`Error::LogicalTable`]), before creating `out`.
pub fn pack_streaming(
    manifest: &Manifest,
    sources: &[(BlockName, &Path)],
    out: &Path,
) -> Result<()> {
    pack_streaming_impl(manifest, sources, out, false)
}

/// **Verifying streaming pack** — identical bytes-on-disk to [`pack_streaming`], with one extra
/// guarantee: as each fragment is copied into the zip its bytes are fed through a streaming blake3
/// (one read, no extra buffering) and the resulting digest is compared to the `BlockRef.digest`
/// recorded in the manifest. A mismatch returns [`Error::Integrity`] with `what = "block_payload"`
/// **before** the archive finalises, so a fragment that changed between "hash" and "pack" can never
/// be silently sealed into a `.tsra` whose recorded `digest` no longer matches its packed bytes
/// (the race the blob streaming ingest opens by hashing the source file first and copying it
/// second). For a blob block whose fragment is the **original source file**, this turns a concurrent
/// writer / atomic-replace between the two reads into a loud `Err(Integrity)` instead of a
/// dead-on-arrival archive that only surfaces on the first read.
///
/// **Determinism contract**: the output is byte-identical to [`pack_streaming`] over the same
/// inputs (same SSoT [`tsra_entry_options`], same entry order, same copy loop — the verify happens
/// on the bytes already buffered for the write, never reordering anything). The conformance corpus
/// goldens MUST remain untouched.
///
/// Refuses (without writing) any fragment whose name has no matching block in the manifest, and any
/// matching block whose [`BlockRef::digest`] is `None` (sealed manifests always have one — an absent
/// digest is a malformed input, not a verify miss).
pub fn pack_streaming_verified(
    manifest: &Manifest,
    sources: &[(BlockName, &Path)],
    out: &Path,
) -> Result<()> {
    pack_streaming_impl(manifest, sources, out, true)
}

/// Stage-and-rename wrapper for [`pack_streaming`] / [`pack_streaming_verified`]: writes the archive
/// to a sibling `.part` via [`pack_streaming_to`] and atomically renames it to `out` only on success,
/// so a failure never leaves a partial / known-bad `.tsra` at the destination.
fn pack_streaming_impl(
    manifest: &Manifest,
    sources: &[(BlockName, &Path)],
    out: &Path,
    verify: bool,
) -> Result<()> {
    // Stage to a sibling `.part` and atomically rename only on success, so any failure — a write
    // error or a `verify` mismatch — never leaves a partial / known-bad `.tsra` at `out` (same
    // crash-safe placement `tessera extract` uses). The bytes written are unchanged, so the
    // determinism contract / corpus goldens hold.
    let mut tmp = out.as_os_str().to_os_string();
    tmp.push(".part");
    let tmp = std::path::PathBuf::from(tmp);
    match pack_streaming_to(manifest, sources, &tmp, verify) {
        Ok(()) => {
            std::fs::rename(&tmp, out)?;
            Ok(())
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// Write the `.tsra` to `out` (a staging path) — the body shared by [`pack_streaming`] /
/// [`pack_streaming_verified`]: identical zip entries + identical copy loop, with `verify` flipping
/// the per-fragment hash check on. Kept as one function so the two surfaces can't drift in entry
/// order / options / buffer size (a drift would change the on-disk bytes and break the corpus).
fn pack_streaming_to(
    manifest: &Manifest,
    sources: &[(BlockName, &Path)],
    out: &Path,
    verify: bool,
) -> Result<()> {
    check_packable(manifest)?;
    let (json, seal) = crate::stored::manifest(manifest)?;
    let mut zw = ZipWriter::new(File::create(out)?);
    let stored = tsra_entry_options();
    write_leading_entries(&mut zw, &json, &seal)?;

    for (name, frag) in sources {
        // Resolve the manifest-recorded digest UP FRONT (only when verifying) so a typo'd name or
        // a malformed unsealed-style ref fails before we open the file.
        let expected_digest = if verify {
            let r = manifest
                .blocks
                .iter()
                .find(|b| b.name == *name)
                .ok_or_else(|| {
                    Error::Container(format!(
                        "pack_streaming_verified: no manifest block named '{name}'"
                    ))
                })?;
            Some(r.digest.clone().ok_or_else(|| {
                Error::MissingDigest(format!(
                    "pack_streaming_verified: block '{name}' has no recorded digest"
                ))
            })?)
        } else {
            None
        };

        zw.start_file(format!("{BLOCKS_PREFIX}{name}"), stored)
            .map_err(cz)?;
        // Buffered copy: 256 KiB chunks → peak RAM ≈ one buffer (no full-block materialisation).
        // When verifying, we hash the buffer in the same pass — one read, no extra I/O — so the
        // on-disk bytes are identical to the non-verifying path (the conformance corpus stays
        // bit-for-bit stable).
        // Read full-buffer chunks straight from the file — a BufReader would only add a second
        // 256 KiB buffer with no benefit at this read size.
        let mut f = File::open(frag)?;
        let mut hasher = verify.then(tessera_core::hash::StreamHasher::new);
        let mut buf = [0u8; 256 * 1024];
        loop {
            let n = f.read(&mut buf)?;
            if n == 0 {
                break;
            }
            if let Some(h) = hasher.as_mut() {
                h.update(&buf[..n]);
            }
            zw.write_all(&buf[..n])?;
        }
        if let (Some(h), Some(exp)) = (hasher, expected_digest) {
            let actual = h.finalize();
            if actual != exp {
                // The fragment changed between hash and pack. Bail without finalising; the wrapper
                // discards the staged `.part`, so nothing lands at the destination.
                return Err(Error::Integrity {
                    what: "block_payload",
                    expected: exp,
                    actual,
                });
            }
        }
    }
    zw.finish().map_err(cz)?;
    Ok(())
}

/// A reader over a `.tsra`. Generic over any `Read + Seek` byte source, so the same reader serves
/// a local file today and an object-store range-reader (S6) later.
pub struct Reader<R: Read + Seek> {
    archive: ZipArchive<SharedSource<R>>,
    /// The same source `archive` reads, for the bytes no zip record points at ([`SharedSource`]).
    source: Arc<Mutex<R>>,
    manifest: Manifest,
    /// The `manifest.json` member exactly as stored, which the seal was verified over (SPEC §4).
    /// Its seal is [`Manifest::manifest_hash`] of [`manifest`](Self::manifest).
    manifest_json: String,
}

impl Reader<File> {
    /// Open + verify a `.tsra` file: magic, manifest parse, and full seal verification.
    pub fn open(path: &Path) -> Result<Self> {
        Self::from_reader(File::open(path)?)
    }
}

impl<R: Read + Seek> Reader<R> {
    /// Build a reader from any seekable byte source. Checks the `mimetype` magic and the leading
    /// entries, then reads `manifest.json` and `manifest.seal` through
    /// [`Manifest::from_stored_members`]: the canonical check, the seal over the stored bytes, then
    /// `id` and `content_hash` (ADR-0066 rule 4). A product without `manifest.seal` (the alpha
    /// layout) is refused with [`Error::UnsupportedLayout`], naming what it found and what is
    /// required; there is no migration (ADR-0066 rule 10).
    pub fn from_reader(reader: R) -> Result<Self> {
        let source = Arc::new(Mutex::new(reader));
        let mut archive = open_tsra_archive(SharedSource(Arc::clone(&source)))?;

        // **Every entry must be STORED — checked before ANY member is read.**
        //
        // The invariant itself is the one the module docs open with, and it is load-bearing rather
        // than stylistic: a block's bytes are addressable by range only because they are stored
        // verbatim, which is what lets a cloud reader fetch one block out of a multi-GB archive. A
        // deflated entry still *verifies* — the bytes decompress to the sealed content — so nothing
        // downstream would notice; the product would simply stop being range-readable.
        //
        // It used to be enforced by **accident**: the workspace pinned
        // `zip = { default-features = false }`, so no decompressor was compiled in and a compressed
        // member failed to decode. The `.npz` reader added in this same change needs `deflate-flate2`,
        // and cargo unifies features workspace-wide, so `tessera-io`'s `zip` gains deflate whether it
        // wants it or not. A guarantee that depends on a dependency *lacking* a feature is not a
        // guarantee.
        //
        // **Ordering matters, and getting it wrong was a real hole.** An earlier revision ran this scan
        // after reading `mimetype`, to keep the friendlier "not a .tsra" error for a random zip. But
        // that read is unbounded, so a `.tsra` whose *mimetype member* is a deflate bomb exhausted
        // memory before the scan could reject it — a guard placed behind the thing it guards. The scan
        // now runs first and touches no payload: `ZipArchive` parsed the central directory at open, so
        // the method and the member names are already in hand.
        //
        // A missing or misplaced `mimetype` never gets this far: `open_tsra_archive` already refused it
        // from the central directory, before any member was read. What is left here is a compressed
        // member anywhere after it.
        //
        // On which compression method this is: `zip` takes it from the **central directory** for both
        // this check (`by_index_raw` → `get_metadata().compression_method`) and the actual decode
        // (`by_index_with_options` → `ZipFileData::make_reader` → `self.compression_method`).
        // `ZipFileData::data_start` (via `find_content`) parses the local header only to locate the
        // payload, never to choose a codec (re-checked against zip 9.0.5). So a crafted file whose
        // local header disagrees with its central directory cannot slip past this check and then be
        // decompressed anyway — there is one source of truth, and it is the one checked here. A local
        // header that claims a codec anyway is refused on every member read by [`open_member`], since
        // a streaming reader would decode it (GHSA-m665-v7pm-7grj).
        let mut compressed: Option<(String, CompressionMethod)> = None;
        for i in 0..archive.len() {
            let f = archive.by_index_raw(i).map_err(cz)?;
            // Decoding every name here is also what lets [`Reader::aux_names`] stay infallible: an
            // archive with an undecodable entry name never becomes a `Reader`.
            let name = f.name().map_err(cz)?;
            if compressed.is_none() && f.compression() != CompressionMethod::Stored {
                compressed = Some((name.into_owned(), f.compression()));
            }
        }
        if let Some((name, method)) = compressed {
            return Err(Error::Container(format!(
                "entry '{name}' is {method:?}, but a .tsra must be STORED throughout — a compressed \
                 entry is not range-readable, so a cloud reader could not fetch one block without the \
                 whole archive"
            )));
        }

        // Bounded even though every entry is now known to be STORED: the magic is a short fixed
        // string, so a member claiming to be `mimetype` and carrying gigabytes is malformed by
        // definition, and `read_to_string` would happily allocate all of it.
        let mut magic = String::new();
        {
            let mut mime = open_member(&mut archive, MIMETYPE_ENTRY, || {
                "not a .tsra (no mimetype entry)".into()
            })?
            .take(64);
            mime.read_to_string(&mut magic)?;
        }
        if magic != MIMETYPE {
            return Err(Error::Container(format!("bad container magic: {magic:?}")));
        }

        let seal = match archive.index_for_name(SEAL_ENTRY) {
            None => None,
            Some(_) => {
                // Bounded like the magic: a seal is 71 bytes, so one longer is malformed and is
                // refused by its format without reading the rest.
                let mut s = Vec::new();
                open_member(&mut archive, SEAL_ENTRY, || "missing manifest.seal".into())?
                    .take(tessera_core::stored::SEAL_LEN as u64 + 1)
                    .read_to_end(&mut s)?;
                Some(s)
            }
        };
        let mut mj = Vec::new();
        open_member(&mut archive, MANIFEST_ENTRY, || {
            "missing manifest.json".into()
        })?
        .read_to_end(&mut mj)?;
        let manifest = Manifest::from_stored_members(&mj, seal.as_deref())?;
        // `from_stored_members` refused bytes that are not UTF-8, so this cannot fail.
        let mj = String::from_utf8(mj).map_err(|e| Error::Container(e.to_string()))?;

        Ok(Reader {
            archive,
            source,
            manifest,
            manifest_json: mj,
        })
    }

    /// The verified manifest (read without touching any payload — partial-product access).
    ///
    /// A typed *view* of [`manifest_json`](Self::manifest_json): fields from a newer format revision
    /// are listed by [`Manifest::newer_fields`], not carried.
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The `manifest.json` member exactly as stored: the bytes the seal was verified over on open.
    /// Copy these, not [`manifest`](Self::manifest)'s JSON, to re-emit the product unchanged.
    pub fn manifest_json(&self) -> &str {
        &self.manifest_json
    }

    /// Names of the blocks recorded in the manifest.
    pub fn block_names(&self) -> Vec<String> {
        self.manifest
            .blocks
            .iter()
            .map(|b| b.name.to_string())
            .collect()
    }

    /// Resolve the table name `name` (ADR-0064 §3) over the manifest alone, reading no block: a
    /// single table block, a partitioned table's leaves (`pairs/0000`, `pairs/0001`, … in index
    /// order), or a legacy flat group (`events_0000`, …, products written before ADR-0064). Every
    /// logical read ([`Self::logical_table`]) goes through it, and through
    /// [`tessera_core::block::table_group`], the function the seal validates with, so `pairs`
    /// means the same table everywhere.
    ///
    /// A name that doesn't resolve is [`Error::LogicalTable`], whose `reason` says why: not found,
    /// not a table (an array, a blob, or a node with no marked leaf), ambiguous between the two
    /// forms, or an invalid partitioned node (a gap, a missing terminal leaf, a foreign child, …).
    /// Every block stays readable by its full name through [`Self::read_block`].
    pub fn table_group(&self, name: &str) -> Result<TableGroup> {
        tessera_core::block::table_group(&self.manifest.blocks, name)
    }

    /// Names of the blocks in the **legacy flat** group of `prefix`, in manifest order: `prefix`
    /// itself and every `prefix_NNNN` (exactly four digits), the form written before ADR-0064. It
    /// does not see a partitioned table (`prefix/0000`, …) and checks nothing; an unknown prefix is
    /// an empty list.
    #[deprecated(
        note = "use `Reader::table_group`, which also resolves partitioned tables (`<table>/NNNN`, \
                ADR-0064) and refuses an invalid or ambiguous name with a typed error"
    )]
    pub fn block_group(&self, prefix: &str) -> Vec<String> {
        self.manifest
            .blocks
            .iter()
            .filter(|b| is_legacy_flat_member(&b.name, prefix))
            .map(|b| b.name.to_string())
            .collect()
    }

    /// Read one block's bytes by name — a targeted read of just that entry (located via the
    /// central directory), then verified against the manifest's recorded digest. A byte that
    /// doesn't match the seal is a typed [`Error::Integrity`].
    pub fn read_block(&mut self, name: &str) -> Result<Vec<u8>> {
        let entry = format!("{BLOCKS_PREFIX}{name}");
        let buf = {
            let mut f = open_member(&mut self.archive, &entry, || {
                format!("no block entry '{entry}'")
            })?;
            let mut b = Vec::with_capacity(f.size() as usize);
            f.read_to_end(&mut b)?;
            b
        };
        if let Some(expected) = self
            .manifest
            .blocks
            .iter()
            .find(|b| b.name == name)
            .and_then(|b| b.digest.as_ref())
        {
            let actual = tessera_core::hash::digest(&buf);
            if &actual != expected {
                return Err(Error::Integrity {
                    what: "block_payload",
                    expected: expected.clone(),
                    actual,
                });
            }
        }
        Ok(buf)
    }

    /// Names of every **auxiliary member** carried inside this `.tsra` (any zip entry under
    /// [`AUX_PREFIX`], returned WITHOUT the prefix — `aux/signatures/abc.sig.json` → `signatures/abc.sig.json`).
    /// Aux members ride *inside* the one shareable file but are **outside the seal** (ADR-0042):
    /// they are not referenced by `manifest.blocks` and therefore do not enter `content_hash` /
    /// `manifest_hash`. Returned in central-directory order — the same order [`pack`] emits, so a
    /// caller iterating over `aux_names()` sees a stable listing.
    pub fn aux_names(&self) -> Vec<String> {
        (0..self.archive.len())
            // `.ok()` drops nothing: `from_reader` already decoded every entry name and refused the
            // archive if one failed, so each `name_for_index` here is `Some(Ok(_))`.
            .filter_map(|i| self.archive.name_for_index(i)?.ok().map(|n| n.into_owned()))
            .filter_map(|n| n.strip_prefix(AUX_PREFIX).map(str::to_string))
            .filter(|n| !n.is_empty()) // the bare `aux/` directory marker, if any, is not a member
            .collect()
    }

    /// Read one aux member's bytes verbatim by its name **without** the [`AUX_PREFIX`] (so
    /// `read_aux("signatures/<key_id>.sig.json")` reads the embedded signature JSON). No digest
    /// check: aux members are seal-ignored (ADR-0042) — carriage, not integrity-guaranteed. Anything
    /// that needs tamper-evidence goes in `manifest.blocks` / `extra` (both sealed).
    pub fn read_aux(&mut self, name: &str) -> Result<Vec<u8>> {
        let entry = format!("{AUX_PREFIX}{name}");
        let mut f = open_member(&mut self.archive, &entry, || {
            format!("no aux entry '{entry}'")
        })?;
        let mut b = Vec::with_capacity(f.size() as usize);
        f.read_to_end(&mut b)?;
        Ok(b)
    }

    /// Stream a block's bytes to `w` in **bounded memory** — copy a fixed buffer at a time, hashing as
    /// the bytes flow, never holding the whole block in a `Vec`. The bounded counterpart of
    /// [`Self::read_block`] for a large blob: over a cloud `Read + Seek` source the underlying zip read
    /// issues range-GETs for just this block, so a multi-GB blob extracts (locally or from S3) without
    /// buffering it. Verifies the block digest after the last byte; returns the number of bytes written.
    ///
    /// **Integrity contract:** the digest is checked only *after* the final byte, so on an
    /// `Err(Integrity)` `w` will already have received the (unverified) bytes. A caller writing to a
    /// final destination must stage to a temp path and rename only on `Ok` (as `tessera extract` does)
    /// — never expose `w`'s contents until this returns `Ok`.
    pub fn stream_block(&mut self, name: &str, w: &mut impl Write) -> Result<u64> {
        let expected = self
            .manifest
            .blocks
            .iter()
            .find(|b| b.name == name)
            .and_then(|b| b.digest.clone());
        let entry = format!("{BLOCKS_PREFIX}{name}");
        let mut f = open_member(&mut self.archive, &entry, || {
            format!("no block entry '{entry}'")
        })?;
        let mut hasher = tessera_core::hash::StreamHasher::new();
        let mut buf = [0u8; 64 * 1024];
        let mut total: u64 = 0;
        loop {
            let n = f.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            w.write_all(&buf[..n])?;
            total += n as u64;
        }
        if let Some(exp) = expected {
            let actual = hasher.finalize();
            if actual != exp {
                return Err(Error::Integrity {
                    what: "block_payload",
                    expected: exp,
                    actual,
                });
            }
        }
        Ok(total)
    }

    /// Verify every block's stored payload by streaming it through [`Self::stream_block`] (the
    /// bounded-memory 64 KiB-ring path) into a null sink — so a multi-GB product verifies at a few
    /// MiB RSS, never buffering a whole block. `open` already checked the seal (manifest); this is
    /// the payload half. The first corrupt block returns a typed [`Error::BlockIntegrity`] naming
    /// `label` (the file path or URL) and the block, with the underlying cause in `detail` (the zip
    /// CRC error, or an `Integrity` mismatch with expected/actual) — never a locus-free `io:` error
    /// (#268). Shared by `tessera verify` and the `--verify` deep opt-in of the read-side commands.
    ///
    /// Before any payload is hashed it checks the container itself (SPEC §6, GHSA-m665-v7pm-7grj):
    /// every local header agrees with its central-directory record, no name appears twice, every
    /// entry is one the manifest lists or a safely named `aux/` member (distinct up to letter case,
    /// none below another, every one matching its CRC-32), the central directory lists the entries in
    /// file order, every local header and
    /// central-directory record is byte for byte the one tessera's writer emits (no data descriptor,
    /// no encryption, no extra field but zip64's, a regular file's mode, disk 0), and every byte of
    /// the file belongs to an entry, the central directory or its end records, which are exactly the
    /// writer's, with no comment and nothing after them. A
    /// `.tsra` that passes reads the same whether a reader walks the central directory, walks the
    /// local headers, or finds the end record by scanning back from the end of the file, so the
    /// verdict holds for every zip reader, not only for tessera's. A failure there is
    /// [`Error::Container`] naming `label`, since it belongs to the archive, not to one block.
    pub fn verify_payloads(&mut self, label: &str) -> Result<()> {
        self.check_canonical_layout(label)?;
        self.stream_all_payloads(label)
    }

    /// The container half of [`Self::verify_payloads`]: [`check_one_reading`] over this archive.
    fn check_canonical_layout(&mut self, label: &str) -> Result<()> {
        let mut raw = SharedSource(Arc::clone(&self.source));
        check_one_reading(&mut self.archive, &mut raw, &self.manifest, label)
    }

    /// The payload half of [`Self::verify_payloads`], with the container check already done.
    fn stream_all_payloads(&mut self, label: &str) -> Result<()> {
        let mut sink = std::io::sink();
        for name in self.block_names() {
            self.stream_block(&name, &mut sink)
                .map_err(|e| Error::BlockIntegrity {
                    file: label.to_string(),
                    block: name.clone(),
                    detail: e.to_string(),
                })?;
        }
        Ok(())
    }
}

/// The size floor below which parallel verify isn't worth the thread-spawn + per-worker archive
/// reopen overhead — mirrors the "a few hundred MiB" knee blake3's mmap/rayon path uses
/// ([`crate::blob::blob_ref_streaming_parallel`]). Below this a `.tsra` verifies serially.
const PARALLEL_VERIFY_MIN_BYTES: u64 = 256 * 1024 * 1024;

/// The adaptive knee for [`verify_payloads_parallel`]: fan the L2 payload probe across workers only
/// when it pays back — more than one worker, more than one block, and a file at least `min_bytes`
/// (thread setup + per-worker reopen is then noise against the hashing). Small products stay serial
/// (the same small-stays-single principle as the streaming write path, ADR-0026). `min_bytes` is a
/// parameter so tests can drop it to `0` and exercise the fan-out branch on a small fixture.
fn should_parallelize(file_size: u64, block_count: usize, workers: usize, min_bytes: u64) -> bool {
    workers > 1 && block_count > 1 && file_size >= min_bytes
}

/// The parallel core: verify `names` by streaming each block's stored bytes through
/// [`Reader::stream_block`] into a null sink, fanned across `n_threads` OS threads. Each thread opens
/// its **own** [`Reader`] (a `.tsra` is a STORED zip so every block is independently addressable) and
/// work-steals blocks via a shared cursor, so one huge blob never idles the other workers. Read-only
/// and order-independent — identical result to the serial path. On corruption it returns the typed
/// [`Error::BlockIntegrity`] for *a* bad block (whichever a worker reaches first, not necessarily the
/// first in manifest order). `std::thread` — the same worker-pool idiom as the write engine, no new dep.
fn verify_payloads_multithread(
    path: &Path,
    names: &[String],
    label: &str,
    n_threads: usize,
) -> Result<()> {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    let cursor = AtomicUsize::new(0);
    // Lock-free "someone already failed" flag gates the hot path (checked every iteration); the
    // Mutex is only ever taken on the rare error path, and `swap` ensures exactly the first worker
    // to fail records its error.
    let failed = AtomicBool::new(false);
    let first_err: std::sync::Mutex<Option<Error>> = std::sync::Mutex::new(None);
    let record = |e: Error| {
        if !failed.swap(true, Ordering::AcqRel) {
            *first_err.lock().unwrap() = Some(e);
        }
    };
    std::thread::scope(|s| {
        for _ in 0..n_threads {
            s.spawn(|| {
                let mut r = match Reader::open(path) {
                    Ok(r) => r,
                    Err(e) => {
                        record(e);
                        return;
                    }
                };
                let mut sink = std::io::sink();
                loop {
                    // Stop pulling work once any worker has recorded a failure (lock-free check).
                    if failed.load(Ordering::Acquire) {
                        return;
                    }
                    let i = cursor.fetch_add(1, Ordering::Relaxed);
                    if i >= names.len() {
                        return;
                    }
                    if let Err(e) = r.stream_block(&names[i], &mut sink) {
                        record(Error::BlockIntegrity {
                            file: label.to_string(),
                            block: names[i].clone(),
                            detail: e.to_string(),
                        });
                        return;
                    }
                }
            });
        }
    });
    match first_err.into_inner().unwrap() {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// Threshold-injectable core of [`verify_payloads_parallel`]: the public entry fixes `min_bytes` to
/// [`PARALLEL_VERIFY_MIN_BYTES`]; tests pass `0` to force the fan-out branch on a small fixture.
/// Returns the number of blocks verified, so a caller needn't reopen the archive to report the count.
fn verify_payloads_parallel_with(
    path: &Path,
    label: &str,
    workers: usize,
    min_bytes: u64,
) -> Result<usize> {
    let mut reader = Reader::open(path)?; // L1 seal check
    let names = reader.block_names();
    // The container check runs once, here, before either branch: the workers only stream payloads.
    reader.check_canonical_layout(label)?;
    let file_size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if should_parallelize(file_size, names.len(), workers, min_bytes) {
        verify_payloads_multithread(path, &names, label, workers.min(names.len()))?;
    } else {
        reader.stream_all_payloads(label)?;
    }
    Ok(names.len())
}

/// Parallel peer of [`Reader::verify_payloads`] for a **local** `.tsra` — the L2 payload probe fanned
/// across up to `workers` OS threads. Above the [`should_parallelize`] knee it work-steals blocks
/// across `min(workers, blocks)` threads via [`verify_payloads_multithread`]; below it (small file,
/// single block, or `workers <= 1`) it falls back to the serial [`Reader::verify_payloads`], since the
/// thread-spawn + reopen overhead doesn't pay back. The result — the block count on success, or a
/// typed [`Error::BlockIntegrity`] — is identical to the serial path; only the wall-clock differs.
pub fn verify_payloads_parallel(path: &Path, label: &str, workers: usize) -> Result<usize> {
    verify_payloads_parallel_with(path, label, workers, PARALLEL_VERIFY_MIN_BYTES)
}

/// One aux member to add to an existing `.tsra` — a name **relative to `aux/`** (so
/// `"signatures/<key_id>.sig.json"` becomes `aux/signatures/<key_id>.sig.json` inside the archive)
/// and its bytes. Aux members ride outside the seal (ADR-0042) — the container hashes
/// `content_hash` / `manifest_hash` remain byte-identical when they are added or removed.
pub struct AuxMember {
    /// Path under `aux/` (e.g. `"signatures/<key_id>.sig.json"`, `"provenance.json"`). Must NOT
    /// itself start with `aux/` — the prefix is added by [`add_aux_members`].
    pub name: String,
    /// Verbatim bytes of the member (typically JSON).
    pub bytes: Vec<u8>,
}

impl AuxMember {
    pub fn new(name: impl Into<String>, bytes: Vec<u8>) -> Self {
        AuxMember {
            name: name.into(),
            bytes,
        }
    }
}

/// Add one or more **non-sealed aux members** to an existing sealed `.tsra` **without touching the
/// sealed region** (ADR-0042). Thin wrapper over [`write_aux_members`] with no removal — an existing
/// `aux/<name>` collides only when the new `AuxMember` has the same name (last-write-wins).
pub fn add_aux_members(path: &Path, members: &[AuxMember]) -> Result<()> {
    write_aux_members(path, members, &[])
}

/// Add non-sealed aux members AND remove any existing aux members whose sub-path (name below
/// `aux/`) starts with any prefix in `remove_prefixes` (ADR-0042). Same seal-ignore invariant as
/// [`add_aux_members`]: the reopened manifest's `content_hash` / `manifest_hash` are byte-identical.
///
/// Used by `sign` to cleanly replace ALL prior `aux/signatures/*` entries with a freshly written
/// one — otherwise a re-sign with a different key would leave the previous signature behind, and
/// [`crate::sign::read_signature`] (first-in-central-directory-order) would resolve to the stale
/// entry. Pass `["signatures/"]` to clear the whole signature subtree; the caller adds the new one
/// through `members`.
///
/// The sealed region is copied byte for byte, so this door can only re-emit what [`Reader::open`]
/// accepts: an A′ product, its `manifest.json` already its JCS form under its `manifest.seal`. A
/// product in the alpha layout is refused on open (ADR-0066 rule 10) and never rewritten (#931).
///
/// Each member name must obey the block-name grammar (ADR-0063: `/`-separated `[A-Za-z0-9._-]+`
/// segments, no `.`/`..`), so no `aux/` entry this writes can escape `aux/` when the archive is
/// unzipped or [`unpack`]ed. The `blocks/` and `aux/` entries it copies through from `path` are held
/// to the same grammar, so a hostile input is refused rather than re-emitted. And the aux names the
/// rewritten archive would hold, kept and added together, must be distinct up to ASCII letter case
/// with none below another (`a` and `a/b`), as verification requires (SPEC §6). Any of these
/// failures is [`Error::Container`], before anything is written.
pub fn write_aux_members(
    path: &Path,
    members: &[AuxMember],
    remove_prefixes: &[&str],
) -> Result<()> {
    for m in members {
        check_aux_name(&m.name)?;
    }
    // Read the existing archive once, gather (name → bytes) for every entry. `Reader::open`
    // verifies magic + manifest seal, so a corrupt input fails before we start rewriting.
    let mut existing: Vec<(String, Vec<u8>)> = Vec::new();
    let mut manifest_json_bytes: Option<Vec<u8>> = None;
    let mut seal_bytes: Option<Vec<u8>> = None;
    let sealed_manifest = {
        let mut r = Reader::open(path)?;
        // Central-directory-ordered pass so the rewritten archive preserves the sealed entry order.
        // Each entry is held to its local header, like every member read ([`open_member`]): the
        // rewrite would otherwise re-emit a cloaked input as a clean-looking archive.
        for i in 0..r.archive.len() {
            let opts = ZipReadOptions::new().enforce_header_consistency(true);
            let mut zf = r
                .archive
                .by_index_with_options(i, opts)
                .map_err(|e| Error::Container(format!("read existing entry #{i}: {e}")))?;
            let name = zf.name().map_err(cz)?.into_owned();
            let mut buf = Vec::with_capacity(zf.size() as usize);
            zf.read_to_end(&mut buf)?;
            if name == MANIFEST_ENTRY {
                manifest_json_bytes = Some(buf.clone());
            } else if name == SEAL_ENTRY {
                seal_bytes = Some(buf.clone());
            }
            existing.push((name, buf));
        }
        r.manifest().clone()
    };
    let manifest_json_bytes = manifest_json_bytes
        .ok_or_else(|| Error::Container("existing .tsra has no manifest.json".into()))?;
    let seal_bytes =
        seal_bytes.ok_or_else(|| Error::Container("existing .tsra has no manifest.seal".into()))?;

    // Names of aux members being added — for the "overwrite by name" behavior. Compare with the
    // AUX_PREFIX added, since existing entries carry the full path.
    let to_add: std::collections::BTreeSet<String> = members
        .iter()
        .map(|m| format!("{AUX_PREFIX}{}", m.name))
        .collect();
    // Full prefixes (including `aux/`) of entries to remove — turns a caller-facing "signatures/"
    // into the archive-level "aux/signatures/" so the pass-through filter matches the entry names.
    let remove_full: Vec<String> = remove_prefixes
        .iter()
        .map(|p| format!("{AUX_PREFIX}{p}"))
        .collect();

    // Every entry copied through is held to the grammar, before anything is written. An aux entry
    // this call replaces or removes is not copied, so it is not checked.
    let mut kept_aux = Vec::new();
    for (name, _) in &existing {
        let dropped = name.starts_with(AUX_PREFIX)
            && (to_add.contains(name) || remove_full.iter().any(|p| name.starts_with(p.as_str())));
        if !dropped {
            check_passthrough_entry(name)?;
            if let Some(rest) = name.strip_prefix(AUX_PREFIX) {
                kept_aux.push(rest);
            }
        }
    }
    // The aux set the rewrite would hold, held to the rule verification applies to it.
    check_aux_name_set(
        kept_aux
            .into_iter()
            .chain(members.iter().map(|m| m.name.as_str())),
    )?;

    // Stage the rewrite to a `.part`, atomic-rename on success.
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(".part");
    let tmp_path = std::path::PathBuf::from(tmp);
    let result = write_repacked_archive(&tmp_path, &existing, &to_add, &remove_full, members);
    match result {
        Ok(()) => {
            std::fs::rename(&tmp_path, path)?;
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(e);
        }
    }

    // Seal-ignore invariant: reopen the rewritten archive; the sealed region's serialised
    // manifest.json bytes and its verified `content_hash` / `manifest_hash` MUST be identical. A
    // failure here is a bug (an aux write is NEVER allowed to perturb the seal) — surface it as a
    // typed integrity error instead of silently shipping a mutated seal.
    let reopened = Reader::open(path)?;
    let reopened_manifest = reopened.manifest();
    if reopened_manifest.content_hash != sealed_manifest.content_hash {
        return Err(Error::Integrity {
            what: "aux_write_changed_content_hash",
            expected: sealed_manifest.content_hash.clone().unwrap_or_default(),
            actual: reopened_manifest.content_hash.clone().unwrap_or_default(),
        });
    }
    if reopened_manifest.manifest_hash != sealed_manifest.manifest_hash {
        return Err(Error::Integrity {
            what: "aux_write_changed_manifest_hash",
            expected: sealed_manifest.manifest_hash.clone().unwrap_or_default(),
            actual: reopened_manifest.manifest_hash.clone().unwrap_or_default(),
        });
    }
    // Also assert the on-disk `manifest.json` and `manifest.seal` entries themselves were preserved
    // byte-for-byte — a hash match with different bytes would still count as a seal-ignore break.
    let mut reopened_zip = open_tsra_archive(File::open(path)?)?;
    for (entry, want) in [
        (MANIFEST_ENTRY, &manifest_json_bytes),
        (SEAL_ENTRY, &seal_bytes),
    ] {
        let mut got = Vec::new();
        open_member(&mut reopened_zip, entry, || {
            format!("aux write lost {entry}")
        })?
        .read_to_end(&mut got)?;
        if &got != want {
            return Err(Error::Container(format!(
                "aux write mutated the {entry} bytes (seal-ignore invariant broken)"
            )));
        }
    }
    Ok(())
}

/// Write the repacked archive to `out`: sealed entries first (byte-identical), then the surviving
/// old aux entries (any name being overwritten OR matching a `remove_full` prefix is dropped here),
/// then the newly added aux members.
fn write_repacked_archive(
    out: &Path,
    existing: &[(String, Vec<u8>)],
    to_add: &std::collections::BTreeSet<String>,
    remove_full: &[String],
    members: &[AuxMember],
) -> Result<()> {
    let mut zw = ZipWriter::new(File::create(out)?);
    let stored = tsra_entry_options();
    // Pass 1: copy through every SEALED entry (mimetype, manifest.seal, manifest.json, blocks/*),
    // preserving order.
    for (name, bytes) in existing {
        if name == MIMETYPE_ENTRY
            || name == SEAL_ENTRY
            || name == MANIFEST_ENTRY
            || name.starts_with(BLOCKS_PREFIX)
        {
            zw.start_file(name, stored).map_err(cz)?;
            zw.write_all(bytes)?;
        }
    }
    // Pass 2: copy through every existing aux entry NOT being replaced by name AND NOT covered by
    // a `remove_full` prefix (the mechanism `sign` uses to clear the prior `aux/signatures/*`).
    for (name, bytes) in existing {
        if !name.starts_with(AUX_PREFIX) {
            continue;
        }
        if to_add.contains(name) {
            continue;
        }
        if remove_full.iter().any(|p| name.starts_with(p)) {
            continue;
        }
        zw.start_file(name, stored).map_err(cz)?;
        zw.write_all(bytes)?;
    }
    // Pass 3: append the newly added aux members (in caller order).
    for m in members {
        zw.start_file(format!("{AUX_PREFIX}{}", m.name), stored)
            .map_err(cz)?;
        zw.write_all(&m.bytes)?;
    }
    zw.finish().map_err(cz)?;
    Ok(())
}

/// Read a `.tsra`'s manifest **without verifying its seal** — for diagnostics only.
///
/// [`Reader::open`] refuses a product whose manifest does not verify, which is right for every read that
/// will act on the contents. But refusing it also throws away the one thing needed to *report* the
/// problem usefully: which product the broken file claims to be. Without that, a tool resolving
/// provenance can only say "something here is corrupt", not "the parent this edge needs is corrupt".
///
/// The returned manifest is **untrusted**. Its `id` and metadata are claims by a file that has already
/// failed verification, so use them to name and locate, never to decide. Its `manifest_hash` is `None`:
/// the seal lives in `manifest.seal` (ADR-0066), which this does not read. Anything that
/// reads block payloads or acts on metadata must go through [`Reader::open`] and get the refusal.
///
/// It still parses strictly ([`Manifest::from_json`]): a manifest that repeats a key is not one
/// manifest, so there is no single claim to name, and it fails with the parse error.
pub fn read_manifest_unverified(path: &Path) -> Result<Manifest> {
    Manifest::from_json(&manifest_text(path)?)
}

/// The lineage `id` a `.tsra`'s manifest **claims**, read without verifying anything else — for
/// naming a product that failed [`Reader::open`] (a provenance walk pointing at a corrupt parent).
///
/// Like [`read_manifest_unverified`] the claim is **untrusted**, but it needs only the top-level
/// `id` string, so it is also read from a manifest the format refuses for its numbers: one an older
/// writer sealed with an integer beyond ±(2^53−1) (SPEC §1, #886). That manifest has passed the
/// duplicate-key check ([`tessera_core::canonical::parse_strict`] refuses its numbers only after),
/// so it still holds exactly one `id`. Text that repeats a key or isn't JSON has no single claim and
/// fails with the parse error.
///
/// # Errors
/// The container and parse errors of [`read_manifest_unverified`], or [`Error::Container`] when the
/// manifest holds no string `id`.
pub fn read_claimed_id(path: &Path) -> Result<String> {
    let mj = manifest_text(path)?;
    let v = match tessera_core::canonical::parse_strict(&mj) {
        Err(Error::InexactInteger { .. }) => serde_json::from_str(&mj)?,
        other => other?,
    };
    v.get("id")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| Error::Container("manifest.json claims no string `id`".into()))
}

/// The text of a `.tsra`'s `manifest.json`, unverified. Held to its local header like every member
/// read: an untrusted manifest is still read the one way every zip reader would read it, or not at all.
fn manifest_text(path: &Path) -> Result<String> {
    let mut archive = open_tsra_archive(File::open(path)?)?;
    let mut mj = String::new();
    open_member(&mut archive, MANIFEST_ENTRY, || {
        "missing manifest.json".into()
    })?
    .read_to_string(&mut mj)?;
    Ok(mj)
}

/// Explode a `.tsra` into a directory: `manifest.json` + `manifest.seal` + `blocks/<name>` +
/// `aux/<name>` (the opt-in exploded form of ADR-0022; ADR-0042 adds the aux round-trip, ADR-0066
/// the seal file). Verifies the seal on open and each block against its digest. `manifest.json`
/// and `manifest.seal` are the stored members, copied byte for byte ([`crate::stored::write_pair`]).
/// Aux members are copied verbatim under `aux/` (they carry no digest — seal-ignored). Returns the
/// verified manifest.
///
/// Every name becomes a path under `outdir` by plain components only (ADR-0063, #685): a block
/// name through [`BlockName::path_under`], an aux member name through the same grammar
/// ([`join_checked`]; an aux name that fails it is [`Error::Container`]). A hierarchical name
/// (`volume/0`) becomes a subdirectory. A product whose names break the **leaf rule** (one name the
/// parent of another — a pyramid sealed before ADR-0063) is refused with
/// [`Error::BlockNameConflict`] before anything is written, since a name can't be both a file and a
/// directory; it still reads in-zip through [`Reader`]. So is a product with an invalid partitioned
/// table (ADR-0064 §2.4, [`check_partitions`], the seal's check): [`Error::LogicalTable`], before
/// anything is written, while each of its blocks still reads in-zip by its full name.
pub fn unpack(path: &Path, outdir: &Path) -> Result<Manifest> {
    let mut r = Reader::open(path)?;
    let manifest = r.manifest().clone();
    check_leaf_rule(manifest.blocks.iter().map(|b| &b.name))?;
    check_partitions(&manifest.blocks)?;
    let blocks_dir = outdir.join("blocks");
    let aux_dir = outdir.join("aux");
    // Resolve every path before writing anything, so a refused name leaves no partial tree.
    let block_paths = manifest
        .blocks
        .iter()
        .map(|b| Ok((b.name.clone(), b.name.path_under(&blocks_dir)?)))
        .collect::<Result<Vec<_>>>()?;
    let aux_paths = r
        .aux_names()
        .into_iter()
        .map(|name| {
            let p =
                join_checked(&aux_dir, &name).map_err(|reason| aux_name_error(&name, reason))?;
            Ok((name, p))
        })
        .collect::<Result<Vec<_>>>()?;
    std::fs::create_dir_all(&blocks_dir)?;
    // The stored bytes, not the view's JSON: the view may not reproduce the seal (it drops a newer
    // writer's fields), while the stored bytes are what the seal is over.
    let seal = manifest
        .manifest_hash
        .as_deref()
        .ok_or_else(|| Error::Container("a verified manifest carries its seal".into()))?;
    crate::stored::write_pair(
        &outdir.join(MANIFEST_ENTRY),
        r.manifest_json().as_bytes(),
        seal,
    )?;
    for (name, out_path) in block_paths {
        let bytes = r.read_block(&name)?;
        write_creating_parents(&out_path, &bytes)?;
    }
    // ADR-0042: also explode any aux members so `unpack` → `pack` round-trips the whole file
    // (including the embedded signature + ingest provenance). Aux paths may contain sub-dirs
    // (`signatures/<key_id>.sig.json`).
    for (name, out_path) in aux_paths {
        let bytes = r.read_aux(&name)?;
        write_creating_parents(&out_path, &bytes)?;
    }
    Ok(manifest)
}

/// An entry [`write_aux_members`] copies through from an existing archive: a `blocks/` or `aux/`
/// entry's name below its prefix obeys the block-name grammar (ADR-0063), or [`Error::Container`].
/// (Entries outside `mimetype`, `manifest.seal`, `manifest.json`, `blocks/` and `aux/` are not
/// copied, see
/// [`write_repacked_archive`].)
fn check_passthrough_entry(name: &str) -> Result<()> {
    let rest = name
        .strip_prefix(BLOCKS_PREFIX)
        .or_else(|| name.strip_prefix(AUX_PREFIX));
    match rest {
        Some(rest) => tessera_core::block::name::check_name(rest).map_err(|reason| {
            Error::Container(format!(
                "entry '{name}' is not a safe relative path (ADR-0063): {reason}"
            ))
        }),
        None => Ok(()),
    }
}

/// An aux member name obeys the block-name grammar (ADR-0063), or [`Error::Container`].
fn check_aux_name(name: &str) -> Result<()> {
    tessera_core::block::name::check_name(name).map_err(|reason| aux_name_error(name, reason))
}

fn aux_name_error(name: &str, reason: tessera_core::block::name::NameError) -> Error {
    Error::Container(format!(
        "aux member '{name}' is not a safe relative path: {reason}"
    ))
}

/// Write `bytes` to `path`, creating its parent directories (a hierarchical name's subtree).
fn write_creating_parents(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, bytes)?;
    Ok(())
}

/// Pack an exploded directory (`manifest.json` + `manifest.seal` + `blocks/<name>` + optional
/// `aux/<name>`) back into a sealed `.tsra`. The pair is read through
/// [`Manifest::from_stored_members`] (the canonical check, the seal, `id`, `content_hash`; a
/// directory without `manifest.seal` is the alpha layout, refused naming both formats) and packed
/// byte for byte; each block's payload is read from `blocks/`. If an `aux/` subtree is present it is appended as
/// non-sealed aux members (ADR-0042) — round-trip counterpart of [`unpack`], so a `.tsra` that
/// carried an embedded signature + ingest provenance survives an unpack/repack round with
/// byte-identical seal AND the aux payload intact.
///
/// Each block is read from [`BlockName::path_under`] `dir/blocks` (plain components only, ADR-0063),
/// and a manifest whose names break the leaf rule is refused with [`Error::BlockNameConflict`]: an
/// exploded directory cannot hold it. An `aux/` file whose relative path fails the grammar (a space,
/// a non-ASCII character), or two `aux/` files whose paths differ only in letter case (possible on a
/// case-sensitive file system), is [`Error::Container`]. A manifest with an invalid partitioned table is
/// refused with [`Error::LogicalTable`] (ADR-0064 §2.4, [`check_partitions`], the seal's check).
/// Every check runs before `out` is written.
pub fn pack_dir(dir: &Path, out: &Path) -> Result<()> {
    let (mj, seal_member) = crate::stored::read_pair(&dir.join(MANIFEST_ENTRY))?;
    let manifest = Manifest::from_stored_members(&mj, seal_member.as_deref())?;
    check_leaf_rule(manifest.blocks.iter().map(|b| &b.name))?;
    check_partitions(&manifest.blocks)?;
    // Collect and check the aux/ subtree first, so a member name the grammar refuses fails before
    // `out` is written (not after, leaving a sealed archive without its aux members).
    let aux_dir = dir.join("aux");
    let members = if aux_dir.is_dir() {
        collect_aux_members(&aux_dir)?
    } else {
        Vec::new()
    };
    for m in &members {
        check_aux_name(&m.name)?;
    }
    check_aux_name_set(members.iter().map(|m| m.name.as_str()))?;
    let blocks_dir = dir.join("blocks");
    let mut payloads = Vec::with_capacity(manifest.blocks.len());
    for b in &manifest.blocks {
        let bytes = std::fs::read(b.name.path_under(&blocks_dir)?)?;
        payloads.push(BlockPayload::new(b.name.clone(), bytes));
    }
    check_packable(&manifest)?;
    // The verified stored bytes, not the view's JSON: they are what the seal covers, so a product
    // from another writer, or with newer fields, packs back under the same seal.
    let seal = manifest
        .manifest_hash
        .as_deref()
        .ok_or_else(|| Error::Container("a verified manifest carries its seal".into()))?;
    pack_manifest_json(&mj, seal, &payloads, out)?;
    // Re-attach the aux/ subtree, each file under its relative path.
    if !members.is_empty() {
        add_aux_members(out, &members)?;
    }
    Ok(())
}

/// Walk `aux_dir` and collect every regular file as an [`AuxMember`] whose `name` is its path
/// relative to `aux_dir` (forward-slashed). Returns members in sorted order so a `pack_dir` on the
/// same tree is deterministic. Used only by [`pack_dir`].
fn collect_aux_members(aux_dir: &Path) -> Result<Vec<AuxMember>> {
    let mut out = Vec::new();
    fn walk(dir: &Path, base: &Path, out: &mut Vec<AuxMember>) -> Result<()> {
        let mut entries: Vec<_> = std::fs::read_dir(dir)?.collect::<std::io::Result<_>>()?;
        entries.sort_by_key(std::fs::DirEntry::path);
        for e in entries {
            let p = e.path();
            let ft = e.file_type()?;
            if ft.is_dir() {
                walk(&p, base, out)?;
            } else if ft.is_file() {
                let rel = p
                    .strip_prefix(base)
                    .map_err(|e| Error::Container(format!("aux dir walk: {e}")))?;
                let name = rel
                    .to_str()
                    .ok_or_else(|| Error::Container("aux member name is not valid UTF-8".into()))?
                    .replace('\\', "/");
                let bytes = std::fs::read(&p)?;
                out.push(AuxMember::new(name, bytes));
            }
        }
        Ok(())
    }
    walk(aux_dir, aux_dir, &mut out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bn(s: &str) -> BlockName {
        s.parse().unwrap()
    }
    use tessera_core::block::array::{ArrayBlock, ArraySpec};
    use tessera_core::ProductBuilder;

    /// [`check_central_directory`] over an archive `zip` writes with [`tsra_entry_options`].
    fn central_verdict<R: Read + Seek>(archive: &ZipArchive<R>, raw: &mut R) -> String {
        let local = check_local_headers(archive, raw);
        match local.and_then(|()| check_central_directory(archive, raw)) {
            Ok(()) => "ok".into(),
            Err(Error::Container(why)) => why,
            Err(e) => panic!("not a container verdict: {e}"),
        }
    }

    /// The zip64 end records of [`canonical_end_records`], forced by the entry count. No committed
    /// `.tsra` has them (`zip` writes them only past 65,535 entries or 4 GiB), so this builds the
    /// smallest archive that gets them: 65,536 empty entries, written with the `.tsra` options. The
    /// honest footer passes; a 32-bit field that is not the sentinel the writer emits, a zip64 field
    /// the writer pins, and a byte after the end record are each refused although `zip` still opens
    /// the archive. (`zip` itself refuses a locator naming more than one disk.)
    #[test]
    fn zip64_end_records_must_be_the_ones_the_writer_emits() {
        let mut w = ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for i in 0..=usize::from(u16::MAX) {
            w.start_file(format!("{i:x}"), tsra_entry_options())
                .unwrap();
        }
        let honest = w.finish().unwrap().into_inner();
        let eocd = honest.len() - EOCD_LEN;
        let locator = eocd - 20; // the zip64 locator's fixed size
        let zip64_record = locator - 56;
        assert_eq!(
            honest[locator..locator + 4],
            ZIP64_LOCATOR_SIG,
            "zip wrote zip64 records"
        );

        let verdict = |bytes: &[u8]| -> Option<String> {
            let archive = ZipArchive::new(std::io::Cursor::new(bytes)).ok()?;
            Some(central_verdict(&archive, &mut std::io::Cursor::new(bytes)))
        };
        assert_eq!(verdict(&honest).as_deref(), Some("ok"));

        let tampered = |at: usize, value: &[u8]| {
            let mut b = honest.clone();
            b[at..at + value.len()].copy_from_slice(value);
            b
        };
        let mut appended = honest.clone();
        appended.push(0);
        for (what, bytes, needle) in [
            (
                "a 32-bit directory size that is not the sentinel",
                tampered(eocd + 12, &7u32.to_le_bytes()),
                "end record directory size",
            ),
            (
                "a 32-bit entry count that is not the sentinel",
                tampered(eocd + 10, &7u16.to_le_bytes()),
                "end record entry count",
            ),
            (
                "a zip64 record's version made by that the writer does not emit",
                tampered(zip64_record + 12, &0x033fu16.to_le_bytes()),
                "zip64 end record version made by",
            ),
            (
                "a byte after the end record",
                appended,
                "follow the central directory",
            ),
        ] {
            let v = verdict(&bytes).unwrap_or_else(|| panic!("{what}: zip no longer opens it"));
            assert!(v.contains(needle), "{what}: {v}");
        }
    }

    /// A `Write + Seek + Read` byte store that keeps every write shorter than [`Self::KEPT`] and reads
    /// everything else back as zeros: enough to let `zip` write an archive past 4 GiB without 4 GiB of
    /// memory, when the only long writes are runs of zeros. `zip` writes each header and record with
    /// short writes of its own, and a test that wrote a long non-zero buffer would read zeros back and
    /// fail its own check, not pass it.
    #[derive(Clone, Default)]
    struct Sparse {
        writes: Vec<(u64, Vec<u8>)>,
        len: u64,
        pos: u64,
    }

    impl Sparse {
        const KEPT: usize = 4096;
    }

    impl Write for Sparse {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if buf.len() < Self::KEPT {
                self.writes.push((self.pos, buf.to_vec()));
            }
            self.pos += buf.len() as u64;
            self.len = self.len.max(self.pos);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Read for Sparse {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = usize::try_from(self.len.saturating_sub(self.pos))
                .unwrap_or(usize::MAX)
                .min(buf.len());
            let (start, end) = (self.pos, self.pos + n as u64);
            buf[..n].fill(0);
            for (at, bytes) in &self.writes {
                let (w_start, w_end) = (*at, *at + bytes.len() as u64);
                let (lo, hi) = (start.max(w_start), end.min(w_end));
                if lo < hi {
                    buf[(lo - start) as usize..(hi - start) as usize]
                        .copy_from_slice(&bytes[(lo - w_start) as usize..(hi - w_start) as usize]);
                }
            }
            self.pos = end;
            Ok(n)
        }
    }

    impl Seek for Sparse {
        fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
            self.pos = match pos {
                SeekFrom::Start(p) => p,
                SeekFrom::End(d) => self.len.checked_add_signed(d).unwrap(),
                SeekFrom::Current(d) => self.pos.checked_add_signed(d).unwrap(),
            };
            Ok(self.pos)
        }
    }

    /// The branches past 4 GiB, on bytes `zip` itself writes: a member whose local header starts past
    /// 32 bits carries its offset in the central record's zip64 field (the 32-bit field holds the
    /// sentinel), and a directory that starts past 32 bits brings the zip64 end records although there
    /// are only three entries. A large `.tsra` (a raw vendor listmode file) is exactly this shape.
    #[test]
    fn a_tsra_past_4_gib_is_canonical() {
        let mut w = ZipWriter::new(Sparse::default());
        let zeros = vec![0u8; 1 << 20];
        w.start_file(MIMETYPE_ENTRY, tsra_entry_options()).unwrap();
        w.write_all(MIMETYPE.as_bytes()).unwrap();
        w.start_file("blocks/big", tsra_entry_options()).unwrap();
        for _ in 0..(4u64 << 10) + 1 {
            w.write_all(&zeros).unwrap();
        }
        w.start_file("blocks/after", tsra_entry_options()).unwrap();
        w.write_all(b"past four gibibytes").unwrap();
        let mut sink = w.finish().unwrap();
        sink.seek(SeekFrom::Start(0)).unwrap();
        let mut raw = sink.clone();
        let archive = ZipArchive::new(sink).unwrap();
        assert!(archive.central_directory_start() > u64::from(u32::MAX));
        let after = archive.index_for_name("blocks/after").unwrap();
        assert!(archive.by_index_data(after).unwrap().header_start() > u64::from(u32::MAX));
        assert_eq!(central_verdict(&archive, &mut raw), "ok");
    }

    /// Build a recon product, pack it, then open + verify + read the block back out.
    #[test]
    fn pack_open_roundtrip_and_block_read() {
        let vol = ArrayBlock::new("volume", ArraySpec::new(vec![64, 64, 64], "int16"));
        // the payload bytes are exactly what the block digest is computed over (the spec, in the
        // spike — real backends store encoded zarr/vortex bytes with the same property).
        let payload = serde_json::to_vec(&vol.spec).unwrap();

        let mut b = ProductBuilder::new("recon", "DP06", "d", "2024-01-01T00:00:00Z");
        b.add_block(&vol).unwrap();
        b.with_field(
            "modality",
            serde_json::json!({"_vocabulary": "DICOM", "_code": "CT"}),
        );
        let sealed = b.seal().unwrap();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("DP06.tsra");
        pack(
            &sealed,
            &[BlockPayload::new(bn("volume"), payload.clone())],
            &path,
        )
        .unwrap();

        let mut r = Reader::open(&path).unwrap();
        assert_eq!(r.manifest().id, sealed.id);
        assert_eq!(r.manifest().manifest_hash, sealed.manifest_hash);
        assert_eq!(r.block_names(), vec!["volume".to_string()]);
        assert_eq!(r.read_block("volume").unwrap(), payload);
    }

    #[test]
    fn tampered_block_payload_fails_on_read() {
        let vol = ArrayBlock::new("volume", ArraySpec::new(vec![8, 8, 8], "int16"));
        let mut b = ProductBuilder::new("recon", "x", "d", "2024-01-01T00:00:00Z");
        b.add_block(&vol).unwrap();
        b.with_field("modality", serde_json::json!("CT"));
        let sealed = b.seal().unwrap();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.tsra");
        // store WRONG bytes for the block → digest mismatch on read
        pack(
            &sealed,
            &[BlockPayload::new(bn("volume"), b"not the spec".to_vec())],
            &path,
        )
        .unwrap();

        let mut r = Reader::open(&path).unwrap();
        match r.read_block("volume") {
            Err(Error::Integrity { what, .. }) => assert_eq!(what, "block_payload"),
            other => panic!("expected block_payload integrity error, got {other:?}"),
        }
    }

    // Build a sealed multi-block product at `path`; return each block's (name, payload bytes).
    fn sealed_multiblock(path: &Path, n: usize) -> Vec<(String, Vec<u8>)> {
        let mut b = ProductBuilder::new("recon", "DP06", "d", "2024-01-01T00:00:00Z");
        let blocks: Vec<ArrayBlock> = (0..n)
            .map(|i| ArrayBlock::new(format!("vol{i}"), ArraySpec::new(vec![8, 8, 8], "int16")))
            .collect();
        let mut out = Vec::new();
        for (i, blk) in blocks.iter().enumerate() {
            b.add_block(blk).unwrap();
            out.push((format!("vol{i}"), serde_json::to_vec(&blk.spec).unwrap()));
        }
        let sealed = b.seal().unwrap();
        let payloads: Vec<BlockPayload> = out
            .iter()
            .map(|(name, bytes)| BlockPayload::new(bn(name), bytes.clone()))
            .collect();
        pack(&sealed, &payloads, path).unwrap();
        out
    }

    #[test]
    fn should_parallelize_respects_the_knee() {
        let floor = PARALLEL_VERIFY_MIN_BYTES;
        assert!(
            should_parallelize(floor, 4, 8, floor),
            "at the floor + multi-block + workers → parallel"
        );
        assert!(
            !should_parallelize(floor - 1, 4, 8, floor),
            "one byte below the floor → serial"
        );
        assert!(
            !should_parallelize(floor, 1, 8, floor),
            "single block → serial"
        );
        assert!(
            !should_parallelize(floor, 4, 1, floor),
            "single worker → serial"
        );
    }

    #[test]
    fn parallel_core_verifies_clean_multiblock() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("multi.tsra");
        let names: Vec<String> = sealed_multiblock(&path, 5)
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        verify_payloads_multithread(&path, &names, &path.display().to_string(), 4).unwrap();
    }

    #[test]
    fn parallel_core_catches_a_tampered_block() {
        // One block's payload is flipped while its recorded digest stays — a worker re-deriving the
        // digest from the stored bytes must catch it, even though other workers verify clean blocks.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("multi.tsra");
        let mut b = ProductBuilder::new("recon", "DP06", "d", "2024-01-01T00:00:00Z");
        let mut names = Vec::new();
        let mut payloads = Vec::new();
        for i in 0..5 {
            let blk = ArrayBlock::new(format!("vol{i}"), ArraySpec::new(vec![8, 8, 8], "int16"));
            b.add_block(&blk).unwrap();
            let mut bytes = serde_json::to_vec(&blk.spec).unwrap();
            if i == 3 {
                bytes[0] ^= 0xFF; // corrupt vol3's payload
            }
            names.push(format!("vol{i}"));
            payloads.push(BlockPayload::new(bn(&format!("vol{i}")), bytes));
        }
        let sealed = b.seal().unwrap();
        pack(&sealed, &payloads, &path).unwrap();
        match verify_payloads_multithread(&path, &names, &path.display().to_string(), 4) {
            Err(Error::BlockIntegrity { block, .. }) => assert_eq!(block, "vol3"),
            other => panic!("expected a BlockIntegrity error for vol3, got {other:?}"),
        }
    }

    #[test]
    fn parallel_and_serial_agree() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("multi.tsra");
        let names: Vec<String> = sealed_multiblock(&path, 5)
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        let label = path.display().to_string();
        // serial L2, the parallel core, and the public entry (which for this small file takes the
        // serial fallback) must all agree that a clean product verifies.
        Reader::open(&path)
            .unwrap()
            .verify_payloads(&label)
            .unwrap();
        verify_payloads_multithread(&path, &names, &label, 4).unwrap();
        assert_eq!(verify_payloads_parallel(&path, &label, 8).unwrap(), 5);
    }

    #[test]
    fn forced_parallel_dispatch_runs_end_to_end() {
        // Exercise the PUBLIC entry's fan-out branch on a small fixture by dropping the size floor to
        // 0 — so should_parallelize → true and verify_payloads_parallel_with dispatches to the
        // multithread core (the branch the 256 MiB threshold hides from the other tests). Clean
        // product → Ok(block count).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("multi.tsra");
        sealed_multiblock(&path, 5);
        let label = path.display().to_string();
        assert_eq!(
            verify_payloads_parallel_with(&path, &label, 4, 0).unwrap(),
            5
        );
    }

    #[test]
    fn forced_parallel_dispatch_catches_tamper() {
        // Same forced fan-out, but one block is corrupt: the dispatch → multithread → typed error
        // path must surface the bad block (not silently pass).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("multi.tsra");
        let mut b = ProductBuilder::new("recon", "DP06", "d", "2024-01-01T00:00:00Z");
        let mut payloads = Vec::new();
        for i in 0..5 {
            let blk = ArrayBlock::new(format!("vol{i}"), ArraySpec::new(vec![8, 8, 8], "int16"));
            b.add_block(&blk).unwrap();
            let mut bytes = serde_json::to_vec(&blk.spec).unwrap();
            if i == 2 {
                bytes[0] ^= 0xFF;
            }
            payloads.push(BlockPayload::new(bn(&format!("vol{i}")), bytes));
        }
        let sealed = b.seal().unwrap();
        pack(&sealed, &payloads, &path).unwrap();
        match verify_payloads_parallel_with(&path, &path.display().to_string(), 4, 0) {
            Err(Error::BlockIntegrity { block, .. }) => assert_eq!(block, "vol2"),
            other => panic!("expected a BlockIntegrity error for vol2, got {other:?}"),
        }
    }

    #[test]
    fn tampered_table_block_payload_fails_on_read() {
        // The Vortex-table twin of tampered_block_payload_fails_on_read (which uses an int16 array):
        // flipping a single value's byte in an encoded table payload trips the block_payload digest
        // on read, exactly as for an array/zarr block — the guarantee is kind-agnostic.
        use tessera_core::block::table::{Column, TableBlock, TableSpec};
        let spec = TableSpec {
            columns: vec![Column {
                name: "energy".into(),
                dtype: "f4".into(),
                ..Default::default()
            }],
            rows: 2_696_935,
            row_index: Some("ms".into()),
            partition: None,
            edition: None,
            codec_profile: None,
        };
        // The valid payload is exactly what the block digest is computed over (the spec, in the
        // spike — a real Vortex backend stores encoded column chunks with the same property).
        let good = serde_json::to_vec(&spec).unwrap();
        let events = TableBlock::new("events_3p", spec);

        let mut b = ProductBuilder::new("recon", "DP06", "d", "2024-01-01T00:00:00Z");
        b.add_block(&events).unwrap();
        let sealed = b.seal().unwrap();

        let dir = tempfile::tempdir().unwrap();
        // Happy path: the correct payload reads back.
        let good_path = dir.path().join("good.tsra");
        pack(
            &sealed,
            &[BlockPayload::new(bn("events_3p"), good.clone())],
            &good_path,
        )
        .unwrap();
        assert_eq!(
            Reader::open(&good_path)
                .unwrap()
                .read_block("events_3p")
                .unwrap(),
            good
        );

        // Flip ONE byte (a single value in the table) → block_payload integrity error on read.
        let mut bad = good.clone();
        bad[0] ^= 0xFF;
        let bad_path = dir.path().join("bad.tsra");
        pack(
            &sealed,
            &[BlockPayload::new(bn("events_3p"), bad)],
            &bad_path,
        )
        .unwrap();
        match Reader::open(&bad_path).unwrap().read_block("events_3p") {
            Err(Error::Integrity { what, .. }) => assert_eq!(what, "block_payload"),
            other => panic!("expected block_payload integrity error, got {other:?}"),
        }
    }

    #[test]
    fn manifest_verify_is_l1_only_payload_flip_needs_read_path() {
        // The verify escalation ladder, as an executable spec:
        //   L1 = manifest.verify(): recomputes id / content_hash / manifest_hash from the RECORDED
        //        block digests. It never reads block payloads, so a flipped payload left alongside
        //        an untouched recorded digest still PASSES L1.
        //   L2 = read_block: re-derives the digest from the ACTUAL stored bytes → catches the flip.
        // (L3 = per-chunk sub-block Merkle, #214 / ADR-0028 §3, would localise WHICH chunk — n/a here.)
        use tessera_core::block::table::{Column, TableBlock, TableSpec};
        let spec = TableSpec {
            columns: vec![Column {
                name: "energy".into(),
                dtype: "f4".into(),
                ..Default::default()
            }],
            rows: 10,
            row_index: Some("ms".into()),
            partition: None,
            edition: None,
            codec_profile: None,
        };
        let good = serde_json::to_vec(&spec).unwrap();
        let events = TableBlock::new("events_3p", spec);

        let mut b = ProductBuilder::new("recon", "DP06", "d", "2024-01-01T00:00:00Z");
        b.add_block(&events).unwrap();
        let sealed = b.seal().unwrap();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("flipped.tsra");
        // Pack a FLIPPED payload but leave the manifest — and its recorded digest — untouched.
        let mut flipped = good.clone();
        flipped[0] ^= 0xFF;
        pack(
            &sealed,
            &[BlockPayload::new(bn("events_3p"), flipped)],
            &path,
        )
        .unwrap();

        let mut r = Reader::open(&path).unwrap();
        // L1: the manifest itself still verifies — recorded digests, roots and seal are internally
        // consistent because none of them were touched. L1 does not read payloads.
        r.manifest()
            .verify()
            .expect("L1 (manifest self-consistency) passes: payload bytes are never read");
        // L2: reading the block re-derives the digest from the stored bytes and catches the flip.
        match r.read_block("events_3p") {
            Err(Error::Integrity { what, .. }) => assert_eq!(what, "block_payload"),
            other => panic!("expected a block_payload integrity error at L2, got {other:?}"),
        }
    }

    #[test]
    fn refuses_unsealed_manifest() {
        let m = Manifest::new("recon", "x", "d", "2024-01-01T00:00:00Z"); // not sealed
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("u.tsra");
        assert!(pack(&m, &[], &path).is_err());
        // pack_streaming refuses the same unsealed input — the SSoT options shouldn't bypass the gate.
        assert!(pack_streaming(&m, &[], &path).is_err());
    }

    /// The zip64 half of [`open_tsra_archive`]'s no-scan guarantee. Every product the other tests build
    /// is small enough that `zip` writes a plain zip32 end record (`large_file` only adds per-entry zip64
    /// fields), so they never reach the zip64-EOCD branch, which `ArchiveOffset::Detect` also resolves
    /// by scanning. More than 65 535 entries forces a zip64 end record. A large leading entry makes the
    /// file mostly body, so reading it would show up in the count.
    #[test]
    fn opening_a_zip64_archive_reads_only_its_tail() {
        use crate::range::CountingReader;
        use std::sync::atomic::Ordering;

        let mut zw = ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let stored = tsra_entry_options();
        zw.start_file(MIMETYPE_ENTRY, stored).unwrap();
        zw.write_all(MIMETYPE.as_bytes()).unwrap();
        zw.start_file("body", stored).unwrap();
        zw.write_all(&vec![0u8; 32 << 20]).unwrap();
        for i in 0..70_000u32 {
            zw.start_file(format!("e{i}"), stored).unwrap();
        }
        let bytes = zw.finish().unwrap().into_inner();
        let total = bytes.len() as u64;
        let tail = &bytes[bytes.len() - 22 - 20 - 56..];
        assert!(
            tail.windows(4).any(|w| w == 0x0606_4b50u32.to_le_bytes()),
            "fixture must carry a zip64 end-of-central-directory record"
        );

        let counter = CountingReader::new(std::io::Cursor::new(bytes));
        let tally = counter.counter();
        let archive = open_tsra_archive(counter).unwrap();
        assert_eq!(archive.len(), 70_002);
        let read = tally.load(Ordering::Relaxed);
        assert!(
            read < total / 2,
            "opening read {read} of {total} bytes: the archive was scanned, not range-read"
        );
    }

    /// Re-emit `src`'s entries verbatim (`raw_copy_file`: same bytes, same method, same CRC) in
    /// central-directory `order`, after writing `prefix`. `ZipWriter` records every offset from the start
    /// of what it writes into, so the result is a well-formed zip whose offsets **include** the prefix —
    /// the `zip -A` / shell-polyglot shape, as opposed to bytes glued in front of an unchanged archive.
    fn repack_raw(src: &[u8], prefix: &[u8], order: &[usize]) -> Vec<u8> {
        let mut archive = ZipArchive::new(std::io::Cursor::new(src)).unwrap();
        let mut out = std::io::Cursor::new(prefix.to_vec());
        out.set_position(prefix.len() as u64);
        let mut zw = ZipWriter::new(out);
        for &i in order {
            zw.raw_copy_file(archive.by_index_raw(i).unwrap()).unwrap();
        }
        zw.finish().unwrap().into_inner()
    }

    /// A sealed three-block product's bytes, plus the identity order of its entries.
    fn sealed_multiblock_bytes() -> (Vec<u8>, Vec<usize>) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("multi.tsra");
        sealed_multiblock(&path, 3);
        let bytes = std::fs::read(&path).unwrap();
        let n = ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap().len();
        (bytes, (0..n).collect())
    }

    /// Assert `bytes` is refused as a `.tsra` with a container error whose message contains `needle`.
    fn assert_refused(bytes: Vec<u8>, needle: &str) {
        match Reader::from_reader(std::io::Cursor::new(bytes)) {
            Err(Error::Container(msg)) => {
                assert!(msg.contains(needle), "unexpected message: {msg}")
            }
            Err(other) => panic!("expected a container error, got {other:?}"),
            Ok(_) => panic!("must not open as a .tsra"),
        }
    }

    /// SPEC §6: a `.tsra` begins at byte 0. Bytes glued in front of an unchanged archive leave its
    /// central directory counting from the zip's own start, so the pinned offset 0 finds no central
    /// directory where the end record says it is, and `zip` itself refuses to open it. Detecting the
    /// offset instead would mean scanning the file, which `open` must not do (#588).
    #[test]
    fn an_archive_behind_an_unadjusted_prefix_is_refused() {
        let (bytes, _) = sealed_multiblock_bytes();
        // Control: the same bytes at offset 0 open, so the refusal below is the prefix's doing.
        Reader::from_reader(std::io::Cursor::new(bytes.clone())).unwrap();

        let mut prefixed = b"#!/bin/sh\nexit 0\n".to_vec();
        prefixed.extend_from_slice(&bytes);
        match Reader::from_reader(std::io::Cursor::new(prefixed)) {
            Err(Error::Container(_)) => {}
            Err(other) => panic!("expected a container error, got {other:?}"),
            Ok(_) => panic!("a prefixed archive must not open as a .tsra"),
        }
    }

    /// The other prefix shape, which `zip` opens happily: a shell/zip polyglot whose offsets were
    /// rewritten to include the prefix. Every hash in it still verifies, so only the leading-entry rule
    /// in `open_tsra_archive` stands between it and a successful open — through `from_reader`, `open`,
    /// and the diagnostic `read_manifest_unverified` alike.
    #[test]
    fn a_polyglot_whose_offsets_include_the_prefix_is_refused() {
        let (bytes, order) = sealed_multiblock_bytes();
        let prefix = b"#!/bin/sh\nexit 0\n";
        let polyglot = repack_raw(&bytes, prefix, &order);
        // Precondition: this really is a zip that opens at the pinned offset, with the prefix
        // accounted for, so a refusal can only come from the SPEC §6 check.
        let config = Config {
            archive_offset: ArchiveOffset::Known(0),
        };
        let zip = ZipArchive::with_config(config, std::io::Cursor::new(&polyglot)).unwrap();
        assert_eq!(
            zip.by_index_data(0).unwrap().header_start(),
            prefix.len() as u64
        );
        drop(zip);
        // Control: the identical repack without the prefix still opens, so the repack is faithful.
        Reader::from_reader(std::io::Cursor::new(repack_raw(&bytes, b"", &order))).unwrap();

        assert_refused(polyglot.clone(), "carries a prefix");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("polyglot.tsra");
        std::fs::write(&path, &polyglot).unwrap();
        assert!(matches!(Reader::open(&path), Err(Error::Container(_))));
        assert!(matches!(
            read_manifest_unverified(&path),
            Err(Error::Container(_))
        ));
    }

    /// SPEC §6 orders the entries: `mimetype` first. An archive at byte 0 that leads with anything else
    /// is refused, even though every entry, the magic included, is present and verifies.
    #[test]
    fn an_archive_whose_first_entry_is_not_mimetype_is_refused() {
        let (bytes, mut order) = sealed_multiblock_bytes();
        order.swap(0, 2); // manifest.json first, mimetype third
        assert_refused(
            repack_raw(&bytes, b"", &order),
            "the first entry is 'manifest.json'",
        );
    }

    /// SPEC §6 (ADR-0066 rule 3): `manifest.seal` is entry 2 and `manifest.json` entry 3.
    #[test]
    fn an_archive_whose_seal_is_not_entry_2_is_refused() {
        let (bytes, mut order) = sealed_multiblock_bytes();
        order.swap(1, 2); // mimetype, manifest.json, manifest.seal
        assert_refused(
            repack_raw(&bytes, b"", &order),
            "entry 2 is 'manifest.json'",
        );
    }

    #[test]
    fn pack_streaming_equals_pack_byte_for_byte() {
        // Writer-determinism: the streaming-from-disk packer MUST produce byte-identical archives
        // to the RAM packer over the same inputs (same options, same order, same content). Anything
        // else breaks the content_hash gate the moment seal() switches from pack to pack_streaming.
        let dir = tempfile::tempdir().unwrap();
        let mut bldr = ProductBuilder::new("recon", "DPpack", "d", "2024-01-01T00:00:00Z");
        // a handful of blocks of varied sizes — including >64 KiB so we cross the buffered-copy chunk
        // boundary (the streaming path uses a 256 KiB BufReader; mid-block copy boundaries must agree).
        let payloads: Vec<BlockPayload> = (0..4)
            .map(|i| {
                let bytes: Vec<u8> = (0..(300_000 + i * 7))
                    .map(|k| ((k + i) % 251) as u8)
                    .collect();
                let nm = format!("blob_{i}");
                let digest = tessera_core::hash::digest(&bytes);
                bldr.add_block_ref(tessera_core::block::BlockRef {
                    name: bn(&nm),
                    kind: tessera_core::block::BlockKind::Array,
                    digest: Some(digest),
                    class: tessera_core::block::BlockClass::Primary,
                    spec: serde_json::json!({}),
                });
                BlockPayload::new(bn(&nm), bytes)
            })
            .collect();
        let sealed = bldr.seal().unwrap();

        // RAM path: pack(...) materialises every block payload as a Vec<u8>.
        let ram_path = dir.path().join("ram.tsra");
        pack(&sealed, &payloads, &ram_path).unwrap();
        let ram_bytes = std::fs::read(&ram_path).unwrap();

        // Streaming path: write each payload to a fragment file, then pack_streaming reads them via
        // std::io::copy — never holding a payload in RAM.
        let stage = dir.path().join("frags");
        std::fs::create_dir_all(&stage).unwrap();
        let mut frag_paths = Vec::new();
        for p in &payloads {
            let fp = stage.join(p.name.as_str());
            std::fs::write(&fp, &p.bytes).unwrap();
            frag_paths.push((p.name.clone(), fp));
        }
        let sources: Vec<(BlockName, &Path)> = frag_paths
            .iter()
            .map(|(n, p)| (n.clone(), p.as_path()))
            .collect();
        let stream_path = dir.path().join("stream.tsra");
        pack_streaming(&sealed, &sources, &stream_path).unwrap();
        let stream_bytes = std::fs::read(&stream_path).unwrap();

        assert_eq!(
            ram_bytes, stream_bytes,
            "pack_streaming and pack must produce byte-identical archives"
        );
    }

    #[test]
    fn pack_streaming_verified_matches_pack_streaming_byte_for_byte_on_honest_input() {
        // Determinism gate for the verifying packer: with truthful inputs (the fragment bytes hash
        // to the digest the manifest records) the bytes on disk are byte-identical to
        // pack_streaming over the same inputs. The verify happens on the bytes already buffered
        // for the write — no extra entries, no reordered copy, no perturbation. The conformance
        // corpus (golden archives) MUST stay untouched after this commit.
        let dir = tempfile::tempdir().unwrap();
        let mut bldr = ProductBuilder::new("recon", "DPverify", "d", "2024-01-01T00:00:00Z");
        let payloads: Vec<BlockPayload> = (0..3)
            .map(|i| {
                let bytes: Vec<u8> = (0..(290_000 + i * 11))
                    .map(|k| ((k + i) % 253) as u8)
                    .collect();
                let nm = format!("blob_{i}");
                let digest = tessera_core::hash::digest(&bytes);
                bldr.add_block_ref(tessera_core::block::BlockRef {
                    name: bn(&nm),
                    kind: tessera_core::block::BlockKind::Array,
                    digest: Some(digest),
                    class: tessera_core::block::BlockClass::Primary,
                    spec: serde_json::json!({}),
                });
                BlockPayload::new(bn(&nm), bytes)
            })
            .collect();
        let sealed = bldr.seal().unwrap();

        let stage = dir.path().join("frags");
        std::fs::create_dir_all(&stage).unwrap();
        let mut frag_paths = Vec::new();
        for p in &payloads {
            let fp = stage.join(p.name.as_str());
            std::fs::write(&fp, &p.bytes).unwrap();
            frag_paths.push((p.name.clone(), fp));
        }
        let sources: Vec<(BlockName, &Path)> = frag_paths
            .iter()
            .map(|(n, p)| (n.clone(), p.as_path()))
            .collect();
        let unverified = dir.path().join("unverified.tsra");
        let verified = dir.path().join("verified.tsra");
        pack_streaming(&sealed, &sources, &unverified).unwrap();
        pack_streaming_verified(&sealed, &sources, &verified).unwrap();
        assert_eq!(
            std::fs::read(&unverified).unwrap(),
            std::fs::read(&verified).unwrap(),
            "pack_streaming_verified must be byte-identical to pack_streaming on honest input"
        );
    }

    #[test]
    fn pack_streaming_verified_catches_fragment_mutated_between_hash_and_pack() {
        // The race this exists to close: blob streaming ingest hashes the source file to seal the
        // manifest, then the packer copies the same file's bytes into the .tsra. If the file is
        // replaced between those two reads, pack_streaming would silently seal an archive whose
        // packed bytes don't match the recorded digest (caught only later, on read). The verifying
        // packer must catch the mismatch AT PACK TIME with a typed Error::Integrity.
        let dir = tempfile::tempdir().unwrap();
        // Simulate the race: hash bytes A, seal the manifest with that digest, then write bytes B
        // to the fragment path. pack_streaming_verified must reject the mismatch.
        let bytes_a: Vec<u8> = (0..40_000u32).map(|k| (k % 251) as u8).collect();
        let bytes_b: Vec<u8> = (0..40_000u32).map(|k| ((k + 7) % 251) as u8).collect();
        assert_ne!(bytes_a, bytes_b, "test setup: A and B must differ");

        let digest_a = tessera_core::hash::digest(&bytes_a);
        let mut bldr = ProductBuilder::new("blob", "race", "x", "2024-01-01T00:00:00Z");
        bldr.add_block_ref(tessera_core::block::BlockRef {
            name: bn("data"),
            kind: tessera_core::block::BlockKind::Blob,
            digest: Some(digest_a.clone()),
            class: tessera_core::block::BlockClass::Primary,
            spec: serde_json::json!({}),
        });
        let sealed = bldr.seal().unwrap();

        let frag = dir.path().join("race.bin");
        std::fs::write(&frag, &bytes_b).unwrap(); // "the file changed between hash and pack"
        let out = dir.path().join("race.tsra");
        let err = pack_streaming_verified(&sealed, &[(bn("data"), frag.as_path())], &out)
            .expect_err("mismatched fragment must fail the verifying pack");
        match err {
            Error::Integrity {
                what,
                expected,
                actual,
            } => {
                assert_eq!(what, "block_payload");
                assert_eq!(
                    expected, digest_a,
                    "must surface the manifest-recorded digest"
                );
                assert_eq!(actual, tessera_core::hash::digest(&bytes_b));
            }
            other => panic!("expected Error::Integrity, got {other:?}"),
        }

        // Sanity: the non-verifying packer happily seals the same race (proving the gap the
        // verifying packer closes — and that's the silent bad archive that surfaces only on read).
        let dead = dir.path().join("dead.tsra");
        pack_streaming(&sealed, &[(bn("data"), frag.as_path())], &dead).unwrap();
        let mut rdr = Reader::open(&dead).unwrap();
        assert!(matches!(
            rdr.read_block("data"),
            Err(Error::Integrity {
                what: "block_payload",
                ..
            })
        ));
    }

    #[test]
    fn pack_streaming_verified_rejects_unknown_block_name() {
        // A typo'd source name (no matching block in the manifest) is a producer bug, not a verify
        // miss — fail loudly, before opening the file, with a typed Container error.
        let dir = tempfile::tempdir().unwrap();
        let bytes = b"abcd".to_vec();
        let digest = tessera_core::hash::digest(&bytes);
        let mut bldr = ProductBuilder::new("blob", "x", "x", "2024-01-01T00:00:00Z");
        bldr.add_block_ref(tessera_core::block::BlockRef {
            name: bn("data"),
            kind: tessera_core::block::BlockKind::Blob,
            digest: Some(digest),
            class: tessera_core::block::BlockClass::Primary,
            spec: serde_json::json!({}),
        });
        let sealed = bldr.seal().unwrap();
        let frag = dir.path().join("f.bin");
        std::fs::write(&frag, &bytes).unwrap();
        let out = dir.path().join("x.tsra");
        let err = pack_streaming_verified(&sealed, &[(bn("typo"), frag.as_path())], &out)
            .expect_err("unknown block name must be rejected");
        assert!(
            matches!(err, Error::Container(ref m) if m.contains("no manifest block named 'typo'")),
            "expected Container error naming the missing block, got {err:?}"
        );
    }

    #[test]
    fn aux_members_are_carried_but_ignored_by_the_seal() {
        // ADR-0042 seal-ignore invariant — the load-bearing guarantee this whole change hangs on.
        // Adding (or replacing) an `aux/` member MUST leave `content_hash` AND `manifest_hash`
        // byte-identical, so the existing conformance corpus never needs regeneration.
        let vol = ArrayBlock::new("volume", ArraySpec::new(vec![8, 8, 8], "int16"));
        let payload = serde_json::to_vec(&vol.spec).unwrap();
        let mut b = ProductBuilder::new("recon", "AUX", "seal-ignore", "2024-01-01T00:00:00Z");
        b.add_block(&vol).unwrap();
        b.with_field(
            "modality",
            serde_json::json!({"_vocabulary": "DICOM", "_code": "CT"}),
        );
        let sealed = b.seal().unwrap();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("aux.tsra");
        pack(&sealed, &[BlockPayload::new(bn("volume"), payload)], &path).unwrap();

        // Snapshot the pre-aux hashes and the on-disk manifest.json bytes — the invariant is that
        // the SEALED region is byte-preserved, not merely that the hashes happen to match.
        let (pre_ch, pre_mh, pre_manifest_json) = {
            let mut zip = ZipArchive::new(File::open(&path).unwrap()).unwrap();
            let mut mj = Vec::new();
            zip.by_name(MANIFEST_ENTRY)
                .unwrap()
                .read_to_end(&mut mj)
                .unwrap();
            let r = Reader::from_reader(File::open(&path).unwrap()).unwrap();
            (
                r.manifest().content_hash.clone(),
                r.manifest().manifest_hash.clone(),
                mj,
            )
        };

        // Add two aux members: an embedded-signature look-alike and a provenance stamp.
        add_aux_members(
            &path,
            &[
                AuxMember::new("signatures/deadbeef.sig.json", br#"{"stub":true}"#.to_vec()),
                AuxMember::new(
                    "provenance.json",
                    br#"{"ingested_at":"2026-06-25T00:00:00Z","producer":"tessera/x","host":"h"}"#
                        .to_vec(),
                ),
            ],
        )
        .unwrap();

        // Reopen: the seal-covered hashes are byte-identical, AND the manifest.json bytes are too.
        let (post_ch, post_mh, post_manifest_json) = {
            let mut zip = ZipArchive::new(File::open(&path).unwrap()).unwrap();
            let mut mj = Vec::new();
            zip.by_name(MANIFEST_ENTRY)
                .unwrap()
                .read_to_end(&mut mj)
                .unwrap();
            let r = Reader::from_reader(File::open(&path).unwrap()).unwrap();
            (
                r.manifest().content_hash.clone(),
                r.manifest().manifest_hash.clone(),
                mj,
            )
        };
        assert_eq!(pre_ch, post_ch, "content_hash MUST be byte-identical");
        assert_eq!(pre_mh, post_mh, "manifest_hash MUST be byte-identical");
        assert_eq!(
            pre_manifest_json, post_manifest_json,
            "manifest.json bytes MUST be byte-preserved (no JSON reformatting)"
        );

        // The reader surfaces the aux members and reads them verbatim.
        let mut r = Reader::open(&path).unwrap();
        let names = r.aux_names();
        assert!(names.contains(&"signatures/deadbeef.sig.json".to_string()));
        assert!(names.contains(&"provenance.json".to_string()));
        assert_eq!(
            r.read_aux("signatures/deadbeef.sig.json").unwrap(),
            br#"{"stub":true}"#
        );
        // A block read still passes its digest check (nothing about the seal moved).
        r.read_block("volume").unwrap();

        // Re-adding the same aux name overwrites it (so re-sign replaces the previous signature).
        add_aux_members(
            &path,
            &[AuxMember::new(
                "signatures/deadbeef.sig.json",
                br#"{"stub":"replaced"}"#.to_vec(),
            )],
        )
        .unwrap();
        let mut r = Reader::open(&path).unwrap();
        assert_eq!(
            r.read_aux("signatures/deadbeef.sig.json").unwrap(),
            br#"{"stub":"replaced"}"#
        );
        // The seal-ignore invariant STILL holds after the overwrite.
        assert_eq!(r.manifest().content_hash, pre_ch);
        assert_eq!(r.manifest().manifest_hash, pre_mh);
    }

    #[test]
    fn unpack_pack_round_trips_the_aux_subtree() {
        // A `.tsra` that carries embedded aux (signature + provenance) survives unpack→pack: the
        // sealed hashes are unchanged AND the aux members are recovered under `aux/<name>` in the
        // exploded dir, then re-embedded by `pack_dir` back into the sealed .tsra.
        let vol = ArrayBlock::new("volume", ArraySpec::new(vec![4, 4, 4], "int16"));
        let payload = serde_json::to_vec(&vol.spec).unwrap();
        let mut b = ProductBuilder::new("recon", "RTRIP", "d", "2024-01-01T00:00:00Z");
        b.add_block(&vol).unwrap();
        b.with_field("modality", serde_json::json!("CT"));
        let sealed = b.seal().unwrap();

        let dir = tempfile::tempdir().unwrap();
        let orig = dir.path().join("orig.tsra");
        pack(&sealed, &[BlockPayload::new(bn("volume"), payload)], &orig).unwrap();
        add_aux_members(
            &orig,
            &[
                AuxMember::new("signatures/k.sig.json", br#"{"sig":true}"#.to_vec()),
                AuxMember::new("provenance.json", br#"{"ingested_at":"t"}"#.to_vec()),
            ],
        )
        .unwrap();
        let (ch, mh) = {
            let r = Reader::open(&orig).unwrap();
            (
                r.manifest().content_hash.clone(),
                r.manifest().manifest_hash.clone(),
            )
        };

        // unpack lays down `manifest.json` + `blocks/volume` + `aux/signatures/k.sig.json` + `aux/provenance.json`.
        let exploded = dir.path().join("out");
        unpack(&orig, &exploded).unwrap();
        assert!(exploded.join("aux/signatures/k.sig.json").exists());
        assert!(exploded.join("aux/provenance.json").exists());

        // pack_dir reassembles them; the seal is byte-identical and the aux members are back inside.
        let repacked = dir.path().join("re.tsra");
        pack_dir(&exploded, &repacked).unwrap();
        let r = Reader::open(&repacked).unwrap();
        assert_eq!(r.manifest().content_hash, ch, "seal must be preserved");
        assert_eq!(r.manifest().manifest_hash, mh, "seal must be preserved");
        let names = r.aux_names();
        assert!(names.contains(&"signatures/k.sig.json".to_string()));
        assert!(names.contains(&"provenance.json".to_string()));
    }

    // The deprecated legacy rule, kept under test (ADR-0064 §6): `block_group` delegates to
    // `is_legacy_flat_member`, and these are its cases from before it did.
    #[test]
    #[allow(deprecated)]
    fn block_group_collects_prefix_and_partitioned_shards_in_manifest_order() {
        // The reader-side counterpart of table::block_name: a single `events` block OR an ordered
        // sweep of `events_NNNN` blocks. Other names with similar prefixes (`events_index`, etc.)
        // are NOT matched — only the exact `prefix` or `prefix_<4 digits>` form.
        let mut b = ProductBuilder::new("listmode", "g", "d", "2024-01-01T00:00:00Z");
        for name in [
            "events_0001",
            "events_0000",
            "noise",        // unrelated → excluded
            "events",       // bare prefix → included
            "events_index", // wrong shape → excluded
            "events_99",    // wrong digit count → excluded
            "events_0010",
        ] {
            let bytes = name.as_bytes().to_vec();
            let digest = tessera_core::hash::digest(&bytes);
            b.add_block_ref(tessera_core::block::BlockRef {
                name: bn(name),
                kind: tessera_core::block::BlockKind::Table,
                digest: Some(digest),
                class: tessera_core::block::BlockClass::Primary,
                spec: serde_json::json!({"columns": [], "rows": 0}),
            });
        }
        let sealed = b.seal().unwrap();
        let payloads: Vec<BlockPayload> = sealed
            .blocks
            .iter()
            .map(|r| BlockPayload::new(r.name.clone(), r.name.as_bytes().to_vec()))
            .collect();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("g.tsra");
        pack(&sealed, &payloads, &path).unwrap();
        let r = Reader::open(&path).unwrap();
        // manifest order (push order) preserved: 0001, 0000, events, 0010.
        assert_eq!(
            r.block_group("events"),
            vec!["events_0001", "events_0000", "events", "events_0010"]
        );
        // a prefix with NO matches returns an empty vec (not an error).
        assert!(r.block_group("missing").is_empty());
    }
}
