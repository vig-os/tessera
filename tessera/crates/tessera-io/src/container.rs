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
use zip::{CompressionMethod, ZipArchive, ZipWriter};

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

/// Pack a **sealed** manifest + its block payloads into a `.tsra` at `path` (STORED zip64).
pub fn pack(manifest: &Manifest, payloads: &[BlockPayload], path: &Path) -> Result<()> {
    if !manifest.is_sealed() {
        return Err(Error::Container(
            "refusing to pack an unsealed manifest".into(),
        ));
    }
    let mut zw = ZipWriter::new(File::create(path)?);
    // STORED + force zip64 so a large study is never silently truncated at 4 GiB / 65 k entries.
    let stored = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .large_file(true);

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
    }
}
