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
//!
//! **Blocks larger than RAM (#677).** [`WriteSession::append_block`] takes the encoded bytes as a
//! slice. [`WriteSession::append_block_from`] takes them from a reader instead and hashes them while
//! it writes the fragment (ADR-0002 first-moment integrity), so a block is never held in memory.
//! The usual source is a [`StagedBlock`]: an encoder writes the block into a file in the session's
//! staging directory ([`WriteSession::staging`]), and [`WriteSession::append_staged`] streams it
//! into the fragment and then deletes it. A staged file is deleted on commit, on error and on drop,
//! and [`WriteSession::recover`] clears any a crash left behind, so staging never leaks. Both
//! appends commit the same fragment, journal line and Merkle leaf for the same bytes.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tessera_core::block::BlockRef;
use tessera_core::hash::{MerkleAccumulator, StreamHasher};
use tessera_core::manifest::Manifest;
use tessera_core::{Error, ProductBuilder, Result};

use crate::pack_streaming;

const HEADER: &str = "header.json";
const JOURNAL: &str = "journal.jsonl";
const BLOCKS: &str = "blocks";
/// Scratch files of blocks that are encoded but not yet committed ([`StagedBlock`]), and the
/// partial fragment of the append in progress. Nothing here is ever part of the product.
const STAGING: &str = "staging";
/// The partial fragment [`WriteSession::append_block_from`] writes before renaming it into
/// `blocks/`. One at a time: an append takes `&mut self`.
const APPEND_PART: &str = "append.part";
/// The copy buffer of [`WriteSession::append_block_from`] — the RAM a streamed commit uses,
/// whatever the block's size.
const APPEND_BUF: usize = 256 * 1024;

/// The manifest header captured at session creation — everything needed to seal except the blocks.
///
/// **`deny_unknown_fields` is load-bearing (#462).** A stage dir is resumed by whichever binary
/// happens to run [`WriteSession::recover`], which need not be the one that wrote it. Without this,
/// an older binary silently DROPS any field it does not know — and those fields are sealed,
/// identity-bearing provenance (`producer`, `generation`), so the resumed seal would differ from
/// the intended one with nothing to show for it. That is the same silent-loss class as #416.
///
/// Chosen over a `version` integer deliberately: a version only protects you if whoever adds the
/// next field remembers to bump it, and #416 happened precisely because a field was added without
/// its plumbing being finished. The cost is that cross-version resume fails loudly, which is the
/// correct outcome for a short-lived internal staging artifact that is cheap to redo.
///
/// **Scope: this covers TOP-LEVEL header fields only.** A new field added *inside* `Producer`,
/// `Generation` or `Source` is still dropped silently on resume. That is deliberate, not an
/// oversight: those are `tessera_core`'s own manifest types, so `deny_unknown_fields` on them would
/// apply to **manifest** deserialization too and turn every future format field into a hard break
/// for older readers — the exact forward-compatibility ADR-0058 relied on when it added `producer`
/// and `generation` behind `#[serde(default)]`. The staging header can afford strictness because it
/// is internal and short-lived; the sealed format cannot. Closing the nested gap needs a
/// stage-local mirror of those structs, which is not worth it until a nested field actually exists.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
    /// Provenance edges (DAG sources) declared on the session BEFORE seal — flow through into the
    /// final manifest, so a streaming ingest can record `ingested_from`/`derived_from` without
    /// re-sealing the manifest after the fact. Vec-of-Source instead of a map because order matters
    /// for the manifest seal (same as [`tessera_core::ProductBuilder`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    sources: Vec<tessera_core::provenance::Source>,
    /// ADR-0058 §1 producer identity, declared BEFORE seal. Like `sources`, this exists on the
    /// header rather than as a post-seal edit because the streamed `.tsra` is written once, straight
    /// to disk — there is no re-seal hook that would not mean rewriting a multi-GB archive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    producer: Option<tessera_core::Producer>,
    /// ADR-0058 §2 generation recipe, declared BEFORE seal (see `producer`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    generation: Option<tessera_core::Generation>,
}

/// A crash-tolerant streaming writer over a staging directory.
pub struct WriteSession {
    dir: PathBuf,
    header: Header,
    journal: File,
    blocks: Vec<BlockRef>,
    merkle: MerkleAccumulator,
    /// Numbers the session's staged files, shared by every [`BlockStaging`] handle.
    staged_seq: Arc<AtomicU64>,
}

