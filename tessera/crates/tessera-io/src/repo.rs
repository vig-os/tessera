//! Content-addressed **repository** — the copy-on-write versioning substrate (ADR-0036).
//!
//! A repository is a directory (or, later, an object-store prefix) laid out as:
//!
//! ```text
//! objects/<alg>/<aa>/<rest-of-hex>   write-once content-addressed objects (blocks AND manifests)
//! layout                             tessera-repository 2
//! refs/lineages/<enc>                 <kind> <address> LF (mutable, CAS)
//! refs/pins/<enc>                     <kind> <address> LF (durable gc roots, CAS)
//! log/lineages/<enc>.jsonl            rebuildable cache: one line per commit (NOT canonical)
//! lock                               the writer lock (empty; see below)
//! tmp/                               a writer's temp files; whatever is here is crash debris
//! ```
//!
//! Objects are addressed by the **blake3 digest we already compute** (`BlockRef.digest`,
//! `manifest_hash`) — no second hash is introduced. A manifest object holds the manifest's stored
//! bytes (ADR-0066), so every object, block or manifest, is literally the bytes that hash to its
//! address. Identical bytes share one path, so a new version
//! that only changes metadata re-stores exactly one object (its manifest): the copy is proportional to
//! the *delta*, not the dataset. The version DAG is the `supersedes`/`derived_from` edges already inside
//! each manifest (ADR-0022); `refs/lineages/<enc>` is the only mutable state, and the `log` is a pure cache
//! that can be rebuilt by walking the DAG from the ref.
//!
//! # One fenced writer per repository (ADR-0036 §2, #946)
//!
//! Every write ([`commit`](Repository::commit), [`set_ref`](Repository::set_ref),
//! [`write_object`](Repository::write_object), [`put_manifest`](Repository::put_manifest),
//! [`put_collection`](Repository::put_collection), [`commit_collection`](Repository::commit_collection),
//! [`set_pin`](Repository::set_pin), [`remove_pin`](Repository::remove_pin),
//! [`forget`](Repository::forget), [`gc`](Repository::gc)) holds one exclusive lock for the whole
//! repository, an OS advisory lock (`flock`) on the file `lock`, from its first check to its last
//! write. Concurrent writers, threads or processes, queue on it. So a ref advance is a real
//! compare-and-set: the check of the tip and the write of the new one happen under the lock, and a
//! commit built on a tip that moved is refused with [`Error::RefMoved`], never silently lost. `gc`
//! holds the lock for its whole run, so it never sweeps the objects of a commit between their store
//! and the ref advance that makes them reachable. Readers take no lock: a ref is replaced by a
//! rename, so a reader sees the old tip or the new one.
//!
//! The lock is **advisory and local-filesystem only**: it binds tessera writers, not other programs
//! that edit the files, and it is not valid on NFS or on any network filesystem without working
//! `flock` (there two hosts can both hold it). The object-store form of a repository fences with a
//! conditional put instead (#288), which is not built.
//!
//! **Durability order.** Each object is written to a temp file in `tmp/`, fsync'd, renamed to its
//! address and its directory fsync'd (missing shard directories are created durably first); only
//! then is the ref written the same way. A crash at any step leaves the ref at the old version or
//! the new one, and every object it reaches complete. An object present at its address is checked
//! by hash before a write skips it, so a torn object a crash left is replaced by the next write of
//! the same bytes. The `log` cache is appended after the ref and not fsync'd: after a crash it can
//! lack the last commit, which a walk from the ref still finds, or end in a torn line that makes
//! [`log`](Repository::log) fail; and a failed append makes `commit` return the error although
//! the version landed (its ref names it). Rebuilding the cache from the DAG is #971.
//!
//! This module is the store + `commit`/`log` primitives; higher-level verbs (`diff`/`publish`/`seal`)
//! and the `evolve` delta builder layer on top.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tessera_core::block::{check_block_refs, BlockName, BlockRef};
use tessera_core::collection::MemberKind;
use tessera_core::{Collection, Error, Manifest, ObjectKind, Result};

use crate::durable::{create_dirs_durable, remove_file, sync_dir, write_durable_via};
use crate::BlockPayload;

mod collection_commit;
mod pins;
pub use collection_commit::{HistoryEdge, HistoryEntry, NewMember};
pub use pins::PinTarget;

/// The writer lock file, under the repository root.
const LOCK: &str = "lock";
/// The directory of a writer's temp files, under the repository root: on the filesystem of
/// `objects/` and `refs/` (so a rename into either never crosses one), and outside both (so no
/// listing of either ever sees a temp file).
const TMP: &str = "tmp";
/// The role of the edge a new version carries to the version it replaces (ADR-0022).
const SUPERSEDES: &str = "supersedes";
/// The layout marker under the repository root (ADR-0067 D1).
const LAYOUT: &str = "layout";
/// The only layout this build reads or writes. No migration is provided.
const LAYOUT_V2: &str = "tessera-repository 2\n";
/// The longest encoded ref name: 255 less the log cache's `.jsonl` suffix.
pub const MAX_REF_FILE_NAME: usize = 249;
/// The longest encoded lineage name, shared with pins.
pub const MAX_LINEAGE_FILE_NAME: usize = MAX_REF_FILE_NAME;
/// The two independent ref roles in repository layout 2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefNamespace {
    /// Mutable lineage tips, advanced by compare-and-set.
    Lineages,
    /// Durable gc roots, independent of lineage operations.
    Pins,
}

impl RefNamespace {
    /// The directory name of this role under `refs/`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Lineages => "lineages",
            Self::Pins => "pins",
        }
    }
}

/// The file name of `lineage`'s ref under `refs/lineages/` (and, with `.jsonl`, of its
/// `log/lineages/` cache): its
/// UTF-8 bytes, each byte outside `a-z`, `0-9`, `-` and `_` written as `%` and two lower-case hex
/// digits (#975). So `blake3:3f…` names `blake3%3a3f…`, `a_b` names `a_b`, and `a:b` names `a%3ab`.
///
/// The mapping is **injective**: `%` is itself encoded, so a name decodes back to exactly one
/// lineage, and distinct lineages never share a file. It stays injective on a case-insensitive
/// file system (APFS and NTFS by default), because upper-case letters are encoded (`Ab` names
/// `%41b`, `ab` names `ab`), and on one that normalizes Unicode, because every non-ASCII byte is
/// encoded. `.` is encoded too, so no name is `.`, `..` or hidden. Windows' reserved device
/// names (`con`, `nul`, …) are not encoded: Windows is not a supported target (its builds are
/// deferred, see `dist-workspace.toml`).
///
/// An empty lineage, or one whose file name would be longer than [`MAX_LINEAGE_FILE_NAME`]
/// bytes, is [`Error::InvalidRefName`]: a ref names a file, and a longer name cannot be created.
/// A product `id` (`blake3:` and 64 hex digits) names 73 bytes.
pub fn lineage_file_name(lineage: &str) -> Result<String> {
    ref_file_name(RefNamespace::Lineages, lineage)
}

/// Encode a ref name injectively in either namespace, with the same 249-byte bound.
/// Empty or overlong names return [`Error::InvalidRefName`] before any lock is taken.
/// The encoding is the one documented by [`lineage_file_name`]; no name is reserved.
pub fn ref_file_name(namespace: RefNamespace, lineage: &str) -> Result<String> {
    let invalid = |reason: String| Error::InvalidRefName {
        namespace: namespace.as_str().to_string(),
        name: lineage.to_string(),
        reason,
    };
    if lineage.is_empty() {
        return Err(invalid("it is empty".into()));
    }
    let name = tessera_core::names::file_name(lineage);
    if name.len() > MAX_LINEAGE_FILE_NAME {
        return Err(invalid(format!(
            "its ref file name is {} bytes, more than the {MAX_LINEAGE_FILE_NAME} a ref and its \
             log can be named with",
            name.len()
        )));
    }
    Ok(name)
}

/// A content-addressed repository rooted at a directory.
pub struct Repository {
    root: PathBuf,
}

/// The held writer lock of a repository: proof, by its presence, that the caller is the one
/// writer. Released when dropped (closing the file releases the `flock`).
struct WriterLock {
    _file: File,
}

/// One entry in a lineage's `log` cache — a committed version and the tip it superseded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEntry {
    /// The committed version's `manifest_hash`.
    pub manifest_hash: String,
    /// The lineage tip this commit advanced from (`None` for the first commit).
    pub parent: Option<String>,
}

/// A manifest the `gc` walk has reached but not read yet, and how it was reached (for the error
/// that names it).
struct Reached {
    address: String,
    via: usize,
    /// Ref and member claims must be checked even for an already visited target.
    expect: Option<ObjectKind>,
    ref_path: Option<PathBuf>,
}

/// Append-only traversal provenance: each step owns only its local label and a parent
/// index. Building a long history never copies its prefix. Paths are rendered only on error.
#[derive(Default)]
struct WalkPaths(Vec<PathStep>);

struct PathStep {
    parent: Option<usize>,
    label: PathLabel,
}

enum PathLabel {
    Version(String),
    Root(PathBuf),
    Member {
        collection: String,
        index: usize,
        reference: String,
    },
    Edge {
        object: String,
        role: String,
    },
}

impl WalkPaths {
    fn push(&mut self, parent: Option<usize>, label: PathLabel) -> usize {
        let index = self.0.len();
        self.0.push(PathStep { parent, label });
        index
    }

    fn render(&self, index: usize) -> String {
        let mut current = Some(index);
        let mut parts = Vec::new();
        while let Some(index) = current {
            let step = &self.0[index];
            current = step.parent;
            match &step.label {
                PathLabel::Version(address) => parts.push(format!("version {address}")),
                PathLabel::Root(path) => parts.push(format!("the tip of ref {}", path.display())),
                PathLabel::Member {
                    collection,
                    index,
                    reference,
                } => {
                    parts.push(format!("{collection} → members[{index}] ({reference})"));
                }
                PathLabel::Edge { object, role } => {
                    let mut hops = 1usize;
                    while let Some(parent) = current {
                        let previous = &self.0[parent];
                        if !matches!(previous.label, PathLabel::Edge { .. }) {
                            break;
                        }
                        hops += 1;
                        current = previous.parent;
                    }
                    let mut edge = format!("the '{role}' edge of {object}");
                    if hops > 1 {
                        // An edge may also reach a source file, so the count includes
                        // both version objects and non-version objects.
                        edge.push_str(&format!(
                            " ({} versions/objects across {hops} source edges)",
                            hops + 1
                        ));
                    }
                    parts.push(edge);
                }
            }
        }
        parts.reverse();
        parts.join(" → ")
    }
}

/// A checked version object. Non-version edge targets have no outgoing edges.
enum Version {
    Product(Box<Manifest>),
    Collection(Box<Collection>),
}

impl Version {
    fn kind(&self) -> ObjectKind {
        match self {
            Self::Product(_) => ObjectKind::Product,
            Self::Collection(_) => ObjectKind::Collection,
        }
    }
}

impl Repository {
    /// Create a layout-2 repository, or open an existing layout-2 repository.
    /// An unmarked directory may hold no ref, log or object entries. Every other layout
    /// is [`Error::UnsupportedRepositoryLayout`]; no conversion is attempted.
    /// Initialization and all subsequent writes use the same repository writer lock.
    /// Refusal is checked before creating that lock file, then validated again under it.
    /// The marker is atomically published and fsync'd before the namespace skeleton;
    /// after a crash, call `init` again to finish any missing directories. Before marker
    /// publication only lock and temporary files can remain; neither prevents retry.
    /// `open` may refuse an incomplete skeleton; it never finishes initialization.
    pub fn init(root: &Path) -> Result<Repository> {
        let repo = Repository {
            root: root.to_path_buf(),
        };
        // Refuse before creating even the coordination file. A concurrent initializer
        // publishes its durable marker before any namespace entries, so rechecking the
        // marker on an apparent refusal distinguishes its completed publication.
        if let Err(error) = repo.check_initial_layout() {
            repo.layout().map_err(|_| error)?;
        }
        create_dirs_durable(root, &mut Vec::new())?;
        drop(repo.lock_inner(true)?);
        Ok(repo)
    }

    /// Open an existing layout-2 repository without writing anything.
    /// Every other marker, including a missing marker in an empty directory, is refused.
    /// A layout-2 marker with a missing skeleton directory is an incomplete initialization:
    /// [`init`](Self::init) can finish it; `open` refuses it with an actionable error.
    /// Ref corruption is reported when a ref is read or by `gc`, so a corrupt lineage
    /// can still be removed with [`forget`](Self::forget).
    pub fn open(root: &Path) -> Result<Repository> {
        let repo = Repository {
            root: root.to_path_buf(),
        };
        repo.layout()?;
        for directory in ["objects", "refs/lineages", "refs/pins", "log/lineages"] {
            if !root.join(directory).is_dir() {
                return Err(Error::Invalid(format!(
                    "{} is not a complete tessera repository (no {directory}/ — run `tessera init`)",
                    root.display()
                )));
            }
        }
        Ok(repo)
    }

