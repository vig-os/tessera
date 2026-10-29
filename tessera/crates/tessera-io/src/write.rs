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
//! slice. For a block too large for that, an encoder writes it into a [`StagedBlock`], a file in the
//! session's staging directory ([`WriteSession::staging`]) that hashes the bytes as they are written
//! (ADR-0002 first-moment integrity), and [`WriteSession::append_staged`] renames that file into
//! place as the fragment, after checking its bytes against that digest; [`WriteSession::append_block_from`]
//! does the same for bytes from a reader. A committed staged file becomes the fragment; an uncommitted one is deleted on error and on drop, and
//! [`WriteSession::recover`] clears any a crash left behind, so staging never leaks. Every append
//! commits the same fragment, journal line and Merkle leaf for the same bytes.
//!
//! **A failed journal write poisons the session.** Once a commit has begun writing its journal line,
//! a failure leaves the journal's state unknown (see [`WriteSession::append_block`]), so the session
//! refuses every later append and the seal; [`WriteSession::recover`] is the way on.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tessera_core::block::{BlockName, BlockRef};
use tessera_core::hash::{MerkleAccumulator, StreamHasher};
use tessera_core::manifest::Manifest;
use tessera_core::{Error, ProductBuilder, Result};

use crate::pack_streaming;

const HEADER: &str = "header.json";
const JOURNAL: &str = "journal.jsonl";
const BLOCKS: &str = "blocks";
/// Scratch files of blocks that are encoded but not yet committed ([`StagedBlock`]). Nothing here
/// is ever part of the product: a commit renames its file into `blocks/`.
const STAGING: &str = "staging";

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
    /// Why the session refuses to go on, once a commit failed after its journal write began.
    poisoned: Option<String>,
    #[cfg(test)]
    faults: Faults,
}

/// Test-only fault injection: seams on the steps whose failure a test must provoke (not timing).
#[cfg(test)]
#[derive(Default)]
struct Faults {
    /// Fail the next journal fsync, after the journal line was written.
    journal_sync: bool,
    /// Treat the next staged-file rename as crossing filesystems (`EXDEV`).
    cross_device: bool,
}

/// Deletes a file when dropped, unless [`Self::disarm`]ed, and then the directories created for it
/// (a hierarchical block name's subdirectories), deepest first, if they are empty. Best effort: a
/// drop can't report an error, and a file that is already gone is the goal.
#[derive(Debug)]
struct RemoveOnDrop {
    path: Option<PathBuf>,
    created_dirs: Vec<PathBuf>,
}

impl RemoveOnDrop {
    fn new(path: PathBuf) -> Self {
        RemoveOnDrop {
            path: Some(path),
            created_dirs: Vec::new(),
        }
    }

    fn path(&self) -> &Path {
        self.path.as_deref().expect("armed")
    }

    fn disarm(mut self) {
        self.path = None;
    }
}

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            let _ = fs::remove_file(path);
            for dir in &self.created_dirs {
                // `remove_dir` refuses a non-empty directory, so a sibling's fragment is never touched.
                let _ = fs::remove_dir(dir);
            }
        }
    }
}

/// The guarded fragment path of block `name` in the session at `dir`: `blocks/<name>`, built from
/// plain path components only ([`BlockName::path_under`], ADR-0063), so no name reaches outside
/// `blocks/` (#685). The parent directories of a hierarchical name (`volume/0`) are created, each
/// fsync'd into its parent so the path survives a crash, and are removed again with the fragment if
/// the append fails.
fn fragment_guard(dir: &Path, name: &BlockName) -> Result<RemoveOnDrop> {
    let blocks = dir.join(BLOCKS);
    let path = name.path_under(&blocks)?;
    let mut created_dirs = Vec::new();
    let mut d = path.parent();
    while let Some(p) = d.filter(|p| *p != blocks && !p.exists()) {
        created_dirs.push(p.to_path_buf());
        d = p.parent();
    }
    let mut guard = RemoveOnDrop::new(path);
    // Deepest first, as the drop removes them; creation runs the other way.
    for created in created_dirs.iter().rev() {
        fs::create_dir(created)?;
        guard.created_dirs.insert(0, created.clone());
        File::open(created.parent().expect("under blocks/"))?.sync_all()?;
    }
    Ok(guard)
}

