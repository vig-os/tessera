//! Streaming write engine — fragment-append with hash-on-write + crash-recovery (ROADMAP P3 / S17).
//!
//! The DAQ rule (S17): **never encode on the hot path** into a single sealed file (a footer-at-end
//! format means a crash = total loss). Instead a [`WriteSession`] stages each already-encoded block
//! as a durable **fragment** under a directory and records it in an append-only **journal**; the
//! journal line is the commit point. A crash leaves the session consistent up to the last intact
//! journal line (the **watermark**); [`WriteSession::recover`] replays to it, dropping any torn tail.
//! At completion [`WriteSession::seal`] builds the manifest through the same [`ProductBuilder`] a
//! batch writer uses, so a streamed product is byte-identical to a batch-sealed one (same `id` /
//! `content_hash` / `manifest_hash`). The running [`MerkleAccumulator`] gives an integrity root at
//! every watermark and is cross-checked against the sealed `content_hash`.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tessera_core::block::BlockRef;
use tessera_core::hash::MerkleAccumulator;
use tessera_core::manifest::Manifest;
use tessera_core::{Error, ProductBuilder, Result};

use crate::{pack, BlockPayload};

const HEADER: &str = "header.json";
const JOURNAL: &str = "journal.jsonl";
const BLOCKS: &str = "blocks";

/// The manifest header captured at session creation — everything needed to seal except the blocks.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Header {
    product: String,
    name: String,
    description: String,
    timestamp: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    study: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    metadata: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    extra: BTreeMap<String, Value>,
}

/// A crash-tolerant streaming writer over a staging directory.
pub struct WriteSession {
    dir: PathBuf,
    header: Header,
    journal: File,
    blocks: Vec<BlockRef>,
    merkle: MerkleAccumulator,
}

fn cz(e: impl std::fmt::Display) -> Error {
    Error::Container(e.to_string())
}

impl WriteSession {
    /// Start a new session in `dir` (created if absent): writes the header and an empty journal.
    pub fn create(
        dir: &Path,
        product: impl Into<String>,
        name: impl Into<String>,
        description: impl Into<String>,
        timestamp: impl Into<String>,
    ) -> Result<Self> {
        fs::create_dir_all(dir.join(BLOCKS))?;
        let header = Header {
            product: product.into(),
            name: name.into(),
            description: description.into(),
            timestamp: timestamp.into(),
            study: None,
            metadata: BTreeMap::new(),
            extra: BTreeMap::new(),
        };
        fs::write(dir.join(HEADER), serde_json::to_vec_pretty(&header)?)?;
        let journal = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(dir.join(JOURNAL))?;
        journal.sync_all()?;
        Ok(WriteSession {
            dir: dir.to_path_buf(),
            header,
            journal,
            blocks: Vec::new(),
            merkle: MerkleAccumulator::new(),
        })
    }

    /// Set the study id (must be called before any metadata is sealed; persisted to the header).
    pub fn with_study(&mut self, study: impl Into<String>) -> Result<&mut Self> {
        self.header.study = Some(study.into());
        self.persist_header()?;
        Ok(self)
    }

    /// Set a schema metadata field (persisted to the header).
    pub fn with_field(&mut self, id: impl Into<String>, value: Value) -> Result<&mut Self> {
        self.header.metadata.insert(id.into(), value);
        self.persist_header()?;
        Ok(self)
    }

    /// Add an extension (`extra/`) field (persisted to the header).
    pub fn with_extra(&mut self, key: impl Into<String>, value: Value) -> Result<&mut Self> {
        self.header.extra.insert(key.into(), value);
        self.persist_header()?;
        Ok(self)
    }

    fn persist_header(&self) -> Result<()> {
        fs::write(
            self.dir.join(HEADER),
            serde_json::to_vec_pretty(&self.header)?,
        )?;
        Ok(())
    }