/// Deletes a file when dropped, unless [`Self::disarm`]ed. Best effort: a drop can't report an
/// error, and a file that is already gone is the goal.
#[derive(Debug)]
struct RemoveOnDrop(Option<PathBuf>);

impl RemoveOnDrop {
    fn new(path: PathBuf) -> Self {
        RemoveOnDrop(Some(path))
    }

    fn path(&self) -> &Path {
        self.0.as_deref().expect("armed")
    }

    fn disarm(mut self) {
        self.0 = None;
    }
}

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = fs::remove_file(path);
        }
    }
}

/// Empty the staging directory of the session at `dir` (creating it if absent). Staged files are
/// never committed (a commit deletes its file), so whatever a crashed or abandoned writer left
/// there is scratch, and would otherwise sit on disk for good. Called by [`WriteSession::create`]
/// and [`WriteSession::recover`].
fn reset_staging(dir: &Path) -> Result<()> {
    let staging = dir.join(STAGING);
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    fs::create_dir_all(&staging)?;
    Ok(())
}

/// A handle on a [`WriteSession`]'s staging directory, for encoders that run apart from the
/// session (the [`crate::StreamWriter`]'s workers). Clone it freely; every [`StagedBlock`] it creates
/// gets a fresh name.
#[derive(Clone, Debug)]
pub struct BlockStaging {
    dir: PathBuf,
    seq: Arc<AtomicU64>,
}

impl BlockStaging {
    /// The staging directory (inside the session directory, so on the same filesystem).
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Create an empty staged file to write one encoded block into.
    pub fn create(&self) -> Result<StagedBlock> {
        let n = self.seq.fetch_add(1, Ordering::Relaxed);
        let guard = RemoveOnDrop::new(self.dir.join(format!("block-{n:010}.staged")));
        let out = BufWriter::new(File::create(guard.path())?);
        Ok(StagedBlock { out, len: 0, guard })
    }
}

/// One encoded block in a scratch file in the session's staging directory: write the block's bytes
/// into it (it is an [`std::io::Write`], buffered), then commit it with
/// [`WriteSession::append_staged`]. The file is deleted when the block is committed, when the
/// commit fails, and when the `StagedBlock` is dropped uncommitted (an encode error, a panic, a
/// pipeline shut down early), so a staged file never outlives its block.
///
/// Write the block **through this value**: [`Self::len`] counts what passes through its
/// [`std::io::Write`] impl, and that count is what a [`crate::StreamWriter`] reports as staged bytes.
/// [`Self::path`] is for diagnostics, not for writing the file behind its back.
#[derive(Debug)]
pub struct StagedBlock {
    out: BufWriter<File>,
    len: u64,
    guard: RemoveOnDrop,
}

impl StagedBlock {
    /// Bytes written so far: the encoded block's size once its encoder is done.
    pub fn len(&self) -> u64 {
        self.len
    }

    /// Whether nothing has been written yet.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The scratch file's path (for diagnostics; the file goes away with the block).
    pub fn path(&self) -> &Path {
        self.guard.path()
    }

    /// Flush the buffer, then reopen the file for reading. The reader keeps the delete-on-drop guard.
    fn into_reader(self) -> Result<StagedReader> {
        let StagedBlock { out, guard, .. } = self;
        out.into_inner().map_err(|e| e.into_error())?;
        let file = BufReader::with_capacity(APPEND_BUF, File::open(guard.path())?);
        Ok(StagedReader {
            file,
            _guard: guard,
        })
    }
}

impl Write for StagedBlock {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.out.write(buf)?;
        self.len += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.out.flush()
    }
}

/// A [`StagedBlock`] reopened for its commit; deletes the file when dropped.
struct StagedReader {
    file: BufReader<File>,
    _guard: RemoveOnDrop,
}

impl Read for StagedReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.file.read(buf)
    }
}