/// Empty the staging directory of the session at `dir` (creating it if absent). Staged files are
/// never committed in place (a commit renames its file into `blocks/`), so whatever a crashed or abandoned writer left
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
        Ok(StagedBlock {
            out,
            hasher: Box::default(),
            len: 0,
            guard,
        })
    }
}

/// One encoded block in a scratch file in the session's staging directory: write the block's bytes
/// into it (it is an [`std::io::Write`], buffered), then commit it with
/// [`WriteSession::append_staged`], which renames the file into place as the block's fragment. The
/// file is deleted when the commit fails, and when the `StagedBlock` is dropped uncommitted (an
/// encode error, a panic, a pipeline shut down early), so a staged file never outlives its block.
///
/// **The digest is born with the bytes** (ADR-0002 first-moment integrity): every byte is hashed as
/// it passes through the [`std::io::Write`] impl, and the commit checks the file it moves into
/// `blocks/` against that digest, so a staged file changed on disk between the encode and the
/// commit is refused. Write the block **through this value**, then: [`Self::len`] and the digest
/// cover only what passes through it (that count is also what a [`crate::StreamWriter`] reports as
/// staged bytes), and [`Self::path`] is for diagnostics, not for writing the file behind its back.
pub struct StagedBlock {
    out: BufWriter<File>,
    /// Boxed: a blake3 hasher is ~2 KB, and a `StagedBlock` travels by value through the pipeline.
    hasher: Box<StreamHasher>,
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
}

impl std::fmt::Debug for StagedBlock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StagedBlock")
            .field("path", &self.path())
            .field("len", &self.len)
            .finish_non_exhaustive()
    }
}

impl Write for StagedBlock {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.out.write(buf)?;
        self.hasher.update(&buf[..n]);
        self.len += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.out.flush()
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
            poisoned: None,
            #[cfg(test)]
            faults: Faults::default(),
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
    ///
    /// **Failure.** A failure before the journal line leaves no trace: the fragment is deleted
    /// again. A failure once the journal write has begun (the write itself, or its fsync)
    /// **poisons** the session: this and every later append, and [`Self::seal`], return an error
    /// that says to [`Self::recover`]. That is the only safe way on, because the journal's state is
    /// then unknown — the write may have left a torn line, or a complete line that its failed fsync
    /// did not make durable, or one it did — and a later append written behind such a line would be
    /// lost: `recover` stops at the first line whose fragment is missing, and the failed block's
    /// fragment is deleted. Truncating the journal back to its last good length instead would rest
    /// on the very fsync that just failed (after an fsync error the kernel may have dropped the dirty
    /// pages and cleared the error, so a later fsync can succeed without the data on disk), so the
    /// session doesn't try: `recover` rereads the journal from disk and keeps exactly the blocks
    /// before the failed one (or, if the deletion of the failed block's fragment doesn't reach the
    /// disk — the process dies first, or the power fails before the directory is written — possibly
    /// that block too, as for a crash during the append).
    pub fn append_block(&mut self, block_ref: BlockRef, payload: &[u8]) -> Result<()> {
        self.admit(&block_ref)?;
        let digest = block_ref
            .digest
            .clone()
            .ok_or_else(|| Error::MissingDigest(block_ref.name.to_string()))?;
        // 1. fragment → durable (deleted again unless the commit succeeds)
        let fragment = fragment_guard(&self.dir, &block_ref.name)?;
        {
            let mut w = File::create(fragment.path())?;
            w.write_all(payload)?;
            w.sync_all()?;
        }
        self.commit(block_ref, digest, payload.len() as u64, fragment)
    }

    /// Durably append one encoded block whose bytes come from `payload`, in bounded memory: the
    /// streaming peer of [`Self::append_block`], for a block too large to hold in RAM. The bytes are
    /// copied into a [`StagedBlock`] (hashing them as they are written) and committed with
    /// [`Self::append_staged`]; see there. Returns the committed [`BlockRef`], digest included.
    pub fn append_block_from(
        &mut self,
        block_ref: BlockRef,
        mut payload: impl Read,
    ) -> Result<BlockRef> {
        self.admit(&block_ref)?;
        let mut staged = self.staging().create()?;
        std::io::copy(&mut payload, &mut staged)?;
        self.append_staged(block_ref, staged)
    }

