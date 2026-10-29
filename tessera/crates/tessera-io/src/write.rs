//! Streaming write engine — fragment-append with hash-on-write + crash-recovery (ROADMAP P3 / S17).
//!
//! The DAQ rule (S17): **never encode on the hot path** into a single sealed file (a footer-at-end
//! format means a crash = total loss). Instead a [`WriteSession`] stages each already-encoded block
//! as a durable **fragment** under a directory and records it in an append-only **journal**; the
//! journal line is the commit point. A crash leaves the session consistent up to the last intact
//! journal line (the **watermark**); [`WriteSession::recover`] replays to it, dropping any torn tail,
//! refusing a corrupt journal and quarantining a tail it can't replay, and rewrites the journal
//! durably, so a crash during recovery loses nothing and recovering twice changes nothing.
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
/// The recovery quarantine (see [`WriteSession::recover`]): each `quarantine/<id>/` holds journal
/// lines a recovery could not replay (`journal.jsonl`, verbatim) and the fragments they name
/// (`blocks/`). Nothing here is ever part of the product, and the names it holds stay taken.
const QUARANTINE: &str = "quarantine";

#[cfg(test)]
thread_local! {
    /// Test-only seam: when set to `n > 0`, fail this thread's `n`-th next [`write_durable`] once its
    /// temp file is durable and before the rename, the point where a crash leaves both the old file
    /// and the new one on disk.
    static FAIL_NTH_DURABLE_RENAME: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

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
    /// The names held in the recovery quarantine, each with its quarantine directory: an append
    /// that collides with one is refused (see [`WriteSession::recover`]).
    quarantined: Vec<(BlockName, PathBuf)>,
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
    let parent = path.parent().expect("under blocks/").to_path_buf();
    let mut guard = RemoveOnDrop::new(path);
    create_dirs_durable(&blocks, &parent, &mut guard.created_dirs)?;
    Ok(guard)
}

/// Create the directory `dir` and its missing ancestors below `base` (an existing ancestor), each
/// fsync'd into its parent so the path survives a crash. Every directory created is recorded in
/// `created` as soon as it exists, deepest first (the order to remove them in), so a caller can
/// undo a partial creation.
fn create_dirs_durable(base: &Path, dir: &Path, created: &mut Vec<PathBuf>) -> Result<()> {
    let mut missing = Vec::new();
    let mut d = Some(dir);
    while let Some(p) = d.filter(|p| *p != base && !p.exists()) {
        missing.push(p.to_path_buf());
        d = p.parent();
    }
    for p in missing.into_iter().rev() {
        fs::create_dir(&p)?;
        sync_dir(p.parent().expect("below base"))?;
        created.insert(0, p);
    }
    Ok(())
}