    fn unsupported_layout(&self, found: String) -> Error {
        Error::UnsupportedRepositoryLayout {
            root: self.root.clone(),
            found,
            required: "`tessera-repository 2` (typed, namespaced refs, ADR-0067)".into(),
        }
    }

    /// Read the marker exactly; an unmarked directory is never opened as a repository.
    fn layout(&self) -> Result<()> {
        match std::fs::read(self.root.join(LAYOUT)) {
            Ok(bytes) if bytes == LAYOUT_V2.as_bytes() => Ok(()),
            Ok(bytes) => Err(self.unsupported_layout(format!(
                "{:?} (its `{LAYOUT}` file)",
                String::from_utf8_lossy(&bytes)
            ))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(self.unsupported_layout(
                "no marker (possibly before #975; unmarked repositories are refused)".into(),
            )),
            Err(e) => Err(e.into()),
        }
    }

    /// Only `init` may write the first marker, and only with no repository entries.
    fn check_initial_layout(&self) -> Result<()> {
        match std::fs::symlink_metadata(self.root.join(LAYOUT)) {
            Ok(_) => self.layout(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                for directory in ["refs", "log", "objects"] {
                    if has_entries(&self.root.join(directory))? {
                        return Err(self.unsupported_layout(format!(
                            "no marker, entries in {directory}/ (possibly before #975)"
                        )));
                    }
                }
                Ok(())
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Take the one writer lock, validate the layout, then clear crashed-writer debris.
    fn lock(&self) -> Result<WriterLock> {
        self.lock_inner(false)
    }

    fn lock_inner(&self, initialize: bool) -> Result<WriterLock> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.root.join(LOCK))?;
        file.lock()?;
        if initialize {
            #[cfg(test)]
            crate::durable::seam::record(crate::durable::seam::Event::InitLayoutCheck);
            self.check_initial_layout()?;
        } else {
            self.layout()?;
        }
        let tmp = self.root.join(TMP);
        create_dirs_durable(&tmp, &mut Vec::new())?;
        for entry in std::fs::read_dir(&tmp)? {
            let path = entry?.path();
            if path.is_dir() {
                std::fs::remove_dir_all(&path)?;
            } else {
                std::fs::remove_file(&path)?;
            }
        }
        if initialize {
            if !self.root.join(LAYOUT).exists() {
                write_durable_via(
                    &tmp.join(LAYOUT),
                    &self.root.join(LAYOUT),
                    LAYOUT_V2.as_bytes(),
                )?;
            }
            // A previous initializer may have died just after the marker rename.
            // Fence that entry even when this initializer did not publish it.
            sync_dir(&self.root)?;
            // No namespace entry may become durable before the layout marker.
            for directory in ["objects", "refs/lineages", "refs/pins", "log/lineages"] {
                let path = self.root.join(directory);
                create_dirs_durable(&path, &mut Vec::new())?;
                // Also finish a directory created just before a prior init crashed.
                sync_dir(path.parent().expect("a skeleton directory has a parent"))?;
            }
            sync_dir(&self.root)?;
        }
        Ok(WriterLock { _file: file })
    }

    /// Map a `blake3:<hex>` address to its sharded on-disk path (`objects/blake3/<aa>/<rest>`).
    fn object_path(&self, address: &str) -> Result<PathBuf> {
        let (alg, hex) = address
            .split_once(':')
            .ok_or_else(|| Error::Invalid(format!("not a digest: '{address}'")))?;
        if hex.len() < 4 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::Invalid(format!("malformed digest '{address}'")));
        }
        Ok(self
            .root
            .join("objects")
            .join(alg)
            .join(&hex[..2])
            .join(&hex[2..]))
    }

    /// Store `bytes` at `address` under the writer lock, durably, unless an object whose bytes hash
    /// to `address` is already there. An object present at the address whose bytes do not (torn by
    /// a crash, truncated, edited) is replaced. The caller has checked that `bytes` hash to
    /// `address`.
    fn store(&self, _writer: &WriterLock, address: &str, bytes: &[u8]) -> Result<()> {
        let path = self.object_path(address)?;
        if self.object_intact(&path, address)? {
            return Ok(()); // content-addressed: the same bytes are already there
        }
        let dir = path.parent().expect("an object path has a shard directory");
        create_dirs_durable(dir, &mut Vec::new())?;
        let hex = address.split_once(':').map_or(address, |(_, hex)| hex);
        write_durable_via(&self.root.join(TMP).join(hex), &path, bytes)
    }