    /// Commit a [`StagedBlock`] as the block `block_ref` without copying it: the staged file
    /// **becomes** the fragment. Returns the committed [`BlockRef`], digest included.
    ///
    /// 1. The digest the block was born with (hashed as it was written) is the block's: if
    ///    `block_ref.digest` is set it must match (else [`Error::Integrity`] with
    ///    `what = "block_payload"`), if it is `None` it is filled in.
    /// 2. The file is fsync'd and renamed into `blocks/`, and the directory fsync'd. The staging
    ///    directory is inside the session directory, so the rename stays on one filesystem; if
    ///    `staging/` is on another filesystem anyway (a mount point or a symlink), the rename fails
    ///    with `CrossesDevices` and the file is copied instead.
    /// 3. **The bytes now in `blocks/` are checked against the born digest** (one read of the
    ///    block, 64 KiB at a time), so scratch corruption between the encode and the commit, or a
    ///    bad copy, is refused ([`Error::Integrity`] with `what = "staged_block"`) rather than sealed
    ///    under the digest of the corrupted bytes.
    /// 4. The journal line and Merkle leaf, as for [`Self::append_block`] (including its poisoning).
    ///
    /// **Failure before the journal write leaves no trace**: no staged file, no fragment, no journal
    /// line, no Merkle leaf, and the session goes on.
    pub fn append_staged(
        &mut self,
        mut block_ref: BlockRef,
        staged: StagedBlock,
    ) -> Result<BlockRef> {
        self.admit(&block_ref)?;
        let StagedBlock {
            out,
            hasher,
            len,
            guard,
        } = staged;
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
        out.into_inner().map_err(|e| e.into_error())?.sync_all()?;
        let fragment = self.place(guard, &block_ref.name)?;
        let on_disk = tessera_core::hash::digest_reader(File::open(fragment.path())?)?;
        if on_disk != digest {
            return Err(Error::Integrity {
                what: "staged_block",
                expected: digest,
                actual: on_disk,
            });
        }
        self.commit(block_ref.clone(), digest, len, fragment)?;
        Ok(block_ref)
    }

    /// A handle on this session's staging directory, for encoders that write a [`StagedBlock`]
    /// on another thread (the [`crate::StreamWriter`]'s workers).
    pub fn staging(&self) -> BlockStaging {
        BlockStaging {
            dir: self.dir.join(STAGING),
            seq: Arc::clone(&self.staged_seq),
        }
    }

    /// Refuse an append the session can't take: any, once poisoned; one whose name is already
    /// committed (its fragment would be overwritten, and deleted again if the append failed); and
    /// one whose name and a committed name break the leaf rule (ADR-0063: one the parent of the
    /// other, [`Error::BlockNameConflict`]), since the seal would refuse the pair and the fragments
    /// can't be both a file and a directory.
    fn admit(&self, block_ref: &BlockRef) -> Result<()> {
        self.usable()?;
        if self.blocks.iter().any(|b| b.name == block_ref.name) {
            return Err(Error::Invalid(format!(
                "block '{}' is already committed to this write session",
                block_ref.name
            )));
        }
        leaf_rule_against(&self.blocks, &block_ref.name)
    }

    /// `Err` once the session is poisoned (see [`Self::append_block`]).
    fn usable(&self) -> Result<()> {
        match &self.poisoned {
            None => Ok(()),
            Some(cause) => Err(Error::Invalid(format!(
                "write session {} is poisoned: an earlier commit failed after its journal write \
                 began ({cause}), so the journal's state is unknown; reopen the session with \
                 WriteSession::recover, which keeps exactly the blocks committed before it",
                self.dir.display()
            ))),
        }
    }

