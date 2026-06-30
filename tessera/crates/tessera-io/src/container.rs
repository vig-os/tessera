//! The `.tsra` container — a STORED zip64 archive (ADR-0022).
//!
//! Layout: `mimetype` (first, uncompressed, magic) · `manifest.json` · `blocks/<name>` payloads.
//! Entries are **STORED** (payloads are already compressed by their codec) so every block byte
//! range is directly addressable via the zip central directory — a cloud reader range-reads just
//! the manifest + the blocks it needs, no whole-archive download. The reader verifies the magic,
//! the manifest seal, and (on access) each stored block's bytes against its recorded digest.

use std::fs::File;
use std::io::{Read, Seek, Write};
use std::path::Path;

use tessera_core::manifest::Manifest;
use tessera_core::{Error, Result};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipArchive, ZipWriter};

/// Container MIME magic — the first, uncompressed entry (EPUB/ODF trick) so `file(1)` and magic
/// sniffers identify a `.tsra` without unzipping.
pub const MIMETYPE: &str = "application/vnd.tessera";
const MIMETYPE_ENTRY: &str = "mimetype";
const MANIFEST_ENTRY: &str = "manifest.json";
const BLOCKS_PREFIX: &str = "blocks/";

/// A block's encoded bytes, to be stored at `blocks/<name>`. `bytes` MUST be the exact bytes the
/// block's recorded digest was computed over (so the reader can verify integrity on access).
pub struct BlockPayload {
    pub name: String,
    pub bytes: Vec<u8>,
}