/// Write `bytes` to `path` **durably + atomically**: write a temp file, fsync it, atomically rename it
/// over `path`, then fsync the directory so the rename itself is on disk. A crash therefore never leaves
/// a torn or lost header — the metadata-first durability guarantee (the reader sees either the old header
/// or the complete new one, never a partial write).
fn write_durable(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("json.tmp");
    {
        let mut f = File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    if let Some(dir) = path.parent() {
        // fsync the directory entry so the rename survives a crash (Linux durability).
        File::open(dir)?.sync_all()?;
    }
    Ok(())
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
        reset_staging(dir)?;
        let header = Header {
            product: product.into(),
            name: name.into(),
            description: description.into(),
            timestamp: timestamp.into(),
            study: None,
            metadata: BTreeMap::new(),
            extra: BTreeMap::new(),
            sources: Vec::new(),
            producer: None,
            generation: None,
        };
        write_durable(&dir.join(HEADER), &serde_json::to_vec_pretty(&header)?)?;
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
            staged_seq: Arc::new(AtomicU64::new(0)),
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

    /// Declare a provenance edge (DAG source) on the session — flows through into the sealed
    /// manifest's `sources` list, so a streaming ingest can record `ingested_from`/`derived_from`
    /// before any blocks are committed (no re-seal). Order is preserved (manifest order matters for
    /// the seal hash). Persisted to the header so it survives crash recovery.
    pub fn add_source(&mut self, source: tessera_core::provenance::Source) -> Result<&mut Self> {
        self.header.sources.push(source);
        self.persist_header()?;
        Ok(self)
    }

    /// Declare the producing tool/build (ADR-0058 §1) on the session — mirrors
    /// [`ProductBuilder::with_producer`], overriding the default `tessera` stamp so an external
    /// DAQ/SIM/sorter records its own identity. Declared before any block commits, so it rides the
    /// sealed `manifest_hash`. Persisted to the header so it survives crash recovery.
    pub fn with_producer(&mut self, producer: tessera_core::Producer) -> Result<&mut Self> {
        self.header.producer = Some(producer);
        self.persist_header()?;
        Ok(self)
    }

    /// Attach the generation record (ADR-0058 §2) — *how* this product was made, as a generic bag —
    /// mirroring [`ProductBuilder::with_generation`]. Declared before any block commits, so the
    /// recipe rides the sealed `manifest_hash` and is tamper-evident. Persisted to the header so it
    /// survives crash recovery.
    pub fn with_generation(&mut self, generation: tessera_core::Generation) -> Result<&mut Self> {
        self.header.generation = Some(generation);
        self.persist_header()?;
        Ok(self)
    }

    fn persist_header(&self) -> Result<()> {
        write_durable(
            &self.dir.join(HEADER),
            &serde_json::to_vec_pretty(&self.header)?,
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
        self.commit(block_ref, digest, payload.len() as u64)
    }

    /// Durably append one encoded block whose bytes come from `payload`, in bounded memory: the
    /// streaming peer of [`Self::append_block`], for a block too large to hold in RAM.
    ///
    /// The bytes are copied through a fixed 256 KiB buffer into a partial fragment in the staging
    /// directory and **hashed while they are written** (ADR-0002 first-moment integrity), so the
    /// digest is of exactly the bytes that land on disk. If `block_ref.digest` is set it must match
    /// them (else [`Error::Integrity`] with `what = "block_payload"`); if it is `None` the session
    /// fills it in. Then the fragment is fsync'd, renamed into `blocks/` (and the directory fsync'd),
    /// and committed exactly like [`Self::append_block`]: same fragment, journal line and Merkle leaf
    /// for the same bytes. Returns the committed [`BlockRef`], digest included.
    ///
    /// **Failure leaves nothing behind.** A read error, a write error or a digest mismatch deletes
    /// the partial fragment and returns before the journal line; a failure after the rename (the
    /// directory fsync or the journal write) deletes the renamed fragment. Either way there is no
    /// fragment, no Merkle leaf and no complete journal line for the block (a journal line torn by
    /// a failed write is dropped by [`Self::recover`], like any torn tail).
    pub fn append_block_from(
        &mut self,
        mut block_ref: BlockRef,
        mut payload: impl Read,
    ) -> Result<BlockRef> {
        let part = RemoveOnDrop::new(self.dir.join(STAGING).join(APPEND_PART));
        let mut hasher = StreamHasher::new();
        let mut len = 0u64;
        {
            let mut frag = File::create(part.path())?;
            let mut buf = vec![0u8; APPEND_BUF];
            loop {
                let n = match payload.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(e.into()),
                };
                hasher.update(&buf[..n]);
                frag.write_all(&buf[..n])?;
                len += n as u64;
            }
            frag.sync_all()?;
        }
        let digest = hasher.finalize();
        match &block_ref.digest {
            Some(expected) if *expected != digest => {
                return Err(Error::Integrity {
                    what: "block_payload",
                    expected: expected.clone(),
                    actual: digest,
                });
            }
            Some(_) => {}
            None => block_ref.digest = Some(digest.clone()),
        }
        let blocks = self.dir.join(BLOCKS);
        let fragment = RemoveOnDrop::new(blocks.join(&block_ref.name));
        fs::rename(part.path(), fragment.path())?;
        part.disarm();
        // Until the journal line is written the fragment is not committed: a failure from here on
        // removes it again, so no fragment is left that the journal doesn't name.
        File::open(&blocks)?.sync_all()?; // the rename itself is durable before the journal names it
        self.commit(block_ref.clone(), digest, len)?;
        fragment.disarm();
        Ok(block_ref)
    }

    /// Commit a [`StagedBlock`] through [`Self::append_block_from`], then delete its file. The file is
    /// deleted on failure too.
    pub fn append_staged(&mut self, block_ref: BlockRef, staged: StagedBlock) -> Result<BlockRef> {
        let reader = staged.into_reader()?;
        self.append_block_from(block_ref, reader)
    }

    /// A handle on this session's staging directory, for encoders that write a [`StagedBlock`]
    /// on another thread (the [`crate::StreamWriter`]'s workers).
    pub fn staging(&self) -> BlockStaging {
        BlockStaging {
            dir: self.dir.join(STAGING),
            seq: Arc::clone(&self.staged_seq),
        }
    }

    /// Steps 2 and 3 of an append, after the fragment named `block_ref.name` is durable: the journal
    /// line (the atomic commit point), the running Merkle root, and the write trace.
    fn commit(&mut self, block_ref: BlockRef, digest: String, bytes: u64) -> Result<()> {
        // 2. journal line → durable (this is the commit watermark advance)
        writeln!(self.journal, "{}", serde_json::to_string(&block_ref)?)?;
        self.journal.sync_all()?;
        // 3. running integrity root
        self.merkle.push(&digest);
        // Write-path observability (SSoT): every durably-committed block — batch, stream, or ingest —
        // flows through here, so one structured event gives devs/users full I/O info on write (name,
        // kind, bytes, content digest, and the new integrity watermark). Zero-cost when no subscriber.
        let kind = format!("{:?}", block_ref.kind);
        tracing::info!(
            target: "tessera::write",
            name = %block_ref.name,
            kind = %kind,
            bytes = bytes,
            digest = %digest,
            watermark = self.merkle.watermark(),
            "committed block"
        );
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
        // A header this build cannot parse in full is refused rather than silently narrowed (see
        // [`Header`]); say what to do about it, since "unknown field" alone reads like corruption.
        let header: Header = serde_json::from_slice(&fs::read(dir.join(HEADER))?).map_err(|e| {
            Error::Invalid(format!(
                "stage dir {} cannot be resumed by this build: {e}. It was most likely written by a \
                 NEWER tessera whose header carries fields this one would drop — including sealed \
                 provenance — so resuming would silently change the product. Re-run the ingest with \
                 the newer build, or start a fresh stage dir with this one.",
                dir.display()
            ))
        })?;
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
        reset_staging(dir)?;
        Ok(WriteSession {
            dir: dir.to_path_buf(),
            header,
            journal,
            blocks,
            merkle,
            staged_seq: Arc::new(AtomicU64::new(0)),
        })
    }

    /// Compact + seal: build the manifest through [`ProductBuilder`] (identical to a batch writer),
    /// verify the running root matches the sealed `content_hash`, and pack the fragments into a
    /// sealed `.tsra` at `out`. Returns the sealed manifest.
    ///
    /// **Constant-memory.** Each fragment is streamed straight off disk into the zip via
    /// [`pack_streaming`] (one buffered `io::copy` per block), so the whole engine (encode, commit,
    /// and seal) never has to fit a product in RAM. This is the must-fix that lets the multi-block
    /// listmode path actually be bounded-memory end-to-end — otherwise seal would re-materialise
    /// every block in a `Vec<BlockPayload>` and defeat the rest of the pipeline.
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
        for s in &self.header.sources {
            b.add_source(s.clone());
        }
        // ADR-0058 §1/§2 — sealed provenance, applied through the same `ProductBuilder` the batch
        // writer uses, so a streamed product with a recipe is byte-identical to a batch-sealed one.
        if let Some(producer) = &self.header.producer {
            b.with_producer(producer.clone());
        }
        if let Some(generation) = &self.header.generation {
            b.with_generation(generation.clone());
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
        // Stream each fragment from disk → zip entry via pack_streaming (no whole-product Vec). The
        // resulting archive is byte-identical to pack(&sealed, &<vec of read fragments>, out) over
        // the same inputs (proven by pack_streaming_equals_pack), so this swap is determinism-safe.
        let frag_paths: Vec<PathBuf> = self
            .blocks
            .iter()
            .map(|r| self.dir.join(BLOCKS).join(&r.name))
            .collect();
        let sources: Vec<(String, &Path)> = self
            .blocks
            .iter()
            .zip(frag_paths.iter())
            .map(|(r, p)| (r.name.clone(), p.as_path()))
            .collect();
        pack_streaming(&sealed, &sources, out)?;
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

    /// #416 / ADR-0058 §1/§2: a recipe + producer declared on the session ride the seal, survive a
    /// crash-recovery round-trip (they live on the durable header, not in RAM), and produce a
    /// manifest byte-identical to the batch builder's — so a streamed product with provenance is
    /// indistinguishable from a batch-sealed one.
    #[test]
    fn declared_producer_and_generation_ride_the_streamed_seal() {
        let dir = tempfile::tempdir().unwrap();
        let stage = dir.path().join("stage");
        let (r1, p1) = block("a", 0);
        let producer = tessera_core::Producer::new("ge-listmode-daq", "3.2");
        let generation = tessera_core::Generation::default()
            .with("energy_window_keV", serde_json::json!("425-650"))
            .with_config_ref("blake3:abc");
        {
            let mut ws = WriteSession::create(&stage, "recon", "p", "d", TS).unwrap();
            ws.with_producer(producer.clone()).unwrap();
            ws.with_generation(generation.clone()).unwrap();
            ws.append_block(r1.clone(), &p1).unwrap();
        }
        // Reopen from disk — the header is the only thing that carried them across the "crash".
        let ws = WriteSession::recover(&stage).unwrap();
        let out = dir.path().join("recovered.tsra");
        let sealed = ws.seal(&out).unwrap();

        assert_eq!(sealed.generation.as_ref(), Some(&generation));
        assert_eq!(
            sealed.producer.as_ref(),
            Some(&tessera_core::ProducerRef::Structured(producer.clone()))
        );

        // Identical to the batch-built equivalent, seal hash included.
        let mut bb = ProductBuilder::new("recon", "p", "d", TS);
        bb.add_block_ref(r1);
        bb.with_producer(producer);
        bb.with_generation(generation);
        let batch = bb.seal().unwrap();
        assert_eq!(sealed.manifest_hash, batch.manifest_hash);

        // And it survives the container round-trip.
        let reread = Reader::open(&out).unwrap().manifest().clone();
        assert_eq!(reread.generation, sealed.generation);
        assert_eq!(reread.producer, sealed.producer);
    }

    /// #462: an older binary resuming a stage dir written by NEWER code must fail loudly rather
    /// than silently drop the fields it does not understand. Those fields are sealed provenance
    /// (`producer`/`generation`), so dropping them yields a different, quieter seal — the #416
    /// failure mode again. Simulated by injecting a field no current version knows.
    #[test]
    fn recover_refuses_a_header_from_a_newer_writer() {
        let dir = tempfile::tempdir().unwrap();
        let stage = dir.path().join("stage");
        {
            let mut ws = WriteSession::create(&stage, "recon", "p", "d", TS).unwrap();
            ws.with_producer(tessera_core::Producer::new("daq", "1.0"))
                .unwrap();
        }
        // A field from a hypothetical future writer, alongside the ones we do know.
        let hpath = stage.join(HEADER);
        let mut v: serde_json::Value = serde_json::from_slice(&fs::read(&hpath).unwrap()).unwrap();
        v.as_object_mut()
            .unwrap()
            .insert("attestation".into(), serde_json::json!({"alg": "future"}));
        fs::write(&hpath, serde_json::to_vec_pretty(&v).unwrap()).unwrap();

        // `expect_err` would need `WriteSession: Debug`, which it deliberately is not.
        let msg = match WriteSession::recover(&stage) {
            Ok(_) => panic!("resume must refuse a header written by a newer writer"),
            Err(e) => e.to_string(),
        };
        assert!(
            msg.contains("attestation"),
            "the error must name the unknown field, got: {msg}"
        );
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
    fn durable_header_persists_metadata_atomically_and_recovers() {
        let dir = tempfile::tempdir().unwrap();
        let stage = dir.path().join("stage");
        let mut ws = WriteSession::create(&stage, "recon", "DP", "d", TS).unwrap();
        ws.with_study("study-7").unwrap();
        ws.with_field(
            "modality",
            serde_json::json!({"_vocabulary": "DICOM", "_code": "CT"}),
        )
        .unwrap();
        ws.with_extra("operator", serde_json::json!("ct-tech-1"))
            .unwrap();

        // the durable write leaves no torn temp file behind, and the header is on disk.
        assert!(stage.join(HEADER).exists());
        assert!(
            !stage.join("header.json.tmp").exists(),
            "atomic write must not leave a temp"
        );

        // recover reads the persisted header (metadata survived the fsync'd writes) and seals it through.
        let recovered = WriteSession::recover(&stage).unwrap();
        let out = dir.path().join("r.tsra");
        let sealed = recovered.seal(&out).unwrap();
        assert_eq!(sealed.study.as_deref(), Some("study-7"));
        assert_eq!(
            sealed.metadata.get("modality"),
            Some(&serde_json::json!({"_vocabulary": "DICOM", "_code": "CT"}))
        );
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

    #[test]
    fn seal_streams_fragments_without_materializing_into_one_vec() {
        // The must-fix: seal MUST stream each fragment file straight into the zip (pack_streaming),
        // NOT fs::read every fragment into a single Vec<BlockPayload>. We can't unit-test peak RSS
        // portably, so we assert two stronger contracts that together pin the implementation:
        //
        //   1. The sealed .tsra is **byte-identical** to packing the same fragments through pack()
        //      after reading them all into RAM — i.e. pack_streaming and pack agree (the SSoT entry
        //      options + order match). If seal regressed to a different writer, this would diverge.
        //   2. Sealing many large fragments succeeds end-to-end (a smoke that the streaming code
        //      path actually runs over a multi-block product).
        let dir = tempfile::tempdir().unwrap();
        let stage = dir.path().join("stage");
        let mut ws = WriteSession::create(&stage, "recon", "p", "d", TS).unwrap();
        // 4 blocks × ~512 KiB each — large enough that holding them all in RAM is wasteful and the
        // buffered copy path crosses multiple 256 KiB BufReader refills.
        let mut blocks: Vec<(BlockRef, Vec<u8>)> = Vec::new();
        for i in 0..4u8 {
            let mut spec = ArraySpec::new(vec![16, 16, 16], "int16");
            spec.codec = "pcodec".into();
            let data = ArrayData::I16(
                (0..(16 * 16 * 16))
                    .map(|k| (k as i16).wrapping_mul(7).wrapping_add(i as i16))
                    .collect(),
            );
            let (r, p) = array::array_block(&format!("b{i:02}"), &spec, &data).unwrap();
            ws.append_block(r.clone(), &p.bytes).unwrap();
            blocks.push((r, p.bytes));
        }
        // Capture the manifest a streaming seal will produce (peek without consuming the session).
        let streamed_out = dir.path().join("streamed.tsra");
        let sealed = ws.seal(&streamed_out).unwrap();
        let streamed_bytes = std::fs::read(&streamed_out).unwrap();

        // Equivalent RAM-side path: pack(...) with the same manifest + payloads read from RAM.
        let ram_out = dir.path().join("ram.tsra");
        let payloads: Vec<crate::BlockPayload> = blocks
            .iter()
            .map(|(r, b)| crate::BlockPayload::new(r.name.clone(), b.clone()))
            .collect();
        crate::pack(&sealed, &payloads, &ram_out).unwrap();
        let ram_bytes = std::fs::read(&ram_out).unwrap();

        assert_eq!(
            streamed_bytes, ram_bytes,
            "seal must stream fragments via pack_streaming (byte-identical to pack)"
        );
        // And the streamed product is openable + verifies its blocks (catches a silent corruption).
        let mut rdr = Reader::open(&streamed_out).unwrap();
        for nm in rdr.block_names() {
            rdr.read_block(&nm).unwrap();
        }
    }

    /// A reader that yields `data` in uneven pieces, then fails once it has yielded `fail_after`
    /// bytes (never, if `None`).
    struct Flaky {
        data: Vec<u8>,
        pos: usize,
        fail_after: Option<usize>,
    }

    impl Read for Flaky {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.fail_after.is_some_and(|n| self.pos >= n) {
                return Err(std::io::Error::other("source went away"));
            }
            let n = buf.len().min(7919).min(self.data.len() - self.pos);
            buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    /// A payload larger than the 256 KiB copy buffer (an incompressible 1 MiB blob), so the
    /// streamed append takes several reads.
    fn big_block(name: &str) -> (BlockRef, Vec<u8>) {
        let mut x = 0x9e37_79b9_7f4a_7c15u64;
        let bytes: Vec<u8> = (0..1 << 20)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                x as u8
            })
            .collect();
        let (r, payload) = crate::blob::blob_block(name, "synthetic", None, bytes).unwrap();
        assert!(
            payload.bytes.len() > 2 * APPEND_BUF,
            "{}",
            payload.bytes.len()
        );
        (r, payload.bytes)
    }

    /// Everything a session leaves on disk that a product or a recovery depends on.
    fn session_files(stage: &Path) -> (String, Vec<(String, Vec<u8>)>, usize) {
        let journal = fs::read_to_string(stage.join(JOURNAL)).unwrap();
        let mut blocks: Vec<(String, Vec<u8>)> = fs::read_dir(stage.join(BLOCKS))
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                (
                    e.file_name().into_string().unwrap(),
                    fs::read(e.path()).unwrap(),
                )
            })
            .collect();
        blocks.sort();
        let staged = fs::read_dir(stage.join(STAGING)).unwrap().count();
        (journal, blocks, staged)
    }

    /// #677: the reader-based append commits exactly what the slice append commits — the same
    /// fragment, journal line, Merkle root and sealed container bytes — whether the caller supplies
    /// the digest (verified) or leaves it to the session (filled in while writing).
    #[test]
    fn append_block_from_commits_the_same_block_as_append_block() {
        let dir = tempfile::tempdir().unwrap();
        let (r1, p1) = block("a", 0);
        let (r2, p2) = big_block("b");
        let seal = |tag: &str, streamed: bool, with_digest: bool| {
            let stage = dir.path().join(tag);
            let mut ws = WriteSession::create(&stage, "recon", "p", "d", TS).unwrap();
            for (r, p) in [(&r1, &p1), (&r2, &p2)] {
                if streamed {
                    let mut r = r.clone();
                    if !with_digest {
                        r.digest = None;
                    }
                    let reader = Flaky {
                        data: p.clone(),
                        pos: 0,
                        fail_after: None,
                    };
                    let committed = ws.append_block_from(r, reader).unwrap();
                    assert_eq!(committed.digest, Some(tessera_core::hash::digest(p)));
                } else {
                    ws.append_block(r.clone(), p).unwrap();
                }
            }
            let root = ws.running_root();
            let files = session_files(&stage);
            let out = dir.path().join(format!("{tag}.tsra"));
            let sealed = ws.seal(&out).unwrap();
            (sealed, root, files, fs::read(&out).unwrap())
        };
        let reference = seal("slice", false, true);
        for (tag, with_digest) in [("reader-verified", true), ("reader-filled", false)] {
            let (sealed, root, files, bytes) = seal(tag, true, with_digest);
            assert_eq!(sealed.manifest_hash, reference.0.manifest_hash, "{tag}");
            assert_eq!(sealed.content_hash, reference.0.content_hash, "{tag}");
            assert_eq!(root, reference.1, "{tag}: running root");
            assert!(files == reference.2, "{tag}: journal/fragments differ");
            assert_eq!(files.2, 0, "{tag}: the append left a partial fragment");
            assert!(bytes == reference.3, "{tag}: sealed container bytes differ");
        }
    }

    /// #677: an append whose reader fails mid-stream, or whose bytes don't match the declared
    /// digest, leaves the session exactly as it was — no partial fragment in staging, no fragment
    /// in `blocks/`, no journal line, no Merkle leaf — and the session keeps working: a retry of the
    /// same block seals the same product as a session that never failed.
    #[test]
    fn failed_append_block_from_leaves_no_staged_file_and_no_half_committed_block() {
        let dir = tempfile::tempdir().unwrap();
        let (r1, p1) = block("a", 0);
        let (r2, p2) = big_block("b");
        let stage = dir.path().join("stage");
        let mut ws = WriteSession::create(&stage, "recon", "p", "d", TS).unwrap();
        ws.append_block(r1.clone(), &p1).unwrap();
        let before = session_files(&stage);
        let root = ws.running_root();

        // Mid-stream read error, after more than one copy buffer reached the partial fragment.
        let flaky = Flaky {
            data: p2.clone(),
            pos: 0,
            fail_after: Some(APPEND_BUF + 1),
        };
        let err = ws.append_block_from(r2.clone(), flaky).unwrap_err();
        assert!(err.to_string().contains("source went away"), "{err}");
        assert!(
            session_files(&stage) == before,
            "a failed append left a trace"
        );
        assert_eq!((ws.watermark(), ws.running_root()), (1, root.clone()));

        // Bytes that don't match the declared digest.
        let mut corrupt = p2.clone();
        corrupt[1000] ^= 1;
        let tampered = Flaky {
            data: corrupt,
            pos: 0,
            fail_after: None,
        };
        let err = ws.append_block_from(r2.clone(), tampered).unwrap_err();
        assert!(
            matches!(
                err,
                Error::Integrity {
                    what: "block_payload",
                    ..
                }
            ),
            "{err}"
        );
        assert!(
            session_files(&stage) == before,
            "a rejected append left a trace"
        );
        assert_eq!((ws.watermark(), ws.running_root()), (1, root));

        // The retry commits; the product is the one a clean session seals.
        let staging = ws.staging();
        let mut staged = staging.create().unwrap();
        staged.write_all(&p2).unwrap();
        let staged_path = staged.path().to_path_buf();
        ws.append_staged(r2.clone(), staged).unwrap();
        assert!(
            !staged_path.exists(),
            "a committed staged file must be deleted"
        );
        let sealed = ws.seal(&dir.path().join("retried.tsra")).unwrap();
        let mut bb = ProductBuilder::new("recon", "p", "d", TS);
        bb.add_block_ref(r1);
        bb.add_block_ref(r2);
        assert_eq!(sealed.manifest_hash, bb.seal().unwrap().manifest_hash);
    }

    /// A staged file is deleted when its block is dropped uncommitted, and `recover` clears the
    /// staged files a crash left behind (they were never committed, so they are scratch).
    #[test]
    fn staged_files_never_outlive_their_block() {
        let dir = tempfile::tempdir().unwrap();
        let stage = dir.path().join("stage");
        let ws = WriteSession::create(&stage, "recon", "p", "d", TS).unwrap();
        let staging = ws.staging();
        let mut a = staging.create().unwrap();
        let b = staging.create().unwrap();
        assert_ne!(a.path(), b.path(), "every staged file gets a fresh name");
        a.write_all(b"partial").unwrap();
        assert_eq!(a.len(), 7);
        let a_path = a.path().to_path_buf();
        drop(a);
        assert!(
            !a_path.exists(),
            "an uncommitted staged file must be deleted on drop"
        );

        // A crash: the session and the staged block vanish without running their drops.
        std::mem::forget(b);
        drop(ws);
        assert_eq!(session_files(&stage).2, 1);
        let ws = WriteSession::recover(&stage).unwrap();
        assert_eq!(
            session_files(&stage).2,
            0,
            "recover must clear stale staged files"
        );
        assert_eq!(ws.watermark(), 0);
    }

    #[test]
    fn append_block_emits_a_structured_write_trace() {
        let dir = tempfile::tempdir().unwrap();
        let logged = crate::test_trace::capture(tracing::Level::INFO, || {
            let mut ws =
                WriteSession::create(&dir.path().join("stage"), "recon", "p", "d", TS).unwrap();
            let (r, p) = block("volume", 0);
            ws.append_block(r, &p).unwrap();
        });
        // the write-path event fired with full I/O info: message + every structured field.
        assert!(
            logged.contains("committed block"),
            "missing event message: {logged}"
        );
        assert!(
            logged.contains("tessera::write"),
            "missing the write target: {logged}"
        );
        assert!(
            logged.contains("name=volume"),
            "missing block name: {logged}"
        );
        assert!(
            logged.contains("kind=Array"),
            "missing block kind: {logged}"
        );
        assert!(
            logged.contains("watermark=1"),
            "missing watermark: {logged}"
        );
        assert!(
            logged.contains("digest=blake3:"),
            "missing content digest: {logged}"
        );
        assert!(logged.contains("bytes="), "missing byte count: {logged}");
        // sanity: a writer with no subscriber installed still works (zero-cost path) — separate session.
        let mut ws2 =
            WriteSession::create(&dir.path().join("stage2"), "recon", "p", "d", TS).unwrap();
        let (r2, p2) = block("v2", 1);
        ws2.append_block(r2, &p2).unwrap();
    }
}