    /// Move the durable `staged` file to the fragment `blocks/<name>` and make the move durable.
    /// Returns the guard on the fragment, which deletes it unless the commit succeeds. A rename;
    /// across filesystems (`CrossesDevices`), a copy, and the staged file is deleted.
    fn place(&mut self, staged: RemoveOnDrop, name: &BlockName) -> Result<RemoveOnDrop> {
        let fragment = fragment_guard(&self.dir, name)?;
        let parent = fragment
            .path()
            .parent()
            .expect("a fragment is under blocks/")
            .to_path_buf();
        match self.rename(staged.path(), fragment.path()) {
            Ok(()) => staged.disarm(),
            Err(e) if e.kind() == std::io::ErrorKind::CrossesDevices => {
                let mut copy = File::create(fragment.path())?;
                std::io::copy(&mut File::open(staged.path())?, &mut copy)?;
                copy.sync_all()?;
            }
            Err(e) => return Err(e.into()),
        }
        File::open(&parent)?.sync_all()?;
        Ok(fragment)
    }

    /// `fs::rename` (with a test seam to fail it as crossing filesystems).
    fn rename(&mut self, from: &Path, to: &Path) -> std::io::Result<()> {
        #[cfg(test)]
        if std::mem::take(&mut self.faults.cross_device) {
            return Err(std::io::ErrorKind::CrossesDevices.into());
        }
        fs::rename(from, to)
    }

    /// The commit every append shares, once the block's `fragment` is durable in `blocks/`: the
    /// journal line (the atomic commit point), the running Merkle root, and the write trace. On
    /// success the fragment is kept; on failure it is deleted, and a failure from the journal write
    /// on poisons the session (see [`Self::append_block`] for why).
    fn commit(
        &mut self,
        block_ref: BlockRef,
        digest: String,
        bytes: u64,
        fragment: RemoveOnDrop,
    ) -> Result<()> {
        let line = serde_json::to_string(&block_ref)?;
        // 2. journal line → durable (this is the commit watermark advance)
        let journaled = writeln!(self.journal, "{line}").and_then(|()| self.sync_journal());
        if let Err(e) = journaled {
            self.poisoned = Some(format!("block '{}': {e}", block_ref.name));
            return Err(std::io::Error::new(
                e.kind(),
                format!(
                    "journal commit of block '{}' failed: {e}; the write session accepts no more \
                     appends — reopen it with WriteSession::recover",
                    block_ref.name
                ),
            )
            .into());
        }
        fragment.disarm();
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

    /// fsync the journal (with a test seam to fail it).
    fn sync_journal(&mut self) -> std::io::Result<()> {
        #[cfg(test)]
        if std::mem::take(&mut self.faults.journal_sync) {
            return Err(std::io::Error::other("injected journal fsync failure"));
        }
        self.journal.sync_all()
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
    ///
    /// A **complete** line is a commit, so it is held to the format rather than dropped as torn: a
    /// name that fails the block-name grammar is refused with [`Error::InvalidBlockName`], and one
    /// that breaks the leaf rule against an earlier line with [`Error::BlockNameConflict`] (ADR-0063,
    /// #685). Neither is ever followed to a path; the session directory is left as it was.
    ///
    /// Once the journal is recovered, every file under `blocks/` that **no line of the journal
    /// names** (the fragment of an append that never reached its journal line) is deleted, with any
    /// directory left empty. Such a file is scratch, and left in place it would block a later append
    /// whose name needs its path as a directory (`volume` vs `volume/0`), or the reverse. A fragment
    /// that some line names is kept even when recovery stops before that line (a missing fragment
    /// earlier, a line this build can't parse), so a journal cut short here can still be repaired by
    /// hand.
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
            // A torn tail is a line that is not complete JSON (a prefix of a JSON object never is).
            let Ok(value) = serde_json::from_str::<Value>(line) else {
                break;
            };
            // Name first, so a bad one is refused by its own typed error, not mistaken for a tear.
            if let Some(name) = value.get("name").and_then(Value::as_str) {
                BlockName::new(name)?;
            }
            let Ok(r) = serde_json::from_value::<BlockRef>(value) else {
                break;
            };
            leaf_rule_against(&blocks, &r.name)?;
            let digest = r
                .digest
                .clone()
                .ok_or_else(|| Error::MissingDigest(r.name.to_string()))?;
            // a committed block must have its fragment on disk
            if !r.name.path_under(&dir.join(BLOCKS))?.is_file() {
                break;
            }
            merkle.push(&digest);
            blocks.push(r);
        }
        // Rewrite the journal to the clean recovered prefix (drops any torn tail), then reopen append.
        let clean: String = blocks
            .iter()
            .map(|r| serde_json::to_string(r).map(|s| s + "\n"))
            .collect::<std::result::Result<_, _>>()?;
        fs::write(dir.join(JOURNAL), &clean)?;
        let journal = OpenOptions::new().append(true).open(dir.join(JOURNAL))?;
        reset_staging(dir)?;
        remove_orphan_fragments(dir, &journal_names(&raw))?;
        Ok(WriteSession {
            dir: dir.to_path_buf(),
            header,
            journal,
            blocks,
            merkle,
            staged_seq: Arc::new(AtomicU64::new(0)),
            poisoned: None,
            #[cfg(test)]
            faults: Faults::default(),
        })
    }