/// fsync the directory `dir`, so the entries created, renamed or removed in it survive a crash.
fn sync_dir(dir: &Path) -> std::io::Result<()> {
    File::open(dir)?.sync_all()
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

/// Write `bytes` to `path` **durably + atomically**: the one path every whole-file write of a session
/// takes (the header, the journal [`WriteSession::recover`] rewrites, a quarantine's journal). It writes
/// a temp file beside `path` (`<file name>.tmp`, in the same directory, so the rename never crosses
/// filesystems), fsyncs it, renames it over `path`, then fsyncs the directory so the rename itself is
/// on disk. The rename is atomic, so a crash at any point leaves either the old file or the complete
/// new one, never a torn, truncated or missing one. A failure removes the temp file again and leaves
/// `path` as it was; a crash can leave the temp file, which the next write to `path` replaces.
fn write_durable(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut tmp_name = path
        .file_name()
        .expect("a durable write names a file")
        .to_os_string();
    tmp_name.push(".tmp");
    let tmp = RemoveOnDrop::new(path.with_file_name(tmp_name));
    {
        let mut f = File::create(tmp.path())?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    #[cfg(test)]
    if FAIL_NTH_DURABLE_RENAME.with(|n| n.replace(n.get().saturating_sub(1)) == 1) {
        return Err(std::io::Error::other("injected crash before the durable rename").into());
    }
    fs::rename(tmp.path(), path)?;
    tmp.disarm();
    sync_dir(match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    })?;
    Ok(())
}

impl WriteSession {
    /// Start a new session in `dir` (created if absent): writes the header and an empty journal. A
    /// recovery quarantine already in `dir` is left in place, and the names it holds stay taken (see
    /// [`Self::recover`]).
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
            quarantined: quarantined_names(dir)?,
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
    /// committed (its fragment would be overwritten, and deleted again if the append failed); one
    /// whose name and a committed name break the leaf rule (ADR-0063: one the parent of the
    /// other, [`Error::BlockNameConflict`]), since the seal would refuse the pair and the fragments
    /// can't be both a file and a directory; and one whose name is, or is the parent or child of, a
    /// name held in the recovery quarantine ([`Error::QuarantinedBlockName`], see [`Self::recover`]).
    fn admit(&self, block_ref: &BlockRef) -> Result<()> {
        self.usable()?;
        let name = &block_ref.name;
        if self.blocks.iter().any(|b| b.name == *name) {
            return Err(Error::Invalid(format!(
                "block '{name}' is already committed to this write session"
            )));
        }
        leaf_rule_against(self.blocks.iter().map(|b| &b.name), name)?;
        let collision = self
            .quarantined
            .iter()
            .find(|(held, _)| held == name || held.is_parent_of(name) || name.is_parent_of(held));
        if let Some((held, quarantine)) = collision {
            return Err(Error::QuarantinedBlockName {
                name: name.to_string(),
                quarantined: held.to_string(),
                quarantine: quarantine.clone(),
            });
        }
        Ok(())
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
        sync_dir(&parent)?;
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

    /// Reopen a staging directory after a crash, replaying the journal to the last intact commit, and
    /// leave it ready for appends. Running it again on its result changes nothing.
    ///
    /// **The journal.** Every complete (newline-terminated) line is a commit; only an unterminated
    /// last line is a **torn tail**, the line of an append that never returned success (its fsync
    /// runs after the newline is written), and it is dropped whatever its bytes. A complete line is
    /// held to the format, so a journal with one that is not a valid commit is refused, before
    /// anything on disk changes: a name that fails the block-name grammar with
    /// [`Error::InvalidBlockName`], a pair of names that breaks the leaf rule with
    /// [`Error::BlockNameConflict`] (ADR-0063, #685; neither name is ever followed to a path), a
    /// line without a digest with [`Error::MissingDigest`], and a line that is not UTF-8, not JSON,
    /// not a block reference, or a block committed a second time with [`Error::CorruptJournal`].
    /// That strictness has a cost: if a crash leaves stale bytes that happen to end in a newline at
    /// the journal's end (possible on a filesystem that journals metadata but not data), recovery
    /// refuses the session rather than guess which lines are commits.
    ///
    /// **Replay** keeps the lines up to the first whose fragment is missing (an append whose fragment
    /// deletion reached the disk and whose line did too, see [`Self::append_block`]). The journal is
    /// rewritten to exactly those lines, byte for byte, through the durable write path (temp file,
    /// fsync, atomic rename, directory fsync), so a crash during recovery leaves the old journal or
    /// the new one, never a truncated one, and recovering again gives the same blocks (#686).
    ///
    /// **Quarantine.** If complete lines follow the first one that can't be replayed, they are
    /// commits recovery can't make sense of (only damage to `blocks/` produces them), so they are
    /// neither replayed nor discarded: that line and every line after it are moved, verbatim, to
    /// `quarantine/<id>/journal.jsonl`, and the fragments they name to `quarantine/<id>/blocks/`,
    /// where `<id>` is the blake3 hex of those lines (a crash mid-quarantine is completed by the next
    /// recovery, into the same directory). A quarantine is never sealed, and the names it holds stay
    /// taken: an append whose name is one, or a parent or child of one, is refused with
    /// [`Error::QuarantinedBlockName`], so a repair never meets two blocks of one name. To repair by
    /// hand, put the missing fragment back, move the quarantined fragments into `blocks/`, append the
    /// quarantined lines to `journal.jsonl`, delete the quarantine directory and recover again;
    /// deleting the directory alone discards the tail and frees its names. A lone last line that
    /// can't be replayed is dropped and not quarantined: its fragment is gone, so nothing of the block
    /// is left to keep, and its block can be appended again. Usually it is the commit whose journal
    /// fsync failed (see [`Self::append_block`]); it can also be an acknowledged commit whose fragment
    /// was lost to damage to `blocks/`, and that block is then lost with it. [`Self::create`] on a
    /// directory with a quarantine leaves it in place, and its names stay taken there too.
    ///
    /// **Clean-up.** Every file under `blocks/` that no replayed line names (the fragment of an
    /// append that never reached its journal line) is deleted, with any directory left empty: such a
    /// file is scratch, and left in place it would block a later append whose name needs its path as
    /// a directory (`volume` vs `volume/0`), or the reverse. The staging directory is emptied.
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
        let journal_path = dir.join(JOURNAL);
        let raw = fs::read(&journal_path)?;
        let lines = parse_journal(&journal_path, &raw)?;
        let blocks_dir = dir.join(BLOCKS);
        let mut replayed = 0;
        for line in &lines {
            if !line.block.name.path_under(&blocks_dir)?.is_file() {
                break;
            }
            replayed += 1;
        }
        let (kept, unreplayed) = lines.split_at(replayed);
        if unreplayed.len() > 1 {
            quarantine(dir, unreplayed)?;
        }
        write_durable(
            &journal_path,
            &kept
                .iter()
                .flat_map(|l| l.raw)
                .copied()
                .collect::<Vec<u8>>(),
        )?;
        let journal = OpenOptions::new().append(true).open(&journal_path)?;
        reset_staging(dir)?;
        remove_orphan_fragments(dir, kept.iter().map(|l| &l.block.name))?;
        let quarantined = quarantined_names(dir)?;
        let mut merkle = MerkleAccumulator::new();
        for line in kept {
            merkle.push(&line.digest);
        }
        Ok(WriteSession {
            dir: dir.to_path_buf(),
            header,
            journal,
            blocks: kept.iter().map(|l| l.block.clone()).collect(),
            merkle,
            staged_seq: Arc::new(AtomicU64::new(0)),
            poisoned: None,
            quarantined,
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

/// One complete line of a journal: the commit it records, the commit's digest, and the line's
/// bytes, newline included.
struct JournalLine<'a> {
    block: BlockRef,
    digest: String,
    raw: &'a [u8],
}

/// The commits of the journal bytes `raw` read from `path`, in order, each checked as
/// [`WriteSession::recover`] documents. The unterminated last line, if any, is the torn tail and is
/// not returned.
fn parse_journal<'a>(path: &Path, raw: &'a [u8]) -> Result<Vec<JournalLine<'a>>> {
    let mut lines: Vec<JournalLine<'a>> = Vec::new();
    let mut names = std::collections::BTreeSet::<String>::new();
    for (i, line) in raw.split_inclusive(|&b| b == b'\n').enumerate() {
        // Only the last piece can lack its newline: the torn tail.
        let Some(text) = line.strip_suffix(b"\n") else {
            break;
        };
        let corrupt = |detail: String| Error::CorruptJournal {
            path: path.to_path_buf(),
            line: i + 1,
            detail,
        };
        let text = std::str::from_utf8(text).map_err(|e| corrupt(format!("not UTF-8: {e}")))?;
        let value: Value =
            serde_json::from_str(text).map_err(|e| corrupt(format!("not JSON: {e}")))?;
        // Name first, so a bad one is refused by its own typed error.
        if let Some(name) = value.get("name").and_then(Value::as_str) {
            BlockName::new(name)?;
        }
        let block: BlockRef = serde_json::from_value(value)
            .map_err(|e| corrupt(format!("not a block reference: {e}")))?;
        let digest = block
            .digest
            .clone()
            .ok_or_else(|| Error::MissingDigest(block.name.to_string()))?;
        let name = block.name.to_string();
        if names.contains(&name) {
            return Err(corrupt(format!(
                "block '{name}' is committed a second time"
            )));
        }
        // The leaf rule against the earlier lines, in O(segments · log n): an earlier parent is a
        // `/`-prefix of the name, and earlier children sort contiguously from `<name>/`.
        let parent = name
            .match_indices('/')
            .map(|(i, _)| &name[..i])
            .find(|p| names.contains(*p));
        let child_prefix = format!("{name}/");
        let child = names
            .range(child_prefix.clone()..)
            .next()
            .filter(|c| c.starts_with(&child_prefix));
        let conflict = match (parent, child) {
            (Some(parent), _) => Some((parent.to_string(), name.clone())),
            (None, Some(child)) => Some((name.clone(), child.clone())),
            (None, None) => None,
        };
        if let Some((parent, child)) = conflict {
            return Err(Error::BlockNameConflict { parent, child });
        }
        names.insert(name);
        lines.push(JournalLine {
            block,
            digest,
            raw: line,
        });
    }
    Ok(lines)
}

/// Move the journal lines `tail` (from the first a recovery can't replay on) and the fragments
/// they name out of the session at `dir`, into `quarantine/<blake3 hex of the lines>/` (see
/// [`WriteSession::recover`]). The quarantine's journal is written first, so its directory never
/// holds a fragment without the line that names it; every step is durable, and a run cut short is
/// completed by the next, which computes the same directory.
fn quarantine(dir: &Path, tail: &[JournalLine]) -> Result<()> {
    let text: Vec<u8> = tail.iter().flat_map(|l| l.raw).copied().collect();
    let id = tessera_core::hash::digest(&text);
    let q = dir
        .join(QUARANTINE)
        .join(id.strip_prefix("blake3:").unwrap_or(&id));
    create_dirs_durable(dir, &q, &mut Vec::new())?;
    write_durable(&q.join(JOURNAL), &text)?;
    let (from, to) = (dir.join(BLOCKS), q.join(BLOCKS));
    for line in tail {
        let src = line.block.name.path_under(&from)?;
        if !src.is_file() {
            continue;
        }
        let dst = line.block.name.path_under(&to)?;
        let dst_dir = dst.parent().expect("under the quarantine's blocks/");
        create_dirs_durable(&q, dst_dir, &mut Vec::new())?;
        fs::rename(&src, &dst)?;
        sync_dir(dst_dir)?;
        sync_dir(src.parent().expect("under blocks/"))?;
    }
    Ok(())
}

/// Every name held in the recovery quarantine of the session at `dir`, with its quarantine
/// directory, read from each quarantine's journal. A quarantine directory without a journal holds
/// no fragment (the journal is written first) and so no name.
fn quarantined_names(dir: &Path) -> Result<Vec<(BlockName, PathBuf)>> {
    let root = dir.join(QUARANTINE);
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let mut quarantines = fs::read_dir(&root)?
        .map(|e| e.map(|e| e.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    quarantines.sort();
    let mut held = Vec::new();
    for q in quarantines {
        let journal = q.join(JOURNAL);
        if !journal.is_file() {
            continue;
        }
        for line in parse_journal(&journal, &fs::read(&journal)?)? {
            held.push((line.block.name, q.clone()));
        }
    }
    Ok(held)
}

/// Delete every file under the session's `blocks/` that is not the fragment of a block `named`,
/// then every directory left empty (see [`WriteSession::recover`]). Paths are compared
/// as built by [`BlockName::path_under`], so the walk never leaves `blocks/`, and it does not follow
/// symlinks.
fn remove_orphan_fragments<'n>(
    dir: &Path,
    named: impl IntoIterator<Item = &'n BlockName>,
) -> Result<()> {
    let blocks = dir.join(BLOCKS);
    let keep = named
        .into_iter()
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
        sync_dir(&blocks)?;
    }
    Ok(())
}

/// The leaf rule (ADR-0063) between `name` and the names already `committed`: neither may be the
/// parent of the other ([`Error::BlockNameConflict`]).
fn leaf_rule_against<'n>(
    committed: impl IntoIterator<Item = &'n BlockName>,
    name: &BlockName,
) -> Result<()> {
    for c in committed {
        let (parent, child) = if c.is_parent_of(name) {
            (c, name)
        } else if name.is_parent_of(c) {
            (name, c)
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

    /// Every path under `root` (relative, `/`-separated) with a file's bytes, or `None` for a
    /// directory: the whole on-disk state, to compare before and after.
    fn tree(root: &Path) -> BTreeMap<String, Option<Vec<u8>>> {
        fn walk(root: &Path, d: &Path, out: &mut BTreeMap<String, Option<Vec<u8>>>) {
            for e in fs::read_dir(d).unwrap() {
                let path = e.unwrap().path();
                let rel = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                if path.is_dir() {
                    out.insert(rel, None);
                    walk(root, &path, out);
                } else {
                    out.insert(rel, Some(fs::read(&path).unwrap()));
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(root, root, &mut out);
        out
    }

    /// A session at `stage` with blocks `names` committed (block `i` built from seed `i`).
    fn session_with(stage: &Path, names: &[&str]) -> WriteSession {
        let mut ws = WriteSession::create(stage, "recon", "p", "d", TS).unwrap();
        for (i, n) in names.iter().enumerate() {
            let (r, p) = block(n, i as i16);
            ws.append_block(r, &p).unwrap();
        }
        ws
    }

    /// #686: a crash while `recover` rewrites the journal (here: after the new journal is written
    /// and before it replaces the old one) leaves the ORIGINAL journal in place, so no committed
    /// block is lost, and a second `recover` gives the same blocks as an uninterrupted one.
    #[test]
    fn a_crash_while_recover_rewrites_the_journal_keeps_the_original() {
        let dir = tempfile::tempdir().unwrap();
        let stage = dir.path().join("stage");
        let root = session_with(&stage, &["a", "b"]).running_root();
        // A torn tail, so the recovery has to rewrite the journal.
        let jpath = stage.join(JOURNAL);
        let clean = fs::read(&jpath).unwrap();
        let mut f = OpenOptions::new().append(true).open(&jpath).unwrap();
        f.write_all(b"{\"name\":\"c\",\"kin").unwrap();
        drop(f);
        let before = fs::read(&jpath).unwrap();

        FAIL_NTH_DURABLE_RENAME.with(|n| n.set(1));
        let err = WriteSession::recover(&stage)
            .err()
            .expect("the injected crash");
        assert!(err.to_string().contains("injected"), "{err}");
        assert_eq!(
            fs::read(&jpath).unwrap(),
            before,
            "the original journal must survive a crash during its rewrite"
        );

        // The seam returns an error, and the failed write removes its temp file again. A real crash
        // leaves the temp file behind; the next durable write replaces it.
        let tmp = stage.join("journal.jsonl.tmp");
        assert!(
            !tmp.exists(),
            "a failed durable write removes its temp file"
        );
        fs::write(&tmp, b"stale bytes of a crashed rewrite\n").unwrap();

        let ws = WriteSession::recover(&stage).unwrap();
        assert_eq!((ws.watermark(), ws.running_root()), (2, root));
        let names: Vec<_> = ws.blocks.iter().map(|b| b.name.to_string()).collect();
        assert_eq!(names, ["a", "b"]);
        assert!(
            !tmp.exists(),
            "the stale temp file is replaced and renamed away"
        );
        assert_eq!(fs::read(&jpath).unwrap(), clean, "the torn tail is gone");
    }

    /// #686 (PR #691 review, finding 1): a recovery that stops early (here: the second of three
    /// fragments is missing) moves the journal lines from that point on, and the fragments they name,
    /// into a quarantine, which a second `recover` leaves exactly as it is. The quarantined names
    /// stay taken: an append that collides with one gets the typed error, never an I/O error (a kept
    /// `c/0` directory made an append of `c` fail with EISDIR).
    #[test]
    fn recover_quarantines_an_unreplayable_tail_and_a_second_recover_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let stage = dir.path().join("stage");
        drop(session_with(&stage, &["a", "b/0", "c/0"]));
        let journal = fs::read_to_string(stage.join(JOURNAL)).unwrap();
        let tail: String = journal.split_inclusive('\n').skip(1).collect();
        let c0 = fs::read(stage.join("blocks/c/0")).unwrap();
        fs::remove_file(stage.join("blocks/b/0")).unwrap();

        let ws = WriteSession::recover(&stage).unwrap();
        assert_eq!(ws.watermark(), 1, "recovery stops at the missing fragment");
        drop(ws);
        let q = stage
            .join(QUARANTINE)
            .join(tessera_core::hash::digest(tail.as_bytes()).trim_start_matches("blake3:"));
        assert_eq!(
            fs::read_to_string(q.join(JOURNAL)).unwrap(),
            tail,
            "the quarantine holds the unreplayed lines verbatim"
        );
        assert_eq!(
            fs::read(q.join("blocks/c/0")).unwrap(),
            c0,
            "and their fragments"
        );
        assert!(
            !stage.join("blocks/c").exists(),
            "nothing of the tail is left in blocks/"
        );
        assert!(!stage.join("blocks/b").exists());

        let after_first = tree(&stage);
        let mut ws = WriteSession::recover(&stage).unwrap();
        assert_eq!(ws.watermark(), 1);
        assert_eq!(
            tree(&stage),
            after_first,
            "a second recover changes nothing"
        );

        for (name, quarantined) in [
            ("c", "c/0"),
            ("c/0", "c/0"),
            ("c/0/x", "c/0"),
            ("b/0", "b/0"),
        ] {
            let (r, p) = block(name, 9);
            match ws.append_block(r, &p) {
                Err(Error::QuarantinedBlockName {
                    name: n,
                    quarantined: held,
                    quarantine,
                }) => {
                    assert_eq!((n.as_str(), held.as_str()), (name, quarantined));
                    assert_eq!(quarantine, q);
                }
                other => panic!("append of {name}: expected QuarantinedBlockName, got {other:?}"),
            }
        }
        assert_eq!(
            tree(&stage),
            after_first,
            "a refused append leaves no trace"
        );

        // The session goes on beside the quarantine, which is never sealed.
        let (rd, pd) = block("d", 3);
        ws.append_block(rd.clone(), &pd).unwrap();
        let sealed = ws.seal(&dir.path().join("out.tsra")).unwrap();
        let names: Vec<_> = sealed.blocks.iter().map(|b| b.name.to_string()).collect();
        assert_eq!(names, ["a", "d"]);

        // A new session in the same directory keeps the quarantine, and its names stay taken.
        let mut ws = WriteSession::create(&stage, "recon", "p", "d", TS).unwrap();
        assert!(q.join(JOURNAL).is_file());
        let (r, p) = block("c", 9);
        assert!(matches!(
            ws.append_block(r, &p),
            Err(Error::QuarantinedBlockName { .. })
        ));
    }

    /// The leaf rule over journal lines holds in both directions and only across a `/`: a name that
    /// merely shares a prefix (`c-x`, `c.x`, which sort between `c` and `c/…`) is no conflict.
    #[test]
    fn parse_journal_applies_the_leaf_rule_both_ways_and_only_across_a_slash() {
        let lines = |names: &[&str]| -> Vec<u8> {
            names
                .iter()
                .enumerate()
                .flat_map(|(i, n)| {
                    format!(
                        "{}\n",
                        serde_json::to_string(&block(n, i as i16).0).unwrap()
                    )
                    .into_bytes()
                })
                .collect()
        };
        let path = Path::new("journal.jsonl");
        for (names, conflict) in [
            (&["c", "c-x", "c.x", "c/0"][..], Some(("c", "c/0"))),
            (&["c-x", "c.x", "c/0", "c"][..], Some(("c", "c/0"))),
            (&["a/b/c", "a"][..], Some(("a", "a/b/c"))),
            (&["c", "c-x", "c.x", "cc/0", "d/c"][..], None),
        ] {
            match (parse_journal(path, &lines(names)), conflict) {
                (Ok(parsed), None) => assert_eq!(parsed.len(), names.len()),
                (Err(Error::BlockNameConflict { parent, child }), Some((p, c))) => {
                    assert_eq!((parent.as_str(), child.as_str()), (p, c), "{names:?}");
                }
                (other, _) => panic!("{names:?}: got {:?}", other.map(|l| l.len())),
            }
        }
    }

    /// A crash in the middle of a quarantining recovery (after the quarantine's journal is written,
    /// or after its fragments are moved but before the session's journal is rewritten) is completed
    /// by the next recovery, into the same quarantine: the result is byte for byte that of a recovery
    /// never interrupted.
    #[test]
    fn a_crash_while_recover_quarantines_is_completed_by_the_next_recover() {
        let dir = tempfile::tempdir().unwrap();
        let stage_cut_short = |tag: &str| {
            let stage = dir.path().join(tag);
            drop(session_with(&stage, &["a", "b/0", "c/0", "d"]));
            fs::remove_file(stage.join("blocks/b/0")).unwrap();
            stage
        };
        let reference = stage_cut_short("reference");
        drop(WriteSession::recover(&reference).unwrap());
        // Durable writes in order: 1 = the quarantine's journal, 2 = the session's journal.
        for nth in [1, 2] {
            let stage = stage_cut_short(&format!("crash-{nth}"));
            FAIL_NTH_DURABLE_RENAME.with(|n| n.set(nth));
            let err = WriteSession::recover(&stage)
                .err()
                .expect("the injected crash");
            assert!(err.to_string().contains("injected"), "{nth}: {err}");
            if nth == 2 {
                // Cut between the moves and the rewrite: the tail's fragments are already in the
                // quarantine, and the journal still names them.
                let journal = fs::read_to_string(stage.join(JOURNAL)).unwrap();
                assert_eq!(journal.lines().count(), 4, "{journal}");
                assert!(!stage.join("blocks/d").exists(), "the fragments were moved");
            }
            let ws = WriteSession::recover(&stage).unwrap();
            assert_eq!(ws.watermark(), 1, "{nth}");
            drop(ws);
            assert_eq!(
                tree(&stage),
                tree(&reference),
                "crash at durable write {nth}"
            );
        }
    }

    /// #686 (PR #691 review, finding 2): a complete journal line is a commit, so one that is not a
    /// valid commit is refused with the typed error and nothing on disk changes (here an orphan
    /// fragment and a later line's fragment would otherwise be deleted). Only an unterminated last
    /// line is a torn tail.
    #[test]
    fn recover_refuses_a_complete_line_that_is_not_a_valid_commit() {
        let dir = tempfile::tempdir().unwrap();
        let cases: [(&str, &[u8]); 5] = [
            ("not a block reference", b"{\"name\":\"c\"}\n"),
            ("not JSON", b"{\"name\":\"c\",\"kin\n"),
            ("not UTF-8", b"\xff\xfe\n"),
            ("an empty line", b"\n"),
            ("committed twice", b""),
        ];
        let (rb, pb) = block("b", 1);
        let line_b = format!("{}\n", serde_json::to_string(&rb).unwrap());
        for (what, bad) in cases {
            let stage = dir.path().join(what.replace(' ', "-"));
            drop(session_with(&stage, &["a"]));
            let jpath = stage.join(JOURNAL);
            // `a`, the bad line (for "committed twice", `a` again), then a valid line for `b`.
            let first = fs::read(&jpath).unwrap();
            let bad = if bad.is_empty() {
                first.as_slice()
            } else {
                bad
            };
            fs::write(&jpath, [first.as_slice(), bad, line_b.as_bytes()].concat()).unwrap();
            fs::write(stage.join("blocks/b"), &pb).unwrap();
            fs::write(stage.join("blocks/orphan"), b"orphan").unwrap();
            let before = tree(&stage);

            match WriteSession::recover(&stage) {
                Err(Error::CorruptJournal { path, line, .. }) => {
                    assert_eq!((path, line), (jpath.clone(), 2), "{what}");
                }
                Err(other) => panic!("{what}: expected CorruptJournal, got {other}"),
                Ok(_) => {
                    panic!("{what}: a complete invalid line must be refused, not dropped as torn")
                }
            }
            assert_eq!(tree(&stage), before, "{what}: recover must change nothing");
        }
    }

    /// The torn tail is exactly the unterminated last line, whatever its bytes: a cut-off prefix,
    /// a whole JSON object whose newline never reached the disk (its fsync never ran, so the append
    /// never returned success), or bytes cut inside a UTF-8 character. Each is dropped, with the
    /// fragment its append left, and the blocks before it are kept.
    #[test]
    fn recover_drops_the_unterminated_last_line_whatever_its_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let (rc, pc) = block("c", 2);
        let whole = serde_json::to_string(&rc).unwrap();
        let cases: [(&str, Vec<u8>); 3] = [
            ("prefix", whole.as_bytes()[..whole.len() / 2].to_vec()),
            ("whole-object", whole.clone().into_bytes()),
            ("split-utf8", b"{\"name\":\"\xc3".to_vec()),
        ];
        for (what, torn) in cases {
            let stage = dir.path().join(what);
            let root = session_with(&stage, &["a", "b"]).running_root();
            let clean = fs::read(stage.join(JOURNAL)).unwrap();
            fs::write(stage.join("blocks/c"), &pc).unwrap();
            let mut f = OpenOptions::new()
                .append(true)
                .open(stage.join(JOURNAL))
                .unwrap();
            f.write_all(&torn).unwrap();
            drop(f);

            let ws = WriteSession::recover(&stage).unwrap();
            assert_eq!((ws.watermark(), ws.running_root()), (2, root), "{what}");
            assert_eq!(fs::read(stage.join(JOURNAL)).unwrap(), clean, "{what}");
            assert!(
                !stage.join("blocks/c").exists(),
                "{what}: the torn append's fragment"
            );
        }
    }
}