    /// Whether the file at `path` holds bytes that hash to `address` (`false` when it is absent).
    fn object_intact(&self, path: &Path, address: &str) -> Result<bool> {
        match File::open(path) {
            Ok(file) => {
                Ok(tessera_core::hash::digest_reader(std::io::BufReader::new(file))? == address)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    fn read_raw(&self, address: &str) -> Result<Vec<u8>> {
        Ok(std::fs::read(self.object_path(address)?)?)
    }

    /// `true` if a file is stored at this address. An **existence** test only: it does not read
    /// the bytes, so a torn object counts as present. [`read_object`](Self::read_object) checks
    /// them, and every write checks them before it skips an object.
    pub fn has_object(&self, address: &str) -> bool {
        self.object_path(address)
            .map(|p| p.exists())
            .unwrap_or(false)
    }

    /// Store a **block** object, verifying its bytes hash to `address` (rejects a digest mismatch
    /// before anything touches disk). Idempotent: an intact object at the address is left as-is, a
    /// torn one is replaced. Takes the writer lock and writes durably (see the module docs).
    pub fn write_object(&self, address: &str, bytes: &[u8]) -> Result<()> {
        check_digest(address, bytes)?;
        let writer = self.lock()?;
        self.store(&writer, address, bytes)
    }

    /// Read a **block** object, verifying its bytes still hash to `address` (catches on-disk
    /// corruption / tampering on access).
    pub fn read_object(&self, address: &str) -> Result<Vec<u8>> {
        self.layout()?;
        let bytes = self.read_raw(address)?;
        check_digest(address, &bytes)?;
        Ok(bytes)
    }

    /// Store a sealed manifest as an object **addressed by its `manifest_hash`**: the seal, which
    /// under ADR-0066 is the digest of the object's own bytes, the manifest's stored form
    /// ([`crate::stored::manifest`]), so the address attests the whole product and `b3sum` of the
    /// object reproduces it. Returns the `manifest_hash`. Takes the writer lock and writes durably;
    /// a torn object at the address is replaced.
    ///
    /// A manifest listing a block name twice ([`Error::DuplicateBlockName`], ADR-0063 §2a) or holding
    /// a chunk index without its block ([`Error::DanglingChunkIndex`], #720) is refused before
    /// anything is written: [`get_manifest`](Self::get_manifest) would refuse it.
    pub fn put_manifest(&self, m: &Manifest) -> Result<String> {
        let (mh, bytes) = stored_manifest(m)?;
        let writer = self.lock()?;
        self.store(&writer, &mh, &bytes)?;
        Ok(mh)
    }

    /// Read a manifest object back through [`Manifest::from_stored`], with the requested address
    /// as the seal: the bytes must be their own JCS form and hash to the address (an object filed
    /// under the wrong name, or edited, is [`Error::Integrity`] for `manifest_hash`), then `id` and
    /// `content_hash` are checked. An object from before ADR-0066 is refused (no migration, ADR-0066
    /// rule 10), by the fault its bytes show: [`Error::NonCanonical`] for a pretty object (written
    /// before #912) and [`Error::InBandSeal`] for a JCS one with the in-band key (written after it).
    /// Its address is its seal, so there is no missing seal member to report as
    /// [`Error::UnsupportedLayout`]; both messages say the stored bytes must be their own JCS form
    /// without the key. Re-ingest and re-commit such a product.
    pub fn get_manifest(&self, manifest_hash: &str) -> Result<Manifest> {
        self.layout()?;
        let bytes = self.read_raw(manifest_hash)?;
        require_object_kind(
            &self.object_path(manifest_hash)?,
            ObjectKind::Product,
            &bytes,
        )?;
        Manifest::from_stored(&bytes, manifest_hash)
    }

    /// Store a sealed collection at its `manifest_hash`, under the fenced writer.
    /// Uses the canonical stored bytes (ADR-0066); unsealed or edited values are refused.
    /// Member closure is checked by `commit_collection` and `gc`, not by this low-level storage primitive.
    /// The object is unreachable until a ref or another live collection pins it; a `gc`
    /// between this call and `set_ref` may reclaim it. Use [`commit_collection`](Self::commit_collection) to store members and advance a ref atomically.
    pub fn put_collection(&self, collection: &Collection) -> Result<String> {
        let (bytes, seal) = crate::stored::collection(collection)?;
        require_object_kind(&self.object_path(&seal)?, ObjectKind::Collection, &bytes)?;
        let writer = self.lock()?;
        self.store(&writer, &seal, &bytes)?;
        Ok(seal)
    }

    /// Read and verify a collection by its seal, including canonical bytes, identity and
    /// member MMR. A product is refused with `ObjectKindMismatch` rather than parsed as an
    /// empty collection. Resolving members belongs to the repository walk.
    pub fn get_collection(&self, manifest_hash: &str) -> Result<Collection> {
        self.layout()?;
        let bytes = self.read_raw(manifest_hash)?;
        require_object_kind(
            &self.object_path(manifest_hash)?,
            ObjectKind::Collection,
            &bytes,
        )?;
        Collection::from_stored(&bytes, manifest_hash)
    }

    fn ref_path(&self, lineage: &str) -> Result<PathBuf> {
        Ok(self
            .root
            .join("refs/lineages")
            .join(lineage_file_name(lineage)?))
    }

    /// The current tip (`manifest_hash`) of a lineage, or `None` if it has no commits yet (no ref
    /// file). A ref file that does not hold exactly `<kind> blake3:<64 lower-case hex> LF`
    /// is [`Error::CorruptRef`]: tessera never writes one (a ref
    /// is replaced by a rename), so it was torn or edited, and reading it as "no commits yet" would
    /// let a commit start the lineage over and `gc` reclaim its history.
    /// Kind is `product` or `collection`; a disagreement with the target object is
    /// [`Error::RefKindMismatch`]. Missing or torn targets are [`Error::RefTargetUnreadable`].
    /// Target bytes and seal are verified on every read.
    pub fn read_ref(&self, lineage: &str) -> Result<Option<String>> {
        let path = self.ref_path(lineage)?;
        self.layout()?;
        self.read_typed_ref(&path)
            .map(|record| record.map(|(_, address)| address))
    }

    /// Read and validate either namespace with the same grammar and object checks.
    fn read_typed_ref(&self, path: &Path) -> Result<Option<(ObjectKind, String)>> {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if !metadata.file_type().is_file() => {
                return Err(Error::CorruptRef {
                    path: path.to_path_buf(),
                    reason: "expected a regular ref file".into(),
                })
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        }
        match std::fs::read(path) {
            Ok(bytes) => {
                let (kind, address) = parse_ref(path, &bytes)?;
                let unreadable = |source| Error::RefTargetUnreadable {
                    path: path.to_path_buf(),
                    address: address.clone(),
                    source: Box::new(source),
                };
                let target = self.read_raw(&address).map_err(unreadable)?;
                let object = ObjectKind::of(&target).map_err(unreadable)?;
                if kind != object {
                    return Err(Error::RefKindMismatch {
                        path: path.to_path_buf(),
                        record: kind,
                        object,
                    });
                }
                read_version(&target, &address).map_err(unreadable)?;
                Ok(Some((kind, address)))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Advance a lineage's ref from `expected` to `new` (compare-and-set): under the writer lock,
    /// so no other writer can move the ref between the check and the write. A tip that moved out
    /// from under us (`current != expected`) is [`Error::RefMoved`] and the ref is left as it is, so
    /// a concurrent commit is never silently lost. `new` must be a `blake3:<64 lower-case hex>`
    /// address of a stored, readable product or collection. The kind is inferred from the
    /// object bytes; bare blocks and ambiguous objects are refused before the ref is written.
    /// An existing lineage keeps its recorded kind: a change is [`Error::LineageKindMismatch`].
    /// The ref is written durably (see the module docs).
    ///
    /// A low-level primitive: [`commit`](Self::commit) is the one atomic path from a manifest to a
    /// ref. An object stored by an earlier call ([`put_manifest`](Self::put_manifest)) is
    /// unreachable until this call lands, so a `gc` that runs in between reclaims it, and this call
    /// then refuses `new` as not stored.
    pub fn set_ref(&self, lineage: &str, expected: Option<&str>, new: &str) -> Result<()> {
        lineage_file_name(lineage)?;
        let writer = self.lock()?;
        self.advance_ref(&writer, lineage, expected, new)
    }

    /// [`set_ref`](Self::set_ref) under a held lock.
    fn advance_ref(
        &self,
        _writer: &WriterLock,
        lineage: &str,
        expected: Option<&str>,
        new: &str,
    ) -> Result<()> {
        tessera_core::stored::parse_seal(new.as_bytes(), "the new ref")?;
        if !self.has_object(new) {
            return Err(Error::Invalid(format!(
                "ref '{lineage}': {new} is not stored in this repository"
            )));
        }
        self.check_ref(lineage, expected)?;
        let bytes = self.read_raw(new)?;
        let kind = ObjectKind::of(&bytes)?;
        read_version(&bytes, new)?;
        self.check_lineage_kind(lineage, kind)?;
        let refs = self.root.join("refs/lineages");
        create_dirs_durable(&refs, &mut Vec::new())?;
        let tmp = self
            .root
            .join(TMP)
            .join(format!("ref-{}", lineage_file_name(lineage)?));
        write_durable_via(
            &tmp,
            &self.ref_path(lineage)?,
            format!("{} {new}\n", kind_name(kind)).as_bytes(),
        )
    }

    /// [`Error::RefMoved`] unless `lineage`'s tip is `expected`. The caller holds the writer lock.
    fn check_ref(&self, lineage: &str, expected: Option<&str>) -> Result<()> {
        let found = self.read_ref(lineage)?;
        if found.as_deref() != expected {
            return Err(Error::RefMoved {
                lineage: lineage.to_string(),
                expected: expected.map(str::to_string),
                found,
            });
        }
        Ok(())
    }

    /// The caller holds the writer lock and has checked CAS. Check the recorded kind,
    /// not the new object's inferred kind alone, before any storage or ref publication.
    fn check_lineage_kind(&self, lineage: &str, kind: ObjectKind) -> Result<()> {
        let path = self.ref_path(lineage)?;
        match std::fs::read(&path) {
            Ok(bytes) => {
                let (tip_kind, _) = parse_ref(&path, &bytes)?;
                if tip_kind != kind {
                    return Err(Error::LineageKindMismatch {
                        lineage: lineage.into(),
                        tip_kind,
                    });
                }
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    fn log_path(&self, lineage: &str) -> Result<PathBuf> {
        Ok(self
            .root
            .join("log/lineages")
            .join(format!("{}.jsonl", lineage_file_name(lineage)?)))
    }

    fn append_log(&self, lineage: &str, manifest_hash: &str, parent: Option<&str>) -> Result<()> {
        let p = self.log_path(lineage)?;
        if let Some(parent_dir) = p.parent() {
            std::fs::create_dir_all(parent_dir)?;
        }
        let entry = LogEntry {
            manifest_hash: manifest_hash.to_string(),
            parent: parent.map(str::to_string),
        };
        let mut line = serde_json::to_string(&entry)?;
        line.push('\n');
        let mut f = OpenOptions::new().create(true).append(true).open(&p)?;
        f.write_all(line.as_bytes())?;
        Ok(())
    }

    /// Read a lineage's `log` cache, oldest commit first (empty if the lineage has no commits).
    pub fn log(&self, lineage: &str) -> Result<Vec<LogEntry>> {
        let path = self.log_path(lineage)?;
        self.layout()?;
        match std::fs::read_to_string(path) {
            Ok(s) => s
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(|l| serde_json::from_str(l).map_err(Error::from))
                .collect(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e.into()),
        }
    }

    /// Commit a sealed product as a new version on `lineage`, built on the tip `expected` (`None`
    /// for a lineage's first version): store any supplied block payloads content-addressed (reusing
    /// — never rewriting — intact blocks already present), confirm every block the manifest
    /// references is in the store, store the manifest object, advance the ref from `expected`, and
    /// append the `log` cache. Returns the new `manifest_hash`.
    ///
    /// The whole commit holds the writer lock (see the module docs), and it checks the tip first: a
    /// ref that is not at `expected` is [`Error::RefMoved`], before anything is stored. `expected`
    /// is the tip the caller read and built the manifest on, so a commit that lost a race to another
    /// writer is refused, never stacked on top of it with a history that skips it.
    ///
    /// With `expected = Some(parent)`, the manifest must carry exactly one `supersedes` edge, whose
    /// `content_hash` is `parent` ([`Error::SupersedesMismatch`]): the ref then always reaches every
    /// earlier version. A first version (`expected = None`) is not checked: it may supersede a
    /// version held elsewhere (a `.tsra` from `tessera seal` keeps its history edges).
    ///
    /// A metadata-only version supplies no new payloads: every block is already stored from the
    /// parent, so only the manifest object is written. A block that is not supplied must be present,
    /// which is an existence test ([`has_object`](Self::has_object)): reading every unchanged block
    /// back would make a one-field edit cost the dataset.
    ///
    /// A manifest listing a block name twice ([`Error::DuplicateBlockName`], ADR-0063 §2a) or holding
    /// a chunk index without its block ([`Error::DanglingChunkIndex`], #720) is refused before any
    /// object is stored or the ref moves, so each payload resolves to exactly one block reference and
    /// no version holds an index a later block of that name would inherit. So is a payload whose
    /// bytes do not hash to its block's digest ([`Error::Integrity`]).
    pub fn commit(
        &self,
        lineage: &str,
        expected: Option<&str>,
        manifest: &Manifest,
        payloads: &[BlockPayload],
    ) -> Result<String> {
        lineage_file_name(lineage)?;
        let (mh, manifest_bytes) = stored_manifest(manifest)?;
        if let Some(parent) = expected {
            check_supersedes(lineage, parent, manifest)?;
        }
        // Names are unique (`stored_manifest` checked), so this map holds every block, one entry per
        // name.
        let by_name: BTreeMap<&BlockName, &BlockRef> =
            manifest.blocks.iter().map(|b| (&b.name, b)).collect();
        let mut supplied = Vec::with_capacity(payloads.len());
        for p in payloads {
            let br = by_name.get(&p.name).copied().ok_or_else(|| {
                Error::Invalid(format!(
                    "commit: payload '{}' has no matching block in the manifest",
                    p.name
                ))
            })?;
            let digest = block_digest(br)?;
            check_digest(digest, &p.bytes)?;
            supplied.push((digest, &p.bytes));
        }

        let writer = self.lock()?;
        self.check_ref(lineage, expected)?;
        self.check_lineage_kind(lineage, ObjectKind::Product)?;

        // 1. Store supplied payloads, content-addressed (skip any already intact = the dedup win).
        for (digest, bytes) in supplied {
            self.store(&writer, digest, bytes)?;
        }

        // 2. Every referenced block must now exist (supplied just now, or shared from a prior version).
        for b in &manifest.blocks {
            let digest = block_digest(b)?;
            if !self.has_object(digest) {
                return Err(Error::MissingProductBlock {
                    name: b.name.to_string(),
                    digest: digest.to_string(),
                });
            }
        }

        // 3. Store the manifest object, advance the ref (CAS, still under the lock), append the log
        //    cache.
        self.store(&writer, &mh, &manifest_bytes)?;
        self.advance_ref(&writer, lineage, expected, &mh)?;
        self.append_log(lineage, &mh, expected)?;
        Ok(mh)
    }

    /// Delete a lineage's ref + `log` cache, under the writer lock. Returns `true` if the lineage
    /// existed. The version objects are left in place — they may be shared with other lineages; run
    /// [`gc`](Self::gc) to reclaim whatever is now unreachable. A corrupt ref is deleted like any
    /// other: this is the way past an [`Error::CorruptRef`] that cannot be restored.
    pub fn forget(&self, lineage: &str) -> Result<bool> {
        lineage_file_name(lineage)?;
        let _writer = self.lock()?;
        let mut existed = false;
        match remove_file(&self.ref_path(lineage)?) {
            Ok(()) => {
                existed = true;
                sync_dir(&self.root.join("refs/lineages"))?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        let lp = self.log_path(lineage)?;
        if lp.exists() {
            std::fs::remove_file(&lp)?;
        }
        Ok(existed)
    }

    /// All roots, from both namespaces. Reject stray entries before any gc sweep.
    fn all_ref_tips(&self) -> Result<Vec<(PathBuf, ObjectKind, String)>> {
        let mut tips = Vec::new();
        for entry in std::fs::read_dir(self.root.join("refs"))? {
            let entry = entry?;
            let path = entry.path();
            if !entry.file_type()?.is_dir()
                || !matches!(entry.file_name().to_str(), Some("lineages" | "pins"))
            {
                return Err(Error::CorruptRef {
                    path,
                    reason: "expected only the lineages/ and pins/ namespace directories".into(),
                });
            }
            for record in std::fs::read_dir(&path)? {
                let record = record?;
                let path = record.path();
                if !record.file_type()?.is_file() {
                    return Err(Error::CorruptRef {
                        path,
                        reason: "expected a regular ref file".into(),
                    });
                }
                let bytes = std::fs::read(&path)?;
                let (kind, address) = parse_ref(&path, &bytes)?;
                tips.push((path, kind, address));
            }
        }
        Ok(tips)
    }

    /// Every stored object as `(address, path, size)` by walking `objects/<alg>/<aa>/<rest>`.
    fn all_objects(&self) -> Result<Vec<(String, PathBuf, u64)>> {
        let mut out = Vec::new();
        let objects = self.root.join("objects");
        let algs = match std::fs::read_dir(&objects) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(e.into()),
        };
        for alg_e in algs {
            let alg_e = alg_e?;
            if !alg_e.path().is_dir() {
                continue;
            }
            let alg = alg_e.file_name().to_string_lossy().into_owned();
            for aa_e in std::fs::read_dir(alg_e.path())? {
                let aa_e = aa_e?;
                if !aa_e.path().is_dir() {
                    continue;
                }
                let aa = aa_e.file_name().to_string_lossy().into_owned();
                for f_e in std::fs::read_dir(aa_e.path())? {
                    let f_e = f_e?;
                    if !f_e.path().is_file() {
                        continue;
                    }
                    let rest = f_e.file_name().to_string_lossy().into_owned();
                    out.push((
                        format!("{alg}:{aa}{rest}"),
                        f_e.path(),
                        f_e.metadata()?.len(),
                    ));
                }
            }
        }
        Ok(out)
    }

    /// Reclaim objects unreachable from either ref namespace, under the fenced writer.
    /// Products mark their blocks; collections recursively mark their members, including
    /// collections of collections. Both follow all stored `sources` edges, regardless of role.
    /// Intact non-version objects reached through edges are kept without further traversal.
    /// Missing edge targets are external and are not followed (ADR-0067 D3.1).
    ///
    /// **Members are closed.** A missing, unreadable, torn or wrong-kind member aborts with
    /// [`Error::GcAborted`] naming the full root-ref → collection → member-index path.
    /// Unreadable roots or stored edge targets, ambiguous version objects and record/object
    /// kind mismatches also abort. Nothing is swept until the entire mark succeeds.
    ///
    /// The walk uses a heap stack with no depth cutoff. Each distinct version is read once;
    /// every incoming kind claim is checked, including claims about shared targets. Sealed
    /// hash pins cannot form cycles without a hash preimage; editing bytes breaks the seal.
    /// Traversal provenance uses parent indices, with constant-size bookkeeping per edge.
    /// Paths are rendered only on error; consecutive source edges are summarized by the
    /// number of reached versions/objects, while every member step remains visible.
    /// Product block payloads are marked without reading them, as before. Cost grows with
    /// the reachable versions plus the listing of all stored objects (ADR-0067 D7).
    pub fn gc(&self) -> Result<GcReport> {
        let _writer = self.lock()?;
        #[cfg(test)]
        crate::durable::seam::record(crate::durable::seam::Event::GcWalk);
        // `reachable` decides what the sweep keeps; `walked` what the walk has read. An address can
        // be both a block of one version and the manifest of another (a blob block holding a
        // manifest's bytes), so marking it as a block must not stop the walk from reading it.
        let mut reachable: BTreeSet<String> = BTreeSet::new();
        let mut walked: BTreeMap<String, Option<ObjectKind>> = BTreeMap::new();
        let mut stack: Vec<Reached> = Vec::new();
        let mut paths = WalkPaths::default();
        for (path, kind, address) in self.all_ref_tips().map_err(|e| gc_aborted("a ref", e))? {
            stack.push(Reached {
                address,
                via: paths.push(None, PathLabel::Root(path.clone())),
                expect: Some(kind),
                ref_path: Some(path),
            });
        }
        while let Some(reached) = stack.pop() {
            if let Some(kind) = walked.get(&reached.address) {
                // A shared target can be reached first through an edge or a differently
                // typed member. Deduplication must never hide a conflicting claim.
                check_reached_kind(&reached, *kind, &paths)
                    .map_err(|e| gc_aborted(paths.render(reached.via), e))?;
                continue;
            }
            reachable.insert(reached.address.clone());
            let version = self.walk_read(&reached, &paths)?;
            walked.insert(reached.address.clone(), version.as_ref().map(Version::kind));
            let sources = match version {
                None => continue,
                Some(Version::Product(m)) => {
                    for b in &m.blocks {
                        if let Some(d) = &b.digest {
                            reachable.insert(d.clone());
                        }
                    }
                    m.sources
                }
                Some(Version::Collection(c)) => {
                    for (index, member) in c.members.iter().enumerate() {
                        let kind = match member.kind {
                            MemberKind::Product => ObjectKind::Product,
                            MemberKind::Collection => ObjectKind::Collection,
                            _ => {
                                return Err(gc_aborted(
                                    paths.render(reached.via),
                                    Error::Invalid("unsupported member kind".into()),
                                ))
                            }
                        };
                        stack.push(Reached {
                            address: member.manifest_hash.clone(),
                            via: paths.push(
                                Some(reached.via),
                                PathLabel::Member {
                                    collection: reached.address.clone(),
                                    index,
                                    reference: member.reference.clone(),
                                },
                            ),
                            expect: Some(kind),
                            ref_path: None,
                        });
                    }
                    c.sources
                }
            };
            for source in sources {
                if let Some(address) = source.content_hash {
                    if self.has_object(&address) {
                        stack.push(Reached {
                            address,
                            via: paths.push(
                                Some(reached.via),
                                PathLabel::Edge {
                                    object: reached.address.clone(),
                                    role: source.role,
                                },
                            ),
                            expect: None,
                            ref_path: None,
                        });
                    }
                }
            }
        }
        let mut report = GcReport::default();
        for (addr, path, size) in self.all_objects()? {
            report.scanned += 1;
            if reachable.contains(&addr) {
                report.kept += 1;
            } else {
                remove_file(&path)?;
                report.reclaimed += 1;
                report.bytes_reclaimed += size;
            }
        }
        Ok(report)
    }

    /// Read a reached product or collection, or an intact non-version edge target.
    fn walk_read(&self, reached: &Reached, paths: &WalkPaths) -> Result<Option<Version>> {
        let what = || format!("{} ({})", reached.address, paths.render(reached.via));
        let read = || {
            let bytes = self.read_raw(&reached.address)?;
            if reached.expect.is_some() || manifest_shaped(&bytes) {
                let kind = ObjectKind::of(&bytes)?;
                check_reached_kind(reached, Some(kind), paths)?;
                return read_version(&bytes, &reached.address).map(Some);
            }
            check_digest(&reached.address, &bytes)?;
            Ok(None)
        };
        read().map_err(|e| gc_aborted(what(), e))
    }
}

/// A sealed manifest's `manifest_hash` and stored bytes, checked as `put_manifest` and `commit`
/// require: sealed, block names unique and chunk indexes resolving (else the typed errors). The
/// seal is the `manifest_hash` (`stored::manifest` verifies it).
fn stored_manifest(m: &Manifest) -> Result<(String, Vec<u8>)> {
    if m.manifest_hash.is_none() {
        return Err(Error::Invalid("the manifest is not sealed".into()));
    }
    check_block_refs(&m.blocks)?;
    let (bytes, seal) = crate::stored::manifest(m)?;
    require_object_kind(Path::new(&seal), ObjectKind::Product, &bytes)?;
    Ok((seal, bytes))
}

/// A block's digest, which a block committed to a repository must carry.
fn block_digest(b: &BlockRef) -> Result<&str> {
    b.digest
        .as_deref()
        .ok_or_else(|| Error::Invalid(format!("commit: block '{}' has no digest", b.name)))
}

/// [`Error::Integrity`] unless `bytes` hash to `address`.
fn check_digest(address: &str, bytes: &[u8]) -> Result<()> {
    let actual = tessera_core::hash::digest(bytes);
    if actual != address {
        return Err(Error::Integrity {
            what: "object",
            expected: address.into(),
            actual,
        });
    }
    Ok(())
}

/// [`Error::SupersedesMismatch`] unless `m` carries exactly one `supersedes` edge, to `parent`.
fn check_supersedes(lineage: &str, parent: &str, m: &Manifest) -> Result<()> {
    collection_commit::check_source_supersedes(lineage, parent, &m.sources)
}

/// Parse the one canonical spelling shared by both ref namespaces.
fn parse_ref(path: &Path, bytes: &[u8]) -> Result<(ObjectKind, String)> {
    let corrupt = || Error::CorruptRef {
        path: path.to_path_buf(),
        reason: format!(
            "expected exactly `<product|collection> blake3:<64 lower-case hex>\\n`, holds {:?}",
            String::from_utf8_lossy(bytes)
        ),
    };
    let (kind, rest) = if let Some(rest) = bytes.strip_prefix(b"product ") {
        (ObjectKind::Product, rest)
    } else if let Some(rest) = bytes.strip_prefix(b"collection ") {
        (ObjectKind::Collection, rest)
    } else {
        return Err(corrupt());
    };
    let address = rest.strip_suffix(b"\n").ok_or_else(corrupt)?;
    let address = tessera_core::stored::parse_seal(address, "ref").map_err(|_| corrupt())?;
    Ok((kind, address.to_string()))
}

fn kind_name(kind: ObjectKind) -> &'static str {
    match kind {
        ObjectKind::Product => "product",
        ObjectKind::Collection => "collection",
        _ => unreachable!("only supported object kinds are constructed"),
    }
}

fn require_object_kind(path: &Path, expected: ObjectKind, bytes: &[u8]) -> Result<()> {
    let object = ObjectKind::of(bytes)?;
    if expected != object {
        return Err(Error::ObjectKindMismatch {
            path: path.to_path_buf(),
            expected,
            object,
        });
    }
    Ok(())
}

fn check_reached_kind(
    reached: &Reached,
    object: Option<ObjectKind>,
    paths: &WalkPaths,
) -> Result<()> {
    let Some(expected) = reached.expect else {
        return Ok(());
    };
    let object = object.ok_or(Error::AmbiguousObjectKind)?;
    if expected != object {
        if let Some(path) = &reached.ref_path {
            return Err(Error::RefKindMismatch {
                path: path.clone(),
                record: expected,
                object,
            });
        }
        return Err(Error::MemberKindMismatch {
            path: paths.render(reached.via),
            expected,
            object,
        });
    }
    Ok(())
}

fn read_version(bytes: &[u8], address: &str) -> Result<Version> {
    match ObjectKind::of(bytes)? {
        ObjectKind::Product => Manifest::from_stored(bytes, address)
            .map(Box::new)
            .map(Version::Product),
        ObjectKind::Collection => Collection::from_stored(bytes, address)
            .map(Box::new)
            .map(Version::Collection),
        _ => Err(Error::AmbiguousObjectKind),
    }
}

/// Whether `bytes` are a JSON object with a `tessera_version` member: a manifest, readable or not,
/// as opposed to another object an edge can pin (a stored source file).
fn manifest_shaped(bytes: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Map<String, serde_json::Value>>(bytes)
        .is_ok_and(|o| o.contains_key("tessera_version"))
}

fn gc_aborted(what: impl Into<String>, source: Error) -> Error {
    Error::GcAborted {
        what: what.into(),
        source: Box::new(source),
    }
}

/// Whether the directory `dir` holds any entry (`false` if it does not exist).
fn has_entries(dir: &Path) -> Result<bool> {
    match std::fs::read_dir(dir) {
        Ok(mut rd) => Ok(rd.next().transpose()?.is_some()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}

/// Counts from a [`Repository::gc`] sweep.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GcReport {
    /// Objects examined.
    pub scanned: usize,
    /// Objects kept (reachable).
    pub kept: usize,
    /// Objects deleted (unreachable).
    pub reclaimed: usize,
    /// Bytes freed by the reclaimed objects.
    pub bytes_reclaimed: u64,
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader};
    use std::process::{Child, Command, Stdio};

    use super::*;
    use crate::durable::seam;
    use crate::table::table_block;
    use crate::ColumnData;
    use tessera_core::block::table::{Column, TableSpec};
    use tessera_core::provenance::Source;
    use tessera_core::ProductBuilder;

    mod pins_tests {
        include!("repo/pins_tests.rs");
    }

    mod collections_tests {
        include!("repo/collections_tests.rs");
    }

    mod commit_collection_tests {
        include!("repo/commit_collection_tests.rs");
    }

    const L: &str = "study";

    /// A sealed 1-block product: a table block of the values `x` and a `rev` metadata field, with a
    /// `supersedes` edge to `parent`. Deterministic, so a parent process and a child build the same
    /// version; versions of the same `x` share their block (the metadata-only-edit fixture).
    fn version(rev: i64, parent: Option<&str>, x: &[u32]) -> (Manifest, Vec<BlockPayload>) {
        let spec = TableSpec {
            columns: vec![Column {
                name: "x".into(),
                dtype: "u4".into(),
                codec: None,
                ..Default::default()
            }],
            rows: x.len() as u64,
            row_index: None,
            partition: None,
            edition: None,
            codec_profile: None,
        };
        let data = vec![("x".into(), ColumnData::U32(x.to_vec()))];
        let (block_ref, payload) = table_block("events", &spec, &data).unwrap();
        let mut b = ProductBuilder::new("listmode", "DP", "d", "2024-01-01T00:00:00Z");
        b.add_block_ref(block_ref);
        b.with_field("rev", serde_json::json!(rev));
        if let Some(p) = parent {
            b.add_source(Source::new(SUPERSEDES, p).with_content_hash(p));
        }
        (b.seal().unwrap(), vec![payload])
    }

    /// Version `rev` of the fixture's shared block, on `parent`.
    fn sealed_on(rev: i64, parent: Option<&str>) -> (Manifest, Vec<BlockPayload>) {
        version(rev, parent, &[1, 2, 3])
    }

    /// The version the kill-point children commit: a new block on the first version of `L`.
    fn second(v0: &str) -> (Manifest, Vec<BlockPayload>) {
        version(1, Some(v0), &[4, 5, 6, 7])
    }

    /// A repository at `root` holding the first version of `L`; returns its `manifest_hash`.
    fn with_first_version(root: &Path) -> (Repository, String) {
        let repo = Repository::init(root).unwrap();
        let (m, p) = sealed_on(0, None);
        let mh = repo.commit(L, None, &m, &p).unwrap();
        (repo, mh)
    }

    fn count_objects(root: &Path) -> usize {
        objects(root).len()
    }

    /// Every object file under `root/objects`, with its bytes: the store, to compare before and
    /// after.
    fn objects(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        fn rec(d: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
            if let Ok(rd) = std::fs::read_dir(d) {
                for e in rd.flatten() {
                    let p = e.path();
                    if p.is_dir() {
                        rec(&p, out);
                    } else {
                        out.insert(p.clone(), std::fs::read(&p).unwrap());
                    }
                }
            }
        }
        let mut out = BTreeMap::new();
        rec(&root.join("objects"), &mut out);
        out
    }

    /// The versions `tip` reaches by `supersedes`, tip first, each read back by hash: the manifest
    /// through `get_manifest` and every block through `read_object`.
    fn reaches_intact(repo: &Repository, tip: &str) -> Vec<String> {
        let mut chain = Vec::new();
        let mut next = Some(tip.to_string());
        while let Some(mh) = next {
            let m = repo
                .get_manifest(&mh)
                .unwrap_or_else(|e| panic!("{mh}: {e}"));
            for b in &m.blocks {
                let d = b.digest.as_deref().unwrap();
                repo.read_object(d)
                    .unwrap_or_else(|e| panic!("block {} of {mh}: {e}", b.name));
            }
            next = m
                .sources
                .iter()
                .find(|s| s.role == SUPERSEDES)
                .and_then(|s| s.content_hash.clone());
            chain.push(mh);
        }
        chain
    }

    /// Write `bytes` at `address` and point `lineage`'s ref at it by hand, past every check: what a
    /// hostile or older writer leaves in a repository.
    fn plant(repo: &Repository, lineage: &str, address: &str, bytes: &[u8]) {
        let p = repo.object_path(address).unwrap();
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, bytes).unwrap();
        std::fs::write(
            repo.ref_path(lineage).unwrap(),
            format!("product {address}\n"),
        )
        .unwrap();
    }

    // ── Child processes ──────────────────────────────────────────────────────────────────────────
    //
    // A writer that races another, or dies at a step, has to be a process of its own: the lock is a
    // per-process `flock`, and a crash is a process that stops. The tests re-run this test binary
    // with `child_process` selected and the scenario in `CHILD`; the child talks over its stdin and
    // stdout, and a test waits on what it prints, never on time.

    const CHILD: &str = "TESSERA_REPO_TEST_CHILD";
    /// Marks a protocol line a child prints. libtest prints `test … ... ` with no newline before
    /// the test runs, so the first line a child prints shares a line with it.
    const SAY: &str = "@@946 ";

    /// Print one protocol line to the parent.
    fn say(line: &str) {
        println!("{SAY}{line}"); // guardrails-ok: stdout is the child-to-parent channel
        std::io::stdout().flush().unwrap();
    }

    /// The protocol line in `line`, if it carries one.
    fn heard(line: &str) -> Option<&str> {
        line.split_once(SAY).map(|(_, said)| said.trim_end())
    }

    /// This test binary as a child running `scenario` (tab-separated words), stdin and stdout piped.
    fn spawn_child(scenario: &[&str]) -> Child {
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "repo::tests::child_process",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD, scenario.join("\t"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap()
    }

    /// Read a child's output until it says `word`.
    fn wait_for(out: &mut impl BufRead, word: &str) {
        let mut line = String::new();
        loop {
            line.clear();
            assert!(
                out.read_line(&mut line).unwrap() > 0,
                "the child exited before saying {word}"
            );
            if heard(&line) == Some(word) {
                return;
            }
        }
    }

    /// Everything a child says from here to its exit (`OK …`, `MOVED`, `ERR …`, `DONE`), the
    /// libtest chatter dropped.
    fn results(out: &mut impl BufRead) -> Vec<String> {
        let mut all = String::new();
        out.read_to_string(&mut all).unwrap();
        all.lines().filter_map(heard).map(str::to_string).collect()
    }

    /// What a child printed for a commit's result.
    fn outcome(r: Result<String>) -> String {
        match r {
            Ok(mh) => format!("OK {mh}"),
            Err(Error::RefMoved { .. }) => "MOVED".into(),
            Err(e) => format!("ERR {e}"),
        }
    }

    /// The child side of every multi-process test, selected by `CHILD`. Not a test of its own: run
    /// without `CHILD` (a plain `--ignored` run), it does nothing.
    #[test]
    #[ignore = "a child process of the repository's multi-process tests"]
    fn child_process() {
        let Ok(scenario) = std::env::var(CHILD) else {
            return;
        };
        let words: Vec<&str> = scenario.split('\t').collect();
        if words[0] == "init-refuse" {
            let _kill = seam::arm(&seam::KILL_AFTER_NTH_STEP, 1);
            assert!(matches!(
                Repository::init(Path::new(words[1])),
                Err(Error::UnsupportedRepositoryLayout { .. })
            ));
            say("REFUSED");
            return;
        }
        if words[0] == "init-kill" {
            let _kill = seam::arm(&seam::KILL_AFTER_NTH_STEP, words[2].parse().unwrap());
            Repository::init(Path::new(words[1])).unwrap();
            say("DONE");
            return;
        }
        let repo = Repository::open(Path::new(words[1])).unwrap();
        let go = || {
            say("READY");
            std::io::stdin().read_line(&mut String::new()).unwrap();
        };
        match words[0] {
            "collection-writer" => {
                let tip = repo.read_ref(L).unwrap().unwrap();
                let parent = repo.get_collection(&tip).unwrap();
                let c = commit_collection_tests::racing_version(&parent, words[2].parse().unwrap());
                go();
                say(&outcome(repo.commit_collection(L, Some(&tip), &c, &[])));
            }
            // Build a version on the tip of `L`, wait for the go, commit it on that tip.
            "writer" => {
                let rev: i64 = words[2].parse().unwrap();
                let tip = repo.read_ref(L).unwrap().unwrap();
                let (m, _) = sealed_on(rev, Some(&tip));
                go();
                say(&outcome(repo.commit(L, Some(&tip), &m, &[])));
            }
            // Commit `second` on the first version of `L`, killed after the n-th file-system step.
            "kill" => {
                let n: usize = words[2].parse().unwrap();
                let tip = repo.read_ref(L).unwrap().unwrap();
                let (m, p) = second(&tip);
                let _kill = seam::arm(&seam::KILL_AFTER_NTH_STEP, n);
                repo.commit(L, Some(&tip), &m, &p).unwrap();
                say("DONE");
            }
            // After the go, commit `k` versions on lineage `words[2]`, each with a new block.
            "committer" => {
                let lineage = words[2];
                let k: u32 = words[3].parse().unwrap();
                let seed: u32 = words[4].parse().unwrap();
                go();
                let mut tip = None;
                for i in 0..k {
                    let (m, p) = version(i64::from(i), tip.as_deref(), &[seed, i, seed + i]);
                    let r = repo.commit(lineage, tip.as_deref(), &m, &p);
                    let line = outcome(r);
                    say(&line);
                    tip = line.strip_prefix("OK ").map(str::to_string);
                }
            }
            // After the go, run `gc` `n` times.
            "gc" => {
                let n: usize = words[2].parse().unwrap();
                go();
                for _ in 0..n {
                    if let Err(e) = repo.gc() {
                        say(&format!("ERR {e}"));
                    }
                }
                say("DONE");
            }
            other => panic!("unknown child scenario {other}"),
        }
    }

    /// Release every child at once (each printed `READY`), then collect each one's results.
    fn release(children: &mut [Child]) -> Vec<Vec<String>> {
        let mut outs: Vec<_> = children
            .iter_mut()
            .map(|c| BufReader::new(c.stdout.take().unwrap()))
            .collect();
        for out in &mut outs {
            wait_for(out, "READY");
        }
        for c in children.iter_mut() {
            c.stdin.take().unwrap().write_all(b"go\n").unwrap();
        }
        let results: Vec<_> = outs.iter_mut().map(results).collect();
        for c in children.iter_mut() {
            assert!(c.wait().unwrap().success());
        }
        results
    }

    // ── #946: one fenced writer ──────────────────────────────────────────────────────────────────

    /// #946 acceptance 1: N processes each build a version on the same tip, then commit at once.
    /// Exactly one commit lands; every other is refused with `RefMoved`; the one that returned Ok is
    /// the tip and reaches the first version, so no commit that reported success is lost. (Before
    /// the lock and the caller's parent, all N returned Ok and N-1 were unreachable.)
    #[test]
    fn concurrent_writers_lose_no_commit() {
        const N: usize = 8;
        let dir = tempfile::tempdir().unwrap();
        let (repo, v0) = with_first_version(dir.path());
        let root = dir.path().to_str().unwrap();
        let revs: Vec<String> = (1..=N).map(|i| i.to_string()).collect();
        let mut children: Vec<Child> = revs
            .iter()
            .map(|rev| spawn_child(&["writer", root, rev]))
            .collect();
        let results: Vec<String> = release(&mut children).into_iter().flatten().collect();
        assert_eq!(results.len(), N, "{results:?}");
        let ok: Vec<&str> = results
            .iter()
            .filter_map(|r| r.strip_prefix("OK "))
            .collect();
        let moved = results.iter().filter(|r| *r == "MOVED").count();
        assert_eq!((ok.len(), moved), (1, N - 1), "{results:?}");
        let tip = repo.read_ref(L).unwrap().unwrap();
        assert_eq!(tip, ok[0], "the commit that returned Ok is the tip");
        assert_eq!(reaches_intact(&repo, &tip), [tip.clone(), v0]);
        assert_eq!(
            repo.log(L).unwrap().len(),
            2,
            "one log line per landed commit"
        );
    }

    /// #946 finding 2: a version built on tip A, committed after the ref moved to B, is refused
    /// with `RefMoved` naming both, and nothing is stored. A manifest whose `supersedes` edge does
    /// not name the parent it is committed on is refused too.
    #[test]
    fn a_commit_built_on_a_moved_tip_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let (repo, a) = with_first_version(dir.path());
        let (on_a, _) = sealed_on(1, Some(&a));
        let (b_manifest, _) = sealed_on(2, Some(&a));
        let b = repo.commit(L, Some(&a), &b_manifest, &[]).unwrap();
        let before = objects(dir.path());

        let e = repo.commit(L, Some(&a), &on_a, &[]).unwrap_err();
        assert!(
            matches!(&e, Error::RefMoved { lineage, expected, found }
                if lineage == L && expected.as_deref() == Some(a.as_str())
                    && found.as_deref() == Some(b.as_str())),
            "{e}"
        );
        // Claiming B as the parent doesn't help: the version's history skips B.
        let e = repo.commit(L, Some(&b), &on_a, &[]).unwrap_err();
        assert!(
            matches!(&e, Error::SupersedesMismatch { expected, found, .. }
                if *expected == b && *found == [a.clone()]),
            "{e}"
        );
        // A first version on a lineage that has one is a moved ref too.
        let (fresh, p) = sealed_on(9, None);
        let e = repo.commit(L, None, &fresh, &p).unwrap_err();
        assert!(matches!(&e, Error::RefMoved { expected: None, .. }), "{e}");
        assert_eq!(repo.read_ref(L).unwrap().as_deref(), Some(b.as_str()));
        assert_eq!(
            objects(dir.path()),
            before,
            "a refused commit stores nothing"
        );
    }

    /// #946 acceptance 3: a process killed after each file-system step of a commit (a new block,
    /// its manifest, the ref) leaves the ref at the old version or the new one, and every object
    /// the ref reaches reads back by hash; the next writer commits and collects as if nothing had
    /// happened. Step `steps + 1` never fires, so that child completes the commit: the count of
    /// steps is the real one.
    #[test]
    fn a_crash_at_every_step_of_a_commit_leaves_the_old_or_the_new_version() {
        let steps = {
            let dir = tempfile::tempdir().unwrap();
            let (repo, v0) = with_first_version(dir.path());
            let (m, p) = second(&v0);
            let recording = seam::record_fs();
            repo.commit(L, Some(&v0), &m, &p).unwrap();
            recording.events().len()
        };
        assert!(steps >= 10, "a block, a manifest and a ref: {steps} steps");
        for n in 1..=steps + 1 {
            let dir = tempfile::tempdir().unwrap();
            let (_, v0) = with_first_version(dir.path());
            let (m, p) = second(&v0);
            let v1 = m.manifest_hash.clone().unwrap();
            let mut child = spawn_child(&["kill", dir.path().to_str().unwrap(), &n.to_string()]);
            let printed = results(&mut BufReader::new(child.stdout.take().unwrap()));
            let status = child.wait().unwrap();
            if n <= steps {
                assert!(
                    status.code().is_none() && printed.is_empty(),
                    "step {n}: the child was not killed: {status} {printed:?}"
                );
            } else {
                assert!(
                    status.success() && printed == ["DONE"],
                    "{status} {printed:?}"
                );
            }

            let repo = Repository::open(dir.path()).unwrap();
            let tip = repo.read_ref(L).unwrap().unwrap();
            assert!(tip == v0 || tip == v1, "step {n}: the ref names {tip}");
            reaches_intact(&repo, &tip);
            if tip == v0 {
                repo.commit(L, Some(&v0), &m, &p).unwrap();
            }
            repo.gc().unwrap();
            assert_eq!(
                reaches_intact(&repo, &v1),
                [v1.clone(), v0.clone()],
                "step {n}"
            );
            assert_eq!(
                std::fs::read_dir(dir.path().join(TMP)).unwrap().count(),
                0,
                "step {n}: the next writer clears the debris"
            );
        }
    }

    /// #946 durability order: replayed against what a power loss could undo (#695's `Replay`), at
    /// the rename that makes a ref name a version, every object of that version (its new block and
    /// its manifest, bytes and entries, and the shard directories made for them) is already
    /// durable, and the ref is durable when the commit returns. Recorded from `init` on.
    #[test]
    fn every_object_a_ref_names_is_durable_before_it() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        let root = base.join("repo");
        let recording = seam::record_fs();
        let (repo, v0) = with_first_version(&root);
        let (m, p) = second(&v0);
        let v1 = repo.commit(L, Some(&v0), &m, &p).unwrap();
        let events = recording.events();

        let ref_path = repo.ref_path(L).unwrap();
        let mut replay = seam::Replay::default();
        let mut advances = Vec::new();
        for event in &events {
            if let seam::Event::Rename { to, .. } = event {
                if *to == ref_path {
                    let tip = if advances.is_empty() { &v0 } else { &v1 };
                    let m = repo.get_manifest(tip).unwrap();
                    let mut paths = vec![repo.object_path(tip).unwrap()];
                    for b in &m.blocks {
                        paths.push(repo.object_path(b.digest.as_deref().unwrap()).unwrap());
                    }
                    let undoable: Vec<String> = paths
                        .iter()
                        .flat_map(|p| replay.undoable(base, p))
                        .collect();
                    assert!(
                        undoable.is_empty(),
                        "the ref moves to {tip} while a power loss could still undo {undoable:#?}"
                    );
                    advances.push(tip.clone());
                }
            }
            replay.apply(event);
        }
        assert_eq!(advances, [v0, v1], "one ref rename per commit");
        assert_eq!(replay.undoable(base, &ref_path), Vec::<String>::new());
    }

    /// #946: the writer lock is held at every file-system step of a commit: another open of the lock
    /// file can't take it (what a concurrent `gc` or commit would wait on).
    #[test]
    fn a_commit_holds_the_writer_lock_at_every_step() {
        let dir = tempfile::tempdir().unwrap();
        let (repo, v0) = with_first_version(dir.path());
        let lock = dir.path().join(LOCK);
        let steps = std::rc::Rc::new(std::cell::Cell::new(0));
        let seen = steps.clone();
        let watching = seam::watch(move |event| {
            let other = File::open(&lock).unwrap();
            assert!(
                matches!(other.try_lock(), Err(std::fs::TryLockError::WouldBlock)),
                "the writer lock is free at {event:?}"
            );
            seen.set(seen.get() + 1);
        });
        let (m, p) = second(&v0);
        repo.commit(L, Some(&v0), &m, &p).unwrap();
        assert!(steps.get() >= 10, "{} steps watched", steps.get());
        drop(watching);
        File::open(dir.path().join(LOCK))
            .unwrap()
            .try_lock()
            .expect("the lock is released when the commit returns");
    }

    /// #946: `gc` holds the writer lock for its whole run: from before it reads the first ref to its
    /// last deletion, so no commit can advance a ref between the walk and the sweep, or store
    /// objects the sweep would take for garbage.
    #[test]
    fn gc_holds_the_writer_lock_for_its_whole_run() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        for (lineage, rev) in [("a", 1), ("b", 2)] {
            let (m, p) = version(rev, None, &[rev as u32]);
            repo.commit(lineage, None, &m, &p).unwrap();
        }
        assert!(repo.forget("a").unwrap());
        let lock = dir.path().join(LOCK);
        let steps = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let seen = steps.clone();
        let watching = seam::watch(move |event| {
            let other = File::open(&lock).unwrap();
            assert!(
                matches!(other.try_lock(), Err(std::fs::TryLockError::WouldBlock)),
                "the writer lock is free at {event:?}"
            );
            seen.borrow_mut().push(event.clone());
        });
        let report = repo.gc().unwrap();
        drop(watching);
        let steps = steps.borrow();
        assert_eq!(steps.first(), Some(&seam::Event::GcWalk), "{steps:?}");
        let removed = steps
            .iter()
            .filter(|e| matches!(e, seam::Event::RemoveFile(_)))
            .count();
        assert_eq!(
            (report.reclaimed, removed),
            (2, 2),
            "a's block and manifest"
        );
    }

    /// #946 acceptance 5: `gc` processes running while committer processes commit, each committer
    /// adding a new block per version: every commit that returned Ok is in its lineage's history
    /// with every object intact. The lock keeps `gc` out of the window between storing a commit's
    /// objects and the ref advance that makes them reachable. That window is short, so this run
    /// alone rarely catches a missing lock (it passed with `gc`'s lock removed); the deterministic
    /// proof is the pair `a_commit_holds_the_writer_lock_at_every_step` and
    /// `gc_holds_the_writer_lock_for_its_whole_run`, each of which fails without its lock.
    #[test]
    fn gc_running_beside_commits_keeps_every_committed_object() {
        const COMMITS: &str = "6";
        let dir = tempfile::tempdir().unwrap();
        let _ = Repository::init(dir.path()).unwrap();
        let root = dir.path().to_str().unwrap();
        let lineages = ["a", "b", "c"];
        let mut children: Vec<Child> = lineages
            .iter()
            .enumerate()
            .map(|(i, l)| spawn_child(&["committer", root, l, COMMITS, &(100 * i + 7).to_string()]))
            .collect();
        children.push(spawn_child(&["gc", root, "20"]));
        children.push(spawn_child(&["gc", root, "20"]));
        let results = release(&mut children);
        let repo = Repository::open(dir.path()).unwrap();
        for (l, r) in lineages.iter().zip(&results) {
            assert_eq!(r.len(), 6, "{l}: {r:?}");
            let ok: Vec<&str> = r.iter().filter_map(|x| x.strip_prefix("OK ")).collect();
            assert_eq!(ok.len(), 6, "{l}: {r:?}");
            let tip = repo.read_ref(l).unwrap().unwrap();
            let chain = reaches_intact(&repo, &tip);
            let mut want: Vec<String> = ok.iter().map(|s| s.to_string()).collect();
            want.reverse();
            assert_eq!(chain, want, "{l}");
        }
        for r in &results[lineages.len()..] {
            assert_eq!(r, &["DONE"], "gc: {r:?}");
        }
        repo.gc().unwrap();
        for l in lineages {
            reaches_intact(&repo, &repo.read_ref(l).unwrap().unwrap());
        }
    }

    /// #946 acceptance 4: an object torn at its address (a crash's truncated write, before #946's
    /// rename) is detected by hash and replaced by the next write of the same bytes: a block through
    /// `write_object` and through a `commit` that supplies it, a manifest through `put_manifest`.
    #[test]
    fn a_torn_object_is_replaced_by_the_next_write_of_its_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let (m, p) = sealed_on(0, None);
        let block = m.blocks[0].digest.clone().unwrap();
        let mh = m.manifest_hash.clone().unwrap();
        let tear = |address: &str| {
            let path = repo.object_path(address).unwrap();
            let bytes = std::fs::read(&path).unwrap();
            std::fs::write(&path, &bytes[..bytes.len() / 2]).unwrap();
        };

        repo.write_object(&block, &p[0].bytes).unwrap();
        tear(&block);
        assert!(matches!(
            repo.read_object(&block),
            Err(Error::Integrity { .. })
        ));
        repo.write_object(&block, &p[0].bytes).unwrap();
        assert_eq!(repo.read_object(&block).unwrap(), p[0].bytes);

        tear(&block);
        repo.commit(L, None, &m, &p).unwrap();
        assert_eq!(
            repo.read_object(&block).unwrap(),
            p[0].bytes,
            "commit repairs it"
        );

        tear(&mh);
        assert!(repo.get_manifest(&mh).is_err());
        repo.put_manifest(&m).unwrap();
        assert_eq!(repo.get_manifest(&mh).unwrap().manifest_hash, Some(mh));
    }

    /// #946 item 5: an empty or unparsable ref is `CorruptRef`, never "no commits yet": a commit
    /// can't start the lineage over and `gc` aborts before deleting anything. `forget` clears it.
    #[test]
    fn an_empty_or_garbage_ref_is_corrupt_not_absent() {
        // A ref written by hand with a trailing newline is still the address it holds.
        let dir = tempfile::tempdir().unwrap();
        let (repo, v0) = with_first_version(dir.path());
        std::fs::write(repo.ref_path(L).unwrap(), format!("product {v0}\n")).unwrap();
        assert_eq!(repo.read_ref(L).unwrap(), Some(v0));
        repo.gc().unwrap();

        for content in [&b""[..], b"blake3:", b"blake3:0123\n", b"not a ref"] {
            let dir = tempfile::tempdir().unwrap();
            let (repo, _) = with_first_version(dir.path());
            std::fs::write(repo.ref_path(L).unwrap(), content).unwrap();
            let before = objects(dir.path());
            let corrupt = |e: &Error| matches!(e, Error::CorruptRef { path, .. } if *path == repo.ref_path(L).unwrap());

            let e = repo.read_ref(L).unwrap_err();
            assert!(corrupt(&e), "{content:?}: {e}");
            let (m, p) = sealed_on(5, None);
            let e = repo.commit(L, None, &m, &p).unwrap_err();
            assert!(corrupt(&e), "{content:?}: {e}");
            let e = repo.gc().unwrap_err();
            assert!(
                matches!(&e, Error::GcAborted { source, .. } if corrupt(source)),
                "{content:?}: {e}"
            );
            assert_eq!(objects(dir.path()), before, "{content:?}: nothing deleted");

            assert!(repo.forget(L).unwrap());
            assert_eq!(repo.read_ref(L).unwrap(), None);
        }
    }

    /// A chunk index of `ct` whose block is gone: the #942 review's orphan tip, sealed by hand
    /// (every writer refuses it) on `parent`. Returns its stored bytes and address.
    fn orphan_index_version(parent: &str) -> (Vec<u8>, String) {
        let (ct, _) = crate::blob_block("ct", "ct.bin", None, b"CT".to_vec()).unwrap();
        let (mut idx, _) = crate::blob_block("ct.cidx", "x.bin", None, b"IDX".to_vec()).unwrap();
        idx.kind = tessera_core::block::BlockKind::ChunkIndex;
        idx.class = tessera_core::block::BlockClass::Derived;
        idx.spec = serde_json::json!({"recipe": "chunk_index@1", "indexes": "ct"});
        let (m, _) = sealed_on(7, Some(parent));
        let mut b = ProductBuilder::from_manifest(&m).unwrap();
        b.add_block_ref(ct);
        b.add_block_ref(idx);
        b.add_source(Source::new(SUPERSEDES, parent).with_content_hash(parent));
        let mut m = b.seal().unwrap();
        m.blocks.retain(|b| b.name != "ct");
        m.content_hash = Some(m.recompute_content_hash().unwrap());
        let bytes = m.canonical_bytes().unwrap();
        let address = tessera_core::hash::digest(&bytes);
        (bytes, address)
    }

    /// #946 item 5, confirmed by probe in the #942 review: `gc` over a repository holding one
    /// manifest it cannot read deletes nothing, fails, and names the manifest and how it reached it.
    /// The tip refused for an orphan chunk index on a lineage with history (#942: `gc` exited 0
    /// having deleted the older versions and the data block), a torn tip, and a torn ancestor.
    #[test]
    fn gc_over_an_unreadable_manifest_deletes_nothing_and_names_it() {
        // The #942 orphan tip, on v0 → v1.
        let dir = tempfile::tempdir().unwrap();
        let (repo, v0) = with_first_version(dir.path());
        let (m1, _) = sealed_on(1, Some(&v0));
        let v1 = repo.commit(L, Some(&v0), &m1, &[]).unwrap();
        let (bytes, orphan) = orphan_index_version(&v1);
        plant(&repo, L, &orphan, &bytes);
        let before = objects(dir.path());
        let e = repo.gc().unwrap_err();
        assert!(
            matches!(&e, Error::GcAborted { what, source }
                if what.contains(&orphan) && what.contains("tip of ref")
                    && matches!(**source, Error::DanglingChunkIndex { .. })),
            "{e}"
        );
        assert_eq!(objects(dir.path()), before, "nothing deleted");

        // A torn tip, and a torn ancestor reached by the tip's supersedes edge.
        for torn in ["tip", "ancestor"] {
            let dir = tempfile::tempdir().unwrap();
            let (repo, v0) = with_first_version(dir.path());
            let (m1, _) = sealed_on(1, Some(&v0));
            let v1 = repo.commit(L, Some(&v0), &m1, &[]).unwrap();
            // An unreachable object, so that a sweep would have something to delete.
            let (m9, _) = sealed_on(9, None);
            repo.put_manifest(&m9).unwrap();
            let victim = if torn == "tip" { &v1 } else { &v0 };
            let path = repo.object_path(victim).unwrap();
            let b = std::fs::read(&path).unwrap();
            std::fs::write(&path, &b[..b.len() / 2]).unwrap();
            let before = objects(dir.path());
            let e = repo.gc().unwrap_err();
            let via = if torn == "tip" {
                "tip of ref".to_string()
            } else {
                format!("'supersedes' edge of {v1}")
            };
            assert!(
                matches!(&e, Error::GcAborted { what, .. } if what.contains(victim.as_str()) && what.contains(&via)),
                "{torn}: {e}"
            );
            assert_eq!(objects(dir.path()), before, "{torn}: nothing deleted");
        }
    }

    /// #946 review: an address can be a block of one version and the tip manifest of another (a blob
    /// block holding that manifest's bytes). Marking it as a block must not stop `gc` from walking
    /// it as a tip: its history and blocks are kept. The walk order follows `refs/`'s listing, so the
    /// case runs with the lineage names swapped, and one of the two runs walks the blob's lineage
    /// first, whatever the filesystem's order.
    #[test]
    fn gc_walks_a_tip_another_version_stores_as_a_block() {
        for (holder, tipped) in [("a", "z"), ("z", "a")] {
            let dir = tempfile::tempdir().unwrap();
            let repo = Repository::init(dir.path()).unwrap();
            let (m0, p0) = version(0, None, &[9, 9]);
            let v0 = repo.commit(tipped, None, &m0, &p0).unwrap();
            let (m1, _) = version(1, Some(&v0), &[9, 9]);
            let v1 = repo.commit(tipped, Some(&v0), &m1, &[]).unwrap();
            let tip_bytes = std::fs::read(repo.object_path(&v1).unwrap()).unwrap();
            let (blob, payload) =
                crate::blob_block("copy", "copy.json", None, tip_bytes.clone()).unwrap();
            assert_eq!(blob.digest.as_deref(), Some(v1.as_str()), "one address");
            let mut b = ProductBuilder::new("blob", "holder", "d", "2024-01-01T00:00:00Z");
            b.add_block_ref(blob);
            let holder_m = b.seal().unwrap();
            repo.commit(holder, None, &holder_m, &[payload]).unwrap();

            let report = repo.gc().unwrap();
            assert_eq!(report.reclaimed, 0, "{holder}/{tipped}: {report:?}");
            assert_eq!(
                reaches_intact(&repo, &v1),
                [v1.clone(), v0.clone()],
                "{holder}/{tipped}"
            );
        }
    }

    /// An intact object an edge pins that is not a manifest (a stored source file) is kept, and
    /// does not abort `gc`: only a manifest it cannot read does.
    #[test]
    fn gc_keeps_a_non_manifest_object_an_edge_pins() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let raw = b"raw vendor bytes, not JSON".to_vec();
        let raw_digest = tessera_core::hash::digest(&raw);
        repo.write_object(&raw_digest, &raw).unwrap();
        let (m, p) = sealed_on(0, None);
        let mut b = ProductBuilder::from_manifest(&m).unwrap();
        b.add_source(Source::new("ingested_from", "scan.l64").with_content_hash(&raw_digest));
        let m = b.seal().unwrap();
        repo.commit(L, None, &m, &p).unwrap();
        let (unreachable, _) = sealed_on(9, None);
        repo.put_manifest(&unreachable).unwrap();
        let rep = repo.gc().unwrap();
        assert_eq!((rep.kept, rep.reclaimed), (3, 1), "{rep:?}");
        assert_eq!(repo.read_object(&raw_digest).unwrap(), raw);
    }

    // ── The store ────────────────────────────────────────────────────────────────────────────────

    #[test]
    fn commit_dedups_shared_blocks_and_advances_ref() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();

        let (m1, p1) = sealed_on(1, None);
        let mh1 = repo.commit(L, None, &m1, &p1).unwrap();
        assert_eq!(count_objects(dir.path()), 2, "v1 = 1 block + 1 manifest");

        let (m2, p2) = sealed_on(2, Some(&mh1));
        assert_eq!(
            m1.blocks[0].digest, m2.blocks[0].digest,
            "block bytes identical → same digest"
        );
        let mh2 = repo.commit(L, Some(&mh1), &m2, &p2).unwrap();
        assert_eq!(
            count_objects(dir.path()),
            3,
            "v2 shares the block → only the manifest is new"
        );
        assert_ne!(mh1, mh2, "metadata changed → different seal");

        assert_eq!(repo.read_ref(L).unwrap(), Some(mh2.clone()));
        assert_eq!(
            repo.get_manifest(&mh1).unwrap().metadata.get("rev"),
            Some(&serde_json::json!(1))
        );
        assert_eq!(
            repo.get_manifest(&mh2).unwrap().metadata.get("rev"),
            Some(&serde_json::json!(2))
        );

        let log = repo.log(L).unwrap();
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].manifest_hash, mh1);
        assert_eq!(log[1].manifest_hash, mh2);
        assert_eq!(log[1].parent.as_deref(), Some(mh1.as_str()));
    }

    /// #734: a hand-built manifest listing a block name twice (properly re-sealed over that list) is
    /// refused by `commit` and `put_manifest` with the typed error, before anything is stored: the
    /// lineage tip never moves to an object `get_manifest` would refuse.
    #[test]
    fn commit_and_put_manifest_refuse_a_block_name_listed_twice() {
        let dup = |e: &Error| matches!(e, Error::DuplicateBlockName { name } if name == "events");
        // The second `events` carries the block's own digest (before the fix the commit stored it
        // and moved the tip), or a foreign one.
        for foreign in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let repo = Repository::init(dir.path()).unwrap();
            let (mut m, p) = sealed_on(1, None);
            let mut second = m.blocks[0].clone();
            if foreign {
                second.digest = Some(tessera_core::hash::digest(b"NOT THE BYTES"));
            }
            m.blocks.push(second);
            m.content_hash = Some(m.recompute_content_hash().unwrap());
            m.manifest_hash = Some(m.compute_manifest_hash().unwrap());

            let e = repo.commit(L, None, &m, &p).unwrap_err();
            assert!(dup(&e), "foreign={foreign}: {e}");
            assert_eq!(repo.read_ref(L).unwrap(), None, "the tip must not move");
            assert_eq!(
                count_objects(dir.path()),
                0,
                "a refused commit stores nothing"
            );

            let e = repo.put_manifest(&m).unwrap_err();
            assert!(dup(&e), "foreign={foreign}: {e}");
            assert_eq!(count_objects(dir.path()), 0);
        }
    }