    /// Compact + seal: build the manifest through [`ProductBuilder`] (identical to a batch writer),
    /// verify the running root matches the sealed `content_hash`, and pack the fragments into a
    /// sealed `.tsra` at `out`. Returns the sealed manifest. A poisoned session (see
    /// [`Self::append_block`]) refuses to seal: [`Self::recover`] it first.
    ///
    /// **Constant-memory.** Each fragment is streamed straight off disk into the zip via
    /// [`pack_streaming`] (one buffered `io::copy` per block), so the whole engine (encode, commit,
    /// and seal) never has to fit a product in RAM. This is the must-fix that lets the multi-block
    /// listmode path actually be bounded-memory end-to-end — otherwise seal would re-materialise
    /// every block in a `Vec<BlockPayload>` and defeat the rest of the pipeline.
    pub fn seal(self, out: &Path) -> Result<Manifest> {
        self.usable()?;
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
        let blocks_dir = self.dir.join(BLOCKS);
        let frag_paths: Vec<PathBuf> = self
            .blocks
            .iter()
            .map(|r| r.name.path_under(&blocks_dir))
            .collect::<Result<_>>()?;
        let sources: Vec<(BlockName, &Path)> = self
            .blocks
            .iter()
            .zip(frag_paths.iter())
            .map(|(r, p)| (r.name.clone(), p.as_path()))
            .collect();
        pack_streaming(&sealed, &sources, out)?;
        Ok(sealed)
    }
}

/// Every valid block name any complete line of the journal text `raw` carries, whether or not
/// recovery reached that line: the fragments [`remove_orphan_fragments`] must keep.
fn journal_names(raw: &str) -> Vec<BlockName> {
    raw.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|v| BlockName::new(v.get("name")?.as_str()?).ok())
        .collect()
}

/// Delete every file under the session's `blocks/` that is not the fragment of a block `named`,
/// then every directory left empty (see [`WriteSession::recover`]). Paths are compared
/// as built by [`BlockName::path_under`], so the walk never leaves `blocks/`, and it does not follow
/// symlinks.
fn remove_orphan_fragments(dir: &Path, named: &[BlockName]) -> Result<()> {
    let blocks = dir.join(BLOCKS);
    let keep = named
        .iter()
        .map(|n| n.path_under(&blocks))
        .collect::<Result<std::collections::BTreeSet<PathBuf>>>()?;
    /// Returns whether `d` is empty once its orphans are gone.
    fn walk(d: &Path, keep: &std::collections::BTreeSet<PathBuf>) -> Result<bool> {
        let mut empty = true;
        for entry in fs::read_dir(d)? {
            let entry = entry?;
            let path = entry.path();
            if keep.contains(&path) {
                // Named by the journal: kept as it is, file or (never walked into) directory.
                empty = false;
            } else if entry.file_type()?.is_dir() {
                if walk(&path, keep)? {
                    fs::remove_dir(&path)?;
                } else {
                    empty = false;
                }
            } else {
                fs::remove_file(&path)?;
            }
        }
        Ok(empty)
    }
    if blocks.is_dir() {
        walk(&blocks, &keep)?;
        File::open(&blocks)?.sync_all()?;
    }
    Ok(())
}