    /// Durably append one already-encoded block: write the fragment, fsync, then commit a journal
    /// line (the atomic commit point), fsync, then fold the digest into the running Merkle root.
    /// `block_ref` MUST carry a digest computed over `payload`.
    pub fn append_block(&mut self, block_ref: BlockRef, payload: &[u8]) -> Result<()> {
        let digest = block_ref
            .digest
            .clone()
            .ok_or_else(|| Error::MissingDigest(block_ref.name.clone()))?;
        // 1. fragment → durable
        let frag = File::create(self.dir.join(BLOCKS).join(&block_ref.name))?;
        {
            let mut w = frag;
            w.write_all(payload)?;
            w.sync_all()?;
        }
        // 2. journal line → durable (this is the commit watermark advance)
        writeln!(self.journal, "{}", serde_json::to_string(&block_ref)?)?;
        self.journal.sync_all()?;
        // 3. running integrity root
        self.merkle.push(&digest);
        self.blocks.push(block_ref);
        Ok(())
    }

    /// The number of committed blocks (the crash-recovery watermark).
    pub fn watermark(&self) -> usize {
        self.merkle.watermark()
    }

    /// The running content Merkle root over all committed blocks (the `content_hash` a seal here
    /// would produce).
    pub fn running_root(&self) -> String {
        self.merkle.root()
    }

    /// Reopen a staging directory after a crash, replaying the journal to the last intact line. A
    /// torn final line (partial write) fails to parse and is dropped — the watermark is the last
    /// fully-committed block — and the journal is rewritten to that clean prefix so appends resume.
    pub fn recover(dir: &Path) -> Result<Self> {
        let header: Header = serde_json::from_slice(&fs::read(dir.join(HEADER))?)?;
        let raw = fs::read_to_string(dir.join(JOURNAL))?;
        let mut blocks = Vec::new();
        let mut merkle = MerkleAccumulator::new();
        for line in raw.lines() {
            match serde_json::from_str::<BlockRef>(line) {
                Ok(r) => {
                    let digest = r
                        .digest
                        .clone()
                        .ok_or_else(|| Error::MissingDigest(r.name.clone()))?;
                    // a committed block must have its fragment on disk
                    if !dir.join(BLOCKS).join(&r.name).exists() {
                        break;
                    }
                    merkle.push(&digest);
                    blocks.push(r);
                }
                Err(_) => break, // torn tail
            }
        }
        // Rewrite the journal to the clean recovered prefix (drops any torn tail), then reopen append.
        let clean: String = blocks
            .iter()
            .map(|r| serde_json::to_string(r).map(|s| s + "\n"))
            .collect::<std::result::Result<_, _>>()?;
        fs::write(dir.join(JOURNAL), &clean)?;
        let journal = OpenOptions::new().append(true).open(dir.join(JOURNAL))?;
        Ok(WriteSession {
            dir: dir.to_path_buf(),
            header,
            journal,
            blocks,
            merkle,
        })
    }