    #[test]
    fn metadata_only_commit_writes_exactly_one_object() {
        let dir = tempfile::tempdir().unwrap();
        let (repo, v0) = with_first_version(dir.path());
        let before = count_objects(dir.path());
        // Second version supplies NO payloads — the block is already stored; only the manifest is new.
        let (m2, _) = sealed_on(2, Some(&v0));
        repo.commit(L, Some(&v0), &m2, &[]).unwrap();
        assert_eq!(count_objects(dir.path()), before + 1);
    }

    #[test]
    fn commit_rejects_a_block_neither_supplied_nor_stored() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let (m, _payloads) = sealed_on(1, None);
        let err = repo.commit(L, None, &m, &[]).unwrap_err();
        assert!(matches!(err, Error::MissingProductBlock { .. }));
        assert_eq!(repo.read_ref(L).unwrap(), None);
    }

    #[test]
    fn set_ref_rejects_a_moved_tip() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let aa = repo.put_manifest(&sealed_on(10, None).0).unwrap();
        let bb = repo.put_manifest(&sealed_on(11, None).0).unwrap();
        repo.set_ref("s", None, &aa).unwrap();
        // expecting None but the tip is now aa → CAS fails, typed
        let err = repo.set_ref("s", None, &bb).unwrap_err();
        assert!(
            matches!(&err, Error::RefMoved { expected: None, found: Some(f), .. } if *f == aa),
            "{err}"
        );
        repo.set_ref("s", Some(&aa), &bb).unwrap();
        assert_eq!(repo.read_ref("s").unwrap(), Some(bb.clone()));
        // A ref holds a typed version address.
        let err = repo.set_ref("s", Some(&bb), "blake3:aa").unwrap_err();
        assert!(matches!(err, Error::MalformedSeal { .. }), "{err}");
        // ... of an object stored here.
        let absent = tessera_core::hash::digest(b"never stored");
        let err = repo.set_ref("s", Some(&bb), &absent).unwrap_err();
        assert!(err.to_string().contains("is not stored"), "{err}");
        assert_eq!(repo.read_ref("s").unwrap(), Some(bb));
    }

    #[test]
    fn forget_then_gc_reclaims_exclusive_keeps_shared() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let (m1, p1) = sealed_on(1, None);
        let (m2, p2) = sealed_on(2, None); // different metadata, SAME block digest
        repo.commit("a", None, &m1, &p1).unwrap();
        repo.commit("b", None, &m2, &p2).unwrap();
        assert_eq!(count_objects(dir.path()), 3, "1 shared block + 2 manifests");

        // nothing to collect while both refs live
        assert_eq!(repo.gc().unwrap().reclaimed, 0);

        // forget "a" → gc reclaims a's manifest, keeps the shared block (b still references it)
        assert!(repo.forget("a").unwrap());
        let rep = repo.gc().unwrap();
        assert_eq!(rep.reclaimed, 1, "a's now-unreachable manifest");
        assert_eq!(
            count_objects(dir.path()),
            2,
            "shared block + b's manifest remain"
        );

        // b is intact + verifiable
        let tip_b = repo.read_ref("b").unwrap().unwrap();
        repo.get_manifest(&tip_b).unwrap();
        assert!(
            !repo.forget("nope").unwrap(),
            "forgetting an absent lineage is a no-op"
        );
    }

    #[test]
    fn read_object_detects_corruption() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let (m, p) = sealed_on(1, None);
        repo.commit(L, None, &m, &p).unwrap();
        let digest = m.blocks[0].digest.clone().unwrap();
        std::fs::write(repo.object_path(&digest).unwrap(), b"corrupted").unwrap();
        assert!(matches!(
            repo.read_object(&digest),
            Err(Error::Integrity { .. })
        ));
    }

    /// #975: lineages whose names differ only in a character the old mapping replaced (`a:b`,
    /// `a_b`, `a/b`, `a\b`), or only in case (`Ab`, `ab`, which a case-insensitive filesystem
    /// folds), each keep their own tip and their own log.
    #[test]
    fn distinct_lineages_keep_independent_tips() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let names = ["a:b", "a_b", "a/b", "a\\b", "Ab", "ab", "a%3ab", "a.b"];
        let mut tips = Vec::new();
        for (rev, lineage) in names.iter().enumerate() {
            let (m, p) = sealed_on(rev as i64, None);
            tips.push(repo.commit(lineage, None, &m, &p).unwrap());
        }
        // A second commit on each, built on its own tip.
        for (i, lineage) in names.iter().enumerate() {
            let (m, p) = version(100 + i as i64, Some(&tips[i]), &[1, 2, 3]);
            tips[i] = repo.commit(lineage, Some(&tips[i]), &m, &p).unwrap();
        }
        for (i, lineage) in names.iter().enumerate() {
            assert_eq!(repo.read_ref(lineage).unwrap().as_ref(), Some(&tips[i]));
            assert_eq!(repo.log(lineage).unwrap().len(), 2, "{lineage}");
        }
        assert_eq!(
            std::fs::read_dir(dir.path().join("refs/lineages"))
                .unwrap()
                .count(),
            names.len()
        );
        assert!(repo.forget("a:b").unwrap());
        assert_eq!(repo.read_ref("a:b").unwrap(), None);
        assert_eq!(repo.read_ref("a_b").unwrap().as_ref(), Some(&tips[1]));
    }

    #[test]
    fn lineage_names_match_shared_encoder_and_original_outputs() {
        for (input, expected) in [
            ("a:b", "a%3ab"),
            ("a_b", "a_b"),
            ("Ab", "%41b"),
            ("ab", "ab"),
            ("blake3:abc", "blake3%3aabc"),
            ("é/%", "%c3%a9%2f%25"),
        ] {
            assert_eq!(lineage_file_name(input).unwrap(), expected);
            assert_eq!(
                lineage_file_name(input).unwrap(),
                crate::names::file_name(input)
            );
        }
    }

    #[test]
    fn lineage_file_names_are_percent_encoded_and_bounded() {
        let name = |l: &str| lineage_file_name(l).unwrap();
        assert_eq!(name("a_b"), "a_b");
        assert_eq!(name("a:b"), "a%3ab");
        assert_eq!(name("a%3ab"), "a%253ab");
        assert_eq!(name("Ab"), "%41b");
        assert_eq!(name(".."), "%2e%2e");
        assert_eq!(name("é"), "%c3%a9");
        let id = format!("blake3:{}", "0".repeat(64));
        assert_eq!(name(&id), format!("blake3%3a{}", "0".repeat(64)));
        let long = |n: usize| lineage_file_name(&"a".repeat(n));
        assert_eq!(
            long(MAX_LINEAGE_FILE_NAME).unwrap().len(),
            MAX_LINEAGE_FILE_NAME
        );
        assert!(matches!(
            long(MAX_LINEAGE_FILE_NAME + 1),
            Err(Error::InvalidRefName { .. })
        ));
        // Encoding counts: 84 `:` are 252 bytes.
        assert!(matches!(
            lineage_file_name(&":".repeat(84)),
            Err(Error::InvalidRefName { .. })
        ));
        assert!(matches!(
            lineage_file_name(""),
            Err(Error::InvalidRefName { .. })
        ));

        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let (m, p) = sealed_on(0, None);
        for bad in ["", &"x".repeat(MAX_LINEAGE_FILE_NAME + 1)] {
            let invalid = |e: Error| matches!(e, Error::InvalidRefName { .. });
            assert!(invalid(repo.read_ref(bad).unwrap_err()));
            assert!(invalid(repo.commit(bad, None, &m, &p).unwrap_err()));
            assert!(invalid(repo.log(bad).unwrap_err()));
            assert!(invalid(repo.forget(bad).unwrap_err()));
        }
        assert_eq!(
            count_objects(dir.path()),
            0,
            "a refused commit stores nothing"
        );
    }

    /// The same injective encoding and bound apply independently in each namespace.
    #[test]
    fn ref_names_encode_injectively_in_both_namespaces() {
        let names = [
            "a:b", "a_b", "A_b", "a.b", "a/b", "a\\b", "a%3ab", "é", "pin:x",
        ];
        for namespace in [RefNamespace::Lineages, RefNamespace::Pins] {
            let encoded: BTreeSet<_> = names
                .iter()
                .map(|name| ref_file_name(namespace, name).unwrap())
                .collect();
            assert_eq!(encoded.len(), names.len());
            for name in names {
                assert_eq!(
                    ref_file_name(namespace, name).unwrap(),
                    lineage_file_name(name).unwrap()
                );
            }
            for name in [String::new(), ":".repeat(84), "a".repeat(250)] {
                assert!(matches!(ref_file_name(namespace, &name),
                    Err(Error::InvalidRefName { namespace: found, name: bad, .. })
                    if found == namespace.as_str() && bad == name));
            }
        }
    }

    /// The files under `root`'s `refs/` and `log/` and its layout marker, with their bytes.
    fn ref_state(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        let mut out = BTreeMap::new();
        for d in ["refs", "refs/lineages", "refs/pins", "log", "log/lineages"] {
            for e in std::fs::read_dir(root.join(d)).into_iter().flatten() {
                let path = e.unwrap().path();
                if path.is_file() {
                    out.insert(path.clone(), std::fs::read(&path).unwrap());
                }
            }
        }
        if let Ok(b) = std::fs::read(root.join(LAYOUT)) {
            out.insert(root.join(LAYOUT), b);
        }
        out
    }

    /// #975: a repository laid out the way tessera wrote it before #975 (no layout marker; each
    /// ref and log cache named with `:`, `/` and `\` replaced by `_`) is refused by every entry
    /// point, reader and writer, never read with the new naming. The error names the layout it
    /// found and the one it needs, and says to import the source products again; nothing is
    /// written.
    #[test]
    fn a_repository_laid_out_before_975_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let (m, p) = sealed_on(0, None);
        let id = m.id.clone();
        assert!(id.starts_with("blake3:"), "{id}");
        for l in [id.as_str(), "plain"] {
            repo.commit(l, None, &m, &p).unwrap();
        }
        // The old layout: no marker, and the old mapping's names.
        std::fs::remove_file(dir.path().join(LAYOUT)).unwrap();
        for l in [id.as_str(), "plain"] {
            let (new, old) = (
                lineage_file_name(l).unwrap(),
                l.replace([':', '/', '\\'], "_"),
            );
            let at = |d: &str, n: &str, ext: &str| dir.path().join(d).join(format!("{n}{ext}"));
            std::fs::rename(at("refs/lineages", &new, ""), at("refs", &old, "")).unwrap();
            std::fs::rename(
                at("log/lineages", &new, ".jsonl"),
                at("log", &old, ".jsonl"),
            )
            .unwrap();
        }
        let before = ref_state(dir.path());

        let refused = |e: Error| {
            let text = e.to_string();
            assert!(
                matches!(&e, Error::UnsupportedRepositoryLayout { found, required, .. }
                    if found.contains("before #975") && required.contains("tessera-repository 2")),
                "{e}"
            );
            assert!(text.contains("import the source products again"), "{text}");
        };
        refused(Repository::open(dir.path()).err().unwrap());
        refused(Repository::init(dir.path()).err().unwrap());
        // A handle opened before the layout changed: its writes check the layout under the lock.
        let (m1, p1) = sealed_on(1, None);
        refused(repo.commit("other", None, &m1, &p1).unwrap_err());
        refused(repo.forget("plain").unwrap_err());
        refused(repo.gc().unwrap_err());
        assert_eq!(ref_state(dir.path()), before, "a refusal writes nothing");
        // A log cache alone is enough to tell.
        std::fs::remove_dir_all(dir.path().join("refs")).unwrap();
        std::fs::create_dir(dir.path().join("refs")).unwrap();
        refused(Repository::open(dir.path()).err().unwrap());
    }

    /// Only init creates a marker, and only in a directory without repository entries.
    #[test]
    fn unmarked_directories_are_only_initialized_when_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            Repository::open(dir.path()),
            Err(Error::UnsupportedRepositoryLayout { .. })
        ));
        let repo = Repository::init(dir.path()).unwrap();
        assert_eq!(
            std::fs::read(dir.path().join(LAYOUT)).unwrap(),
            LAYOUT_V2.as_bytes()
        );
        std::fs::remove_file(dir.path().join(LAYOUT)).unwrap();
        assert!(matches!(
            Repository::open(dir.path()),
            Err(Error::UnsupportedRepositoryLayout { .. })
        ));
        assert!(matches!(
            Repository::init(dir.path()),
            Err(Error::UnsupportedRepositoryLayout { .. })
        ));
        let (m, p) = sealed_on(0, None);
        assert!(matches!(
            repo.commit(L, None, &m, &p),
            Err(Error::UnsupportedRepositoryLayout { .. })
        ));
    }
    #[test]
    fn gc_marks_product_pins_independently_of_lineages() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let (m, payloads) = sealed_on(0, None);
        let tip = repo.commit("x", None, &m, &payloads).unwrap();
        let pin = dir.path().join("refs/pins/x");
        let record = format!("product {tip}\n");
        std::fs::write(&pin, &record).unwrap();
        repo.forget("x").unwrap();
        assert_eq!(repo.log("x").unwrap(), Vec::<LogEntry>::new());
        let report = repo.gc().unwrap();
        assert_eq!((report.kept, report.reclaimed), (2, 0));
        repo.get_manifest(&tip).unwrap();
        repo.commit("x", None, &m, &payloads).unwrap();
        assert_eq!(std::fs::read_to_string(&pin).unwrap(), record);
        std::fs::remove_file(&pin).unwrap();
        assert_eq!(repo.read_ref("x").unwrap(), Some(tip));
        assert_eq!(repo.gc().unwrap().reclaimed, 0);
        repo.forget("x").unwrap();
        assert_eq!(repo.gc().unwrap().reclaimed, 2);
    }

    #[test]
    fn stray_entries_in_either_ref_namespace_abort_gc() {
        for (entry, directory) in [
            ("refs/stray", false),
            ("refs/stray", true),
            ("refs/lineages/stray", true),
            ("refs/pins/stray", true),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let repo = Repository::init(dir.path()).unwrap();
            let (m, _) = sealed_on(0, None);
            repo.put_manifest(&m).unwrap();
            let path = dir.path().join(entry);
            if directory {
                std::fs::create_dir(&path).unwrap();
            } else {
                std::fs::write(&path, b"stray").unwrap();
            }
            let before = objects(dir.path());
            assert!(matches!(repo.gc(), Err(Error::GcAborted { source, .. })
                if matches!(source.as_ref(), Error::CorruptRef { path: found, .. } if *found == path)));
            assert_eq!(objects(dir.path()), before);
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_not_regular_ref_files() {
        use std::os::unix::fs::symlink;
        for entry in ["refs/lineages/x", "refs/pins/x", "refs/other"] {
            let dir = tempfile::tempdir().unwrap();
            let repo = Repository::init(dir.path()).unwrap();
            let (m, _) = sealed_on(0, None);
            repo.put_manifest(&m).unwrap();
            let external = dir.path().join("external");
            std::fs::write(&external, format!("product {}\n", m.manifest_hash.unwrap())).unwrap();
            symlink(&external, dir.path().join(entry)).unwrap();
            let before = objects(dir.path());
            assert!(matches!(repo.gc(), Err(Error::GcAborted { .. })));
            assert_eq!(objects(dir.path()), before);
            if entry == "refs/lineages/x" {
                assert!(matches!(repo.read_ref("x"), Err(Error::CorruptRef { .. })));
            }
        }
    }

    #[test]
    fn every_writer_refuses_other_layouts_before_touching_objects_or_tmp() {
        for marker in [
            Some("tessera-repository 1\n"),
            Some("tessera-repository 3\n"),
            Some("tessera-repository 2"),
            None,
        ] {
            let dir = tempfile::tempdir().unwrap();
            let (repo, tip) = with_first_version(dir.path());
            let (m, payloads) = sealed_on(1, Some(&tip));
            if let Some(marker) = marker {
                std::fs::write(dir.path().join(LAYOUT), marker).unwrap();
            } else {
                std::fs::remove_file(dir.path().join(LAYOUT)).unwrap();
            }
            let debris = dir.path().join("tmp/debris");
            std::fs::write(&debris, b"untouched").unwrap();
            let before = objects(dir.path());
            let refs_before = ref_state(dir.path());
            let refused = |e| {
                assert!(
                    matches!(e, Error::UnsupportedRepositoryLayout { required, .. }
                if required.contains("tessera-repository 2"))
                )
            };
            refused(Repository::open(dir.path()).err().unwrap());
            refused(Repository::init(dir.path()).err().unwrap());
            refused(repo.commit(L, Some(&tip), &m, &payloads).unwrap_err());
            refused(repo.set_ref(L, Some(&tip), &tip).unwrap_err());
            refused(
                repo.write_object(&tessera_core::hash::digest(b"new"), b"new")
                    .unwrap_err(),
            );
            refused(repo.put_manifest(&m).unwrap_err());
            refused(repo.forget(L).unwrap_err());
            refused(repo.gc().unwrap_err());
            refused(repo.read_ref(L).unwrap_err());
            refused(repo.log(L).unwrap_err());
            assert_eq!(objects(dir.path()), before);
            assert_eq!(ref_state(dir.path()), refs_before);
            assert_eq!(std::fs::read(debris).unwrap(), b"untouched");
        }
    }
    #[test]
    fn init_refusal_creates_no_lock_or_other_entries() {
        for directory in ["refs", "log", "objects"] {
            let dir = tempfile::tempdir().unwrap();
            std::fs::create_dir(dir.path().join(directory)).unwrap();
            let entry = dir.path().join(directory).join("existing");
            std::fs::write(&entry, b"untouched").unwrap();
            // Even a crash at the first durable seam must never leave a lock in
            // a directory initialization refuses. Correct preflight reaches no seam.
            let mut child = spawn_child(&["init-refuse", dir.path().to_str().unwrap()]);
            let printed = results(&mut BufReader::new(child.stdout.take().unwrap()));
            let status = child.wait().unwrap();
            assert!(!dir.path().join(LOCK).exists());
            assert!(
                status.success() && printed == ["REFUSED"],
                "{status} {printed:?}"
            );
            assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
            assert_eq!(std::fs::read(entry).unwrap(), b"untouched");
        }
    }

    #[test]
    fn a_crash_at_every_step_of_init_can_be_retried() {
        for nested in [false, true] {
            let steps = {
                let dir = tempfile::tempdir().unwrap();
                let root = if nested {
                    dir.path().join("nested/repo")
                } else {
                    dir.path().to_path_buf()
                };
                let recording = seam::record_fs();
                Repository::init(&root).unwrap();
                let events = recording.events();
                let mut replay = seam::Replay::default();
                let mut published = false;
                for event in &events {
                    if let seam::Event::Rename { to, .. } = event {
                        published |= *to == root.join(LAYOUT);
                    }
                    if let seam::Event::CreateDir(path) = event {
                        if ["objects", "refs", "log"]
                            .iter()
                            .any(|directory| path.starts_with(root.join(directory)))
                        {
                            assert!(published, "the marker must precede the namespace skeleton");
                            assert!(
                                replay.undoable(dir.path(), &root.join(LAYOUT)).is_empty(),
                                "the marker must survive power loss before creating {}",
                                path.display()
                            );
                        }
                    }
                    replay.apply(event);
                }
                events.len()
            };
            for n in 1..=steps + 1 {
                let dir = tempfile::tempdir().unwrap();
                let root = if nested {
                    dir.path().join("nested/repo")
                } else {
                    dir.path().to_path_buf()
                };
                let mut child = spawn_child(&["init-kill", root.to_str().unwrap(), &n.to_string()]);
                let printed = results(&mut BufReader::new(child.stdout.take().unwrap()));
                let status = child.wait().unwrap();
                if n <= steps {
                    assert!(
                        status.code().is_none() && printed.is_empty(),
                        "step {n}: {status} {printed:?}"
                    );
                } else {
                    assert!(
                        status.success() && printed == ["DONE"],
                        "{status} {printed:?}"
                    );
                }
                match Repository::open(&root) {
                    Ok(repo) => {
                        assert!(repo.pins().unwrap().is_empty());
                        repo.gc().unwrap();
                    }
                    Err(Error::UnsupportedRepositoryLayout { .. }) => {}
                    Err(Error::Invalid(message)) => assert!(message.contains("run `tessera init`")),
                    Err(error) => panic!("step {n}: unexpected refusal: {error}"),
                }
                let repo = Repository::init(&root).unwrap_or_else(|e| panic!("step {n}: {e}"));
                Repository::open(&root).unwrap();
                for directory in ["objects", "refs/lineages", "refs/pins", "log/lineages", TMP] {
                    assert!(root.join(directory).is_dir(), "step {n}: {directory}");
                }
                let (m, _) = sealed_on(0, None);
                repo.put_manifest(&m).unwrap();
                assert_eq!(
                    repo.get_manifest(m.manifest_hash.as_deref().unwrap())
                        .unwrap(),
                    m
                );
                repo.gc().unwrap();
            }
        }
    }

    /// The authoritative initialization check remains fenced by the writer lock;
    /// the read-only preflight must not replace that check.
    #[test]
    fn init_checks_entries_under_the_writer_lock() {
        let dir = tempfile::tempdir().unwrap();
        let lock = dir.path().join(LOCK);
        let checks = std::rc::Rc::new(std::cell::Cell::new(0));
        let seen = checks.clone();
        let watching = seam::watch(move |event| {
            if matches!(event, seam::Event::InitLayoutCheck) {
                let other = File::open(&lock).unwrap();
                assert!(
                    matches!(other.try_lock(), Err(std::fs::TryLockError::WouldBlock)),
                    "initialization inspected repository entries without the writer lock"
                );
                seen.set(seen.get() + 1);
            }
        });
        Repository::init(dir.path()).unwrap();
        Repository::init(dir.path()).unwrap();
        drop(watching);
        assert_eq!(checks.get(), 2);
    }
}