/// The leaf rule (ADR-0063) between `name` and the names already in `committed`: neither may be the
/// parent of the other ([`Error::BlockNameConflict`]).
fn leaf_rule_against(committed: &[BlockRef], name: &BlockName) -> Result<()> {
    for b in committed {
        let (parent, child) = if b.name.is_parent_of(name) {
            (&b.name, name)
        } else if name.is_parent_of(&b.name) {
            (name, &b.name)
        } else {
            continue;
        };
        return Err(Error::BlockNameConflict {
            parent: parent.to_string(),
            child: child.to_string(),
        });
    }
    Ok(())
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

    /// An incompressible 1 MiB blob payload, so a streamed append takes many reads and writes.
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

        // Mid-stream read error, halfway through the block.
        let flaky = Flaky {
            data: p2.clone(),
            pos: 0,
            fail_after: Some(p2.len() / 2),
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

    /// A staged block holding `bytes`, flushed to its file (as an encoder leaves it).
    fn staged(ws: &WriteSession, bytes: &[u8]) -> StagedBlock {
        let mut s = ws.staging().create().unwrap();
        s.write_all(bytes).unwrap();
        s.flush().unwrap();
        s
    }

    /// #677 r2 item 1: a journal fsync that fails AFTER its line was written leaves a complete line
    /// for a block the caller was told failed. Unless the session refuses to go on, a retry commits
    /// more blocks behind that orphaned line, and `recover`, which stops at it (its fragment is
    /// gone), drops every one of them. So the failure poisons the session: later appends are refused
    /// with an error that says to `recover`, and `recover` yields exactly the blocks committed
    /// before the failure and resumes from there. Both appends share the commit, so both are tested.
    #[test]
    fn a_failed_journal_sync_poisons_the_session_and_recover_keeps_the_blocks_before_it() {
        let dir = tempfile::tempdir().unwrap();
        let (ra, pa) = block("a", 0);
        let (rb, pb) = block("b", 1);
        let (rc, pc) = block("c", 2);
        for via in ["append_block", "append_staged"] {
            let stage = dir.path().join(via);
            let mut ws = WriteSession::create(&stage, "recon", "p", "d", TS).unwrap();
            ws.append_block(ra.clone(), &pa).unwrap();
            let root = ws.running_root();

            ws.faults.journal_sync = true;
            let err = if via == "append_block" {
                ws.append_block(rb.clone(), &pb).unwrap_err()
            } else {
                let s = staged(&ws, &pb);
                ws.append_staged(rb.clone(), s).unwrap_err()
            };
            assert!(
                err.to_string().contains("injected journal fsync failure"),
                "{via}: {err}"
            );
            // The hazard is real: only the fsync failed, so the line for `b` is complete.
            let journal = fs::read_to_string(stage.join(JOURNAL)).unwrap();
            assert_eq!(journal.lines().count(), 2, "{via}: {journal}");
            assert_eq!((ws.watermark(), ws.running_root()), (1, root.clone()));

            // Poisoned: no later append is accepted, and the error names the way out.
            let err = ws.append_block(rc.clone(), &pc).unwrap_err();
            assert!(err.to_string().contains("recover"), "{via}: {err}");
            let s = staged(&ws, &pc);
            let err = ws.append_staged(rc.clone(), s).unwrap_err();
            assert!(err.to_string().contains("recover"), "{via}: {err}");
            assert_eq!(
                session_files(&stage).2,
                0,
                "{via}: a refused append left a staged file"
            );
            let err = ws.seal(&dir.path().join("poisoned.tsra")).unwrap_err();
            assert!(err.to_string().contains("recover"), "{via}: {err}");

            // Recovery keeps exactly the blocks committed before the failure, then resumes.
            let mut ws = WriteSession::recover(&stage).unwrap();
            assert_eq!(
                (ws.watermark(), ws.running_root()),
                (1, root.clone()),
                "{via}: recover kept a block the caller was told failed"
            );
            assert_eq!(
                session_files(&stage).1.len(),
                1,
                "{via}: fragment of the failed block"
            );
            ws.append_block(rb.clone(), &pb).unwrap();
            ws.append_block(rc.clone(), &pc).unwrap();
            let sealed = ws.seal(&dir.path().join(format!("{via}.tsra"))).unwrap();
            let mut bb = ProductBuilder::new("recon", "p", "d", TS);
            for r in [&ra, &rb, &rc] {
                bb.add_block_ref(r.clone());
            }
            assert_eq!(
                sealed.manifest_hash,
                bb.seal().unwrap().manifest_hash,
                "{via}"
            );
        }
    }

    /// #677 r2 item 2 (ADR-0002 first-moment integrity): a staged block's digest is born as the
    /// encoder writes it, and the commit checks the bytes it moved into `blocks/` against it. So
    /// scratch corruption between the encode and the commit is refused (even with the digest left to
    /// the session) — no block, no fragment, no staged file — and, since the journal was never
    /// touched, the session goes on. Both ways into `blocks/` are checked: the rename, and the copy
    /// across filesystems.
    #[test]
    fn a_staged_block_corrupted_before_its_commit_is_refused_and_leaves_no_trace() {
        let dir = tempfile::tempdir().unwrap();
        let (ra, pa) = block("a", 0);
        let (rb, pb) = big_block("b");
        for cross_device in [false, true] {
            let stage = dir.path().join(format!("stage-{cross_device}"));
            let mut ws = WriteSession::create(&stage, "recon", "p", "d", TS).unwrap();
            ws.append_block(ra.clone(), &pa).unwrap();
            let before = session_files(&stage);
            let root = ws.running_root();

            let s = staged(&ws, &pb);
            let path = s.path().to_path_buf();
            let mut bytes = fs::read(&path).unwrap();
            bytes[pb.len() / 2] ^= 1;
            fs::write(&path, &bytes).unwrap();
            ws.faults.cross_device = cross_device;
            let mut unset = rb.clone();
            unset.digest = None;
            let err = ws.append_staged(unset, s).unwrap_err();
            assert!(
                matches!(
                    err,
                    Error::Integrity {
                        what: "staged_block",
                        ..
                    }
                ),
                "cross_device={cross_device}: {err}"
            );
            assert!(!path.exists(), "the corrupted staged file was left");
            assert!(
                session_files(&stage) == before,
                "a refused commit left a trace"
            );
            assert_eq!((ws.watermark(), ws.running_root()), (1, root));

            ws.faults.cross_device = cross_device;
            let s = staged(&ws, &pb);
            let committed = ws.append_staged(rb.clone(), s).unwrap();
            assert_eq!(committed.digest, rb.digest);
            assert_eq!(fs::read(stage.join(BLOCKS).join("b")).unwrap(), pb);
            assert_eq!((ws.watermark(), session_files(&stage).2), (2, 0));
        }
    }

    /// #677 r2 item 3: the commit moves a staged file into `blocks/` by a rename (the staging
    /// directory is inside the session directory, so on the same filesystem): the fragment IS the
    /// staged file, not a copy of it.
    #[cfg(unix)]
    #[test]
    fn a_staged_block_is_renamed_into_place_not_copied() {
        use std::os::unix::fs::MetadataExt;
        let dir = tempfile::tempdir().unwrap();
        let (rb, pb) = big_block("b");
        let stage = dir.path().join("stage");
        let mut ws = WriteSession::create(&stage, "recon", "p", "d", TS).unwrap();
        let s = staged(&ws, &pb);
        let inode = fs::metadata(s.path()).unwrap().ino();
        ws.append_staged(rb, s).unwrap();
        let fragment = stage.join(BLOCKS).join("b");
        assert_eq!(
            fs::metadata(&fragment).unwrap().ino(),
            inode,
            "the commit copied the staged block instead of renaming it"
        );
        assert_eq!(fs::read(&fragment).unwrap(), pb);
        assert_eq!(session_files(&stage).2, 0);
    }

    /// A block name is committed once: a second append of it is refused before it touches
    /// `blocks/`, so it can neither overwrite the committed fragment nor, failing, delete it.
    #[test]
    fn a_committed_block_name_is_refused_and_its_fragment_kept() {
        let dir = tempfile::tempdir().unwrap();
        let (ra, pa) = block("a", 0);
        let (other, po) = block("a", 7);
        let stage = dir.path().join("stage");
        let mut ws = WriteSession::create(&stage, "recon", "p", "d", TS).unwrap();
        ws.append_block(ra, &pa).unwrap();
        let before = session_files(&stage);
        let err = ws.append_block(other.clone(), &po).unwrap_err();
        assert!(err.to_string().contains("already committed"), "{err}");
        let s = staged(&ws, &po);
        let err = ws.append_staged(other, s).unwrap_err();
        assert!(err.to_string().contains("already committed"), "{err}");
        assert!(
            session_files(&stage) == before,
            "a refused append changed the session"
        );
        assert_eq!(ws.watermark(), 1);
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

    /// #685 / ADR-0063: a hierarchical name's fragment gets its parent directories under `blocks/`,
    /// and an append that fails after creating them (here, its journal fsync) removes them again with
    /// the fragment, through either append path.
    #[test]
    fn a_failed_append_removes_the_directories_it_created() {
        let dir = tempfile::tempdir().unwrap();
        let (r, p) = block("x/y/z", 0);
        for via in ["append_block", "append_staged"] {
            let stage = dir.path().join(via);
            let mut ws = WriteSession::create(&stage, "recon", "p", "d", TS).unwrap();
            ws.faults.journal_sync = true;
            let err = if via == "append_block" {
                ws.append_block(r.clone(), &p).unwrap_err()
            } else {
                let s = staged(&ws, &p);
                ws.append_staged(r.clone(), s).unwrap_err()
            };
            assert!(
                err.to_string().contains("injected journal fsync failure"),
                "{via}: {err}"
            );
            assert!(
                !stage.join(BLOCKS).join("x").exists(),
                "{via}: blocks/x/ left behind"
            );
            assert_eq!(
                fs::read_dir(stage.join(BLOCKS)).unwrap().count(),
                0,
                "{via}"
            );
        }
    }

    /// `recover` deletes fragments no recovered journal line names (an append that never reached its
    /// line), so a leftover file can't block a later append whose name needs that path as a directory.
    #[test]
    fn recover_removes_orphan_fragments_so_a_hierarchical_append_can_follow() {
        let dir = tempfile::tempdir().unwrap();
        let stage = dir.path().join("stage");
        let (ra, pa) = block("a/0", 0);
        {
            let mut ws = WriteSession::create(&stage, "recon", "p", "d", TS).unwrap();
            ws.append_block(ra, &pa).unwrap();
        }
        let blocks = stage.join(BLOCKS);
        fs::write(blocks.join("volume"), b"orphan").unwrap();
        fs::create_dir_all(blocks.join("deep/er")).unwrap();
        fs::write(blocks.join("deep/er/orphan"), b"orphan").unwrap();
        fs::write(blocks.join("a/1"), b"orphan beside a committed fragment").unwrap();

        let mut ws = WriteSession::recover(&stage).unwrap();
        assert_eq!(ws.watermark(), 1);
        assert!(blocks.join("a/0").is_file(), "the committed fragment stays");
        for gone in ["volume", "deep", "a/1"] {
            assert!(!blocks.join(gone).exists(), "orphan {gone} left behind");
        }
        let (r, p) = block("volume/0", 1);
        ws.append_block(r, &p).unwrap();
        assert!(blocks.join("volume/0").is_file());
    }

    /// Recovery that stops early (here: the second of three fragments is missing) truncates the
    /// journal but deletes no fragment a journal line named, so the third block can still be
    /// recovered by hand.
    #[test]
    fn recover_cut_short_keeps_the_fragments_of_later_lines() {
        let dir = tempfile::tempdir().unwrap();
        let stage = dir.path().join("stage");
        {
            let mut ws = WriteSession::create(&stage, "recon", "p", "d", TS).unwrap();
            for (i, n) in ["a", "b/0", "c/0"].into_iter().enumerate() {
                let (r, p) = block(n, i as i16);
                ws.append_block(r, &p).unwrap();
            }
        }
        let blocks = stage.join(BLOCKS);
        fs::remove_file(blocks.join("b/0")).unwrap();
        let ws = WriteSession::recover(&stage).unwrap();
        assert_eq!(ws.watermark(), 1, "recovery stops at the missing fragment");
        assert!(blocks.join("a").is_file());
        assert!(
            blocks.join("c/0").is_file(),
            "a later line's fragment must survive"
        );
    }
}