    /// Compact + seal: build the manifest through [`ProductBuilder`] (identical to a batch writer),
    /// verify the running root matches the sealed `content_hash`, and pack the fragments into a
    /// sealed `.tsra` at `out`. Returns the sealed manifest.
    pub fn seal(self, out: &Path) -> Result<Manifest> {
        let mut b = ProductBuilder::new(
            &self.header.product,
            &self.header.name,
            &self.header.description,
            &self.header.timestamp,
        );
        if let Some(study) = &self.header.study {
            b.with_study(study.clone());
        }
        for (k, v) in &self.header.metadata {
            b.with_field(k.clone(), v.clone());
        }
        for (k, v) in &self.header.extra {
            b.with_extra(k.clone(), v.clone());
        }
        for r in &self.blocks {
            b.add_block_ref(r.clone());
        }
        let sealed = b.seal()?;
        // Integrity self-check: the streamed running root must equal the batch content hash.
        if sealed.content_hash.as_deref() != Some(self.merkle.root().as_str()) {
            return Err(Error::Integrity {
                what: "streaming_content_hash",
                expected: sealed.content_hash.clone().unwrap_or_default(),
                actual: self.merkle.root(),
            });
        }
        let mut payloads = Vec::with_capacity(self.blocks.len());
        for r in &self.blocks {
            let bytes = fs::read(self.dir.join(BLOCKS).join(&r.name)).map_err(cz)?;
            payloads.push(BlockPayload::new(r.name.clone(), bytes));
        }
        pack(&sealed, &payloads, out)?;
        Ok(sealed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::array::{self, ArrayData};
    use crate::Reader;
    use tessera_core::block::array::ArraySpec;

    const TS: &str = "2024-01-01T00:00:00Z";

    fn block(name: &str, n: i16) -> (BlockRef, Vec<u8>) {
        let mut spec = ArraySpec::new(vec![4, 4, 4], "int16");
        spec.codec = "pcodec".into();
        let data = ArrayData::I16((0..64).map(|k| k as i16 + n).collect());
        let (r, payload) = array::array_block(name, &spec, &data).unwrap();
        (r, payload.bytes)
    }

    #[test]
    fn streamed_product_is_identical_to_batch_and_verifies() {
        let dir = tempfile::tempdir().unwrap();
        let stage = dir.path().join("stage");

        let mut ws = WriteSession::create(&stage, "recon", "p", "d", TS).unwrap();
        ws.with_field("modality", serde_json::json!("CT")).unwrap();
        let (r1, p1) = block("a", 0);
        let (r2, p2) = block("b", 10);
        ws.append_block(r1.clone(), &p1).unwrap();
        ws.append_block(r2.clone(), &p2).unwrap();
        assert_eq!(ws.watermark(), 2);

        let out = dir.path().join("streamed.tsra");
        let sealed = ws.seal(&out).unwrap();

        // batch-built equivalent → identical identity + content + seal
        let mut bb = ProductBuilder::new("recon", "p", "d", TS);
        bb.add_block_ref(r1);
        bb.add_block_ref(r2);
        bb.with_field("modality", serde_json::json!("CT"));
        let batch = bb.seal().unwrap();
        assert_eq!(sealed.id, batch.id);
        assert_eq!(sealed.content_hash, batch.content_hash);
        assert_eq!(sealed.manifest_hash, batch.manifest_hash);

        // the .tsra opens, verifies, and its blocks read back
        let mut rdr = Reader::open(&out).unwrap();
        for n in rdr.block_names() {
            rdr.read_block(&n).unwrap();
        }
    }

    #[test]
    fn recovery_drops_a_torn_journal_tail() {
        let dir = tempfile::tempdir().unwrap();
        let stage = dir.path().join("stage");
        let (r1, p1) = block("a", 0);
        let (r2, p2) = block("b", 10);
        {
            let mut ws = WriteSession::create(&stage, "recon", "p", "d", TS).unwrap();
            ws.append_block(r1.clone(), &p1).unwrap();
            ws.append_block(r2.clone(), &p2).unwrap();
        }
        // Simulate a crash mid-write of a 3rd block: a torn (partial, unterminated) journal line.
        let jpath = stage.join(JOURNAL);
        let mut torn = fs::read_to_string(&jpath).unwrap();
        torn.push_str("{\"name\":\"c\",\"kind\":\"arr"); // truncated JSON, no newline
        fs::write(&jpath, torn).unwrap();

        let ws = WriteSession::recover(&stage).unwrap();
        assert_eq!(ws.watermark(), 2, "torn tail must be dropped");

        // recovery rewrote a clean journal → sealing yields the same product as the 2-block batch
        let out = dir.path().join("recovered.tsra");
        let sealed = ws.seal(&out).unwrap();
        let mut bb = ProductBuilder::new("recon", "p", "d", TS);
        bb.add_block_ref(r1);
        bb.add_block_ref(r2);
        let batch = bb.seal().unwrap();
        assert_eq!(sealed.manifest_hash, batch.manifest_hash);
        Reader::open(&out).unwrap();
    }

    #[test]
    fn running_root_advances_and_equals_sealed_content_hash() {
        let dir = tempfile::tempdir().unwrap();
        let stage = dir.path().join("stage");
        let mut ws = WriteSession::create(&stage, "recon", "p", "d", TS).unwrap();
        let (r1, p1) = block("a", 0);
        ws.append_block(r1, &p1).unwrap();
        let root1 = ws.running_root();
        let (r2, p2) = block("b", 5);
        ws.append_block(r2, &p2).unwrap();
        // hash-on-write: committing another block advances the running root
        assert_ne!(root1, ws.running_root());
        let final_root = ws.running_root();
        // seal() self-checks root == content_hash; assert it here too for clarity
        let sealed = ws.seal(&dir.path().join("s.tsra")).unwrap();
        assert_eq!(sealed.content_hash.as_deref(), Some(final_root.as_str()));
    }
}