impl BlockPayload {
    pub fn new(name: impl Into<String>, bytes: Vec<u8>) -> Self {
        BlockPayload {
            name: name.into(),
            bytes,
        }
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
}

/// Pack a **sealed** manifest + its block payloads into a `.tsra` at `path` (STORED zip64).
pub fn pack(manifest: &Manifest, payloads: &[BlockPayload], path: &Path) -> Result<()> {
    if !manifest.is_sealed() {
        return Err(Error::Container(
            "refusing to pack an unsealed manifest".into(),
        ));
    }
    let mut zw = ZipWriter::new(File::create(path)?);
    // STORED + force zip64 so a large study is never silently truncated at 4 GiB / 65 k entries.
    // Pin the entry mtime to the zip epoch (1980-01-01) so the same product packs to byte-identical
    // bytes — writer-determinism is a release gate (FEATURE-MATRIX §C); a wall-clock mtime breaks it.
    let stored = tsra_entry_options();

    zw.start_file(MIMETYPE_ENTRY, stored).map_err(cz)?; // first + uncompressed = magic
    zw.write_all(MIMETYPE.as_bytes())?;

    zw.start_file(MANIFEST_ENTRY, stored).map_err(cz)?;
    zw.write_all(manifest.to_json()?.as_bytes())?;

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
/// and emit the mimetype/manifest/blocks in the same order.
pub fn pack_streaming(manifest: &Manifest, sources: &[(String, &Path)], out: &Path) -> Result<()> {
    if !manifest.is_sealed() {
        return Err(Error::Container(
            "refusing to pack an unsealed manifest".into(),
        ));
    }
    let mut zw = ZipWriter::new(File::create(out)?);
    let stored = tsra_entry_options();

    zw.start_file(MIMETYPE_ENTRY, stored).map_err(cz)?;
    zw.write_all(MIMETYPE.as_bytes())?;

    zw.start_file(MANIFEST_ENTRY, stored).map_err(cz)?;
    zw.write_all(manifest.to_json()?.as_bytes())?;

    for (name, frag) in sources {
        zw.start_file(format!("{BLOCKS_PREFIX}{name}"), stored)
            .map_err(cz)?;
        // Buffered copy: 256 KiB chunks → peak RAM ≈ one buffer (no full-block materialisation).
        let f = File::open(frag)?;
        let mut br = std::io::BufReader::with_capacity(256 * 1024, f);
        std::io::copy(&mut br, &mut zw)?;
    }
    zw.finish().map_err(cz)?;
    Ok(())
}

/// A reader over a `.tsra`. Generic over any `Read + Seek` byte source, so the same reader serves
/// a local file today and an object-store range-reader (S6) later.
pub struct Reader<R: Read + Seek> {
    archive: ZipArchive<R>,
    manifest: Manifest,
}

impl Reader<File> {
    /// Open + verify a `.tsra` file: magic, manifest parse, and full seal verification.
    pub fn open(path: &Path) -> Result<Self> {
        Self::from_reader(File::open(path)?)
    }
}

impl<R: Read + Seek> Reader<R> {
    /// Build a reader from any seekable byte source. Checks the `mimetype` magic, parses
    /// `manifest.json`, and verifies all three hashes (id / content_hash / manifest_hash).
    pub fn from_reader(reader: R) -> Result<Self> {
        let mut archive = ZipArchive::new(reader).map_err(cz)?;

        let mut magic = String::new();
        archive
            .by_name(MIMETYPE_ENTRY)
            .map_err(|_| Error::Container("not a .tsra (no mimetype entry)".into()))?
            .read_to_string(&mut magic)?;
        if magic != MIMETYPE {
            return Err(Error::Container(format!("bad container magic: {magic:?}")));
        }

        let mut mj = String::new();
        archive
            .by_name(MANIFEST_ENTRY)
            .map_err(|_| Error::Container("missing manifest.json".into()))?
            .read_to_string(&mut mj)?;
        let manifest = Manifest::from_json_verified(&mj)?;

        Ok(Reader { archive, manifest })
    }

    /// The verified manifest (read without touching any payload — partial-product access).
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// Names of the blocks recorded in the manifest.
    pub fn block_names(&self) -> Vec<String> {
        self.manifest
            .blocks
            .iter()
            .map(|b| b.name.clone())
            .collect()
    }

    /// Names of all blocks in the manifest that belong to `prefix`'s partitioned group, in **manifest
    /// order** — every block whose name is `prefix` (the single-block case) or `prefix_NNNN` where the
    /// suffix is a 4-digit zero-padded number (the multi-block case from
    /// [`crate::table::block_name`]). Used by readers that need to iterate over a logically-split
    /// table without knowing whether it was partitioned (e.g. `events` vs `events_0000..events_NNNN`).
    pub fn block_group(&self, prefix: &str) -> Vec<String> {
        self.manifest
            .blocks
            .iter()
            .filter_map(|b| {
                if b.name == prefix {
                    Some(b.name.clone())
                } else if let Some(rest) = b
                    .name
                    .strip_prefix(prefix)
                    .and_then(|r| r.strip_prefix('_'))
                {
                    // a 4-digit numeric suffix marks a partitioned shard (block_name's format).
                    if rest.len() == 4 && rest.bytes().all(|c| c.is_ascii_digit()) {
                        Some(b.name.clone())
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect()
    }

    /// Read one block's bytes by name — a targeted read of just that entry (located via the
    /// central directory), then verified against the manifest's recorded digest. A byte that
    /// doesn't match the seal is a typed [`Error::Integrity`].
    pub fn read_block(&mut self, name: &str) -> Result<Vec<u8>> {
        let entry = format!("{BLOCKS_PREFIX}{name}");
        let buf = {
            let mut f = self
                .archive
                .by_name(&entry)
                .map_err(|_| Error::Container(format!("no block entry '{entry}'")))?;
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
        let mut f = self
            .archive
            .by_name(&entry)
            .map_err(|_| Error::Container(format!("no block entry '{entry}'")))?;
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
}

/// Explode a `.tsra` into a directory: `manifest.json` + `blocks/<name>` (the opt-in exploded
/// form of ADR-0022). Verifies the seal on open and each block against its digest. Returns the
/// verified manifest.
pub fn unpack(path: &Path, outdir: &Path) -> Result<Manifest> {
    let mut r = Reader::open(path)?;
    let manifest = r.manifest().clone();
    std::fs::create_dir_all(outdir.join("blocks"))?;
    std::fs::write(outdir.join("manifest.json"), manifest.to_json()?)?;
    for name in manifest.blocks.iter().map(|b| b.name.clone()) {
        let bytes = r.read_block(&name)?;
        std::fs::write(outdir.join("blocks").join(&name), bytes)?;
    }
    Ok(manifest)
}

/// Pack an exploded directory (`manifest.json` + `blocks/<name>`) back into a sealed `.tsra`.
/// The manifest is seal-verified before packing; each block's payload is read from `blocks/`.
pub fn pack_dir(dir: &Path, out: &Path) -> Result<()> {
    let mj = std::fs::read_to_string(dir.join("manifest.json"))?;
    let manifest = Manifest::from_json_verified(&mj)?;
    let mut payloads = Vec::with_capacity(manifest.blocks.len());
    for b in &manifest.blocks {
        let bytes = std::fs::read(dir.join("blocks").join(&b.name))?;
        payloads.push(BlockPayload::new(b.name.clone(), bytes));
    }
    pack(&manifest, &payloads, out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tessera_core::block::array::{ArrayBlock, ArraySpec};
    use tessera_core::ProductBuilder;

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
            &[BlockPayload::new("volume", payload.clone())],
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
            &[BlockPayload::new("volume", b"not the spec".to_vec())],
            &path,
        )
        .unwrap();

        let mut r = Reader::open(&path).unwrap();
        match r.read_block("volume") {
            Err(Error::Integrity { what, .. }) => assert_eq!(what, "block_payload"),
            other => panic!("expected block_payload integrity error, got {other:?}"),
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
                    name: nm.clone(),
                    kind: tessera_core::block::BlockKind::Array,
                    digest: Some(digest),
                    spec: serde_json::json!({}),
                });
                BlockPayload::new(nm, bytes)
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
            let fp = stage.join(&p.name);
            std::fs::write(&fp, &p.bytes).unwrap();
            frag_paths.push((p.name.clone(), fp));
        }
        let sources: Vec<(String, &Path)> = frag_paths
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
                name: name.into(),
                kind: tessera_core::block::BlockKind::Table,
                digest: Some(digest),
                spec: serde_json::json!({}),
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
