//! `tessera provenance` — the operator view of a product's `derived_from` DAG (#452 part 2).
//!
//! `inspect` answers "who made THIS product, and how". ADR-0058 §5's real question is a chain
//! question: *what identity did this inherit, from where, and what recipe made each hop?* Answering it
//! means resolving parents, which live in separate `.tsra` files — so this is parent resolution plus a
//! render, not a formatting change.
//!
//! The traversal is [`tessera_core::provenance::walk`], the same one `verify_chain` drives. Two walks
//! would drift, and then the verifier and the operator view would disagree about what the DAG is.
//! Where they differ is the *visitor*: `verify_chain` refuses any edge it cannot prove, while this
//! renders the gap and keeps going, because "I can't reach that parent from here" is a fact an operator
//! needs to see rather than an error that hides the rest of the chain.
//!
//! **Local-only.** Parents are found on the filesystem: the product's own directory, a
//! `collection.json`, and any `--search` directory. An OCI/registry-backed resolver is deliberately out
//! of scope here (the `--help` says so) — it needs a fetch policy and a cache, and shipping it inside a
//! read verb would make `provenance` quietly hit the network.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use tessera_core::collection::{member_filename, Collection, MemberKind};
use tessera_core::provenance::{is_version_pointer, walk, EdgeOutcome, Resolver, Step, Visit};
use tessera_core::{Error, Manifest, Result};
use tessera_io::Reader;

use crate::nav;

/// One `.tsra` found on disk, with where it was found (so the render can point at it).
struct Candidate {
    manifest: Manifest,
    path: PathBuf,
}

/// Resolves provenance references against `.tsra` files found on disk.
///
/// Indexed by lineage `id` **and** version: a reference addresses a lineage, not a version
/// (ADR-0036), so a directory can legitimately hold two commits of the same parent. Keeping both and
/// letting [`Resolver::resolve_pinned`] pick the pinned one is what stops a walk from reporting skew
/// by coin flip when the newer version happens to sort first.
#[derive(Default)]
pub struct FileResolver {
    by_id: BTreeMap<String, Vec<Candidate>>,
    corrupt: Vec<Corrupt>,
}

/// A candidate file that **is** a `.tsra` but whose manifest does not verify. Kept separately because it
/// cannot be indexed — its manifest is exactly the thing we cannot trust — and because it must never be
/// reported as a merely *missing* parent: "I could not find it" invites another `--search`, while "what I
/// found is broken" is the end of the search and the start of an incident.
struct Corrupt {
    path: PathBuf,
    detail: String,
    /// The lineage `id` the file *claims*, read without verifying (`read_manifest_unverified`). Untrusted
    /// by construction — used only to point the operator at the file from the edge that needed it, never
    /// to satisfy that edge. `None` when even the claim is unreadable.
    claims_id: Option<String>,
}

impl FileResolver {
    /// Index one `.tsra`, ignoring a version already held (first source wins, which is what makes the
    /// discovery order in [`discover`] meaningful). Indexed under its lineage `id` **and** its
    /// `manifest_hash`: an edge may reference either — `derived_from` names a lineage, while ADR-0036's
    /// `snapshot_of` breadcrumb names one specific version.
    fn add(&mut self, manifest: Manifest, path: PathBuf) {
        let versions = self.by_id.entry(manifest.id.clone()).or_default();
        if versions
            .iter()
            .any(|c| c.manifest.manifest_hash == manifest.manifest_hash)
        {
            return;
        }
        let mh = manifest.manifest_hash.clone();
        versions.push(Candidate {
            manifest: manifest.clone(),
            path: path.clone(),
        });
        // A version is also addressable by its own seal, for edges that reference a version directly.
        if let Some(mh) = mh {
            if mh != manifest.id {
                self.by_id
                    .entry(mh)
                    .or_default()
                    .push(Candidate { manifest, path });
            }
        }
    }

    /// Open a `.tsra` and index its manifest, splitting the two very different ways that can fail.
    ///
    /// A search directory is allowed to hold unrelated or half-written files, so "this is not a Tessera
    /// product" is skipped silently — otherwise `--search` would be unusable on a real staging dir. But a
    /// file that **is** a product (right magic, manifest present) whose manifest fails verification is
    /// corruption, and swallowing it reported a tampered parent as merely *absent*, at exit 0 (#452
    /// review). The split is exact, from `Reader::from_reader`: bad magic / no mimetype / missing
    /// `manifest.json` raise `Container`, while a manifest that does not verify raises `Integrity` and a
    /// malformed one raises `Serde`.
    fn add_path(&mut self, path: &Path) {
        match Reader::open(path) {
            Ok(r) => self.add(r.manifest().clone(), path.to_path_buf()),
            Err(e @ (Error::Integrity { .. } | Error::BlockIntegrity { .. } | Error::Serde(_))) => {
                // Read the identity it claims so the edge that needed this parent can name the file.
                // Untrusted: a claim by something that already failed verification, good for pointing
                // and nothing else.
                let claims_id = tessera_io::read_manifest_unverified(path)
                    .ok()
                    .map(|m| m.id);
                self.corrupt.push(Corrupt {
                    path: path.to_path_buf(),
                    detail: e.to_string(),
                    claims_id,
                });
            }
            // Not a `.tsra`, or unreadable: not this walk's business.
            Err(_) => {}
        }
    }

    /// Index every `.tsra` directly inside `dir` (not recursive — a search root is a directory of
    /// products, and recursing into a data tree turns a read verb into a filesystem crawl).
    fn add_dir(&mut self, dir: &Path) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        // A search root names where to look, so a symlink must not quietly widen it: resolve each
        // candidate and keep only what still lives under the (resolved) root. Without this, one link in a
        // staging dir turns `--search` into a read of somewhere the operator never named.
        let root = dir.canonicalize().ok();
        let mut paths: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "tsra"))
            .filter(|p| match (&root, p.canonicalize()) {
                (Some(root), Ok(real)) => real.starts_with(root),
                _ => false,
            })
            .collect();
        paths.sort(); // deterministic indexing order, so the render is reproducible
        for p in &paths {
            self.add_path(p);
        }
    }

    /// Where a specific version was found.
    fn path_of(&self, id: &str, manifest_hash: Option<&str>) -> Option<&Path> {
        let versions = self.by_id.get(id)?;
        versions
            .iter()
            .find(|c| c.manifest.manifest_hash.as_deref() == manifest_hash)
            .or_else(|| versions.first())
            .map(|c| c.path.as_path())
    }

    /// A corrupt file claiming to be this lineage, if one was found. What makes an unresolved edge
    /// reportable as corruption rather than absence.
    fn corrupt_claiming(&self, reference: &str) -> Option<&Corrupt> {
        self.corrupt
            .iter()
            .find(|c| c.claims_id.as_deref() == Some(reference))
    }

    /// How many distinct versions of this lineage were found — the number that explains a skew.
    fn versions(&self, id: &str) -> usize {
        self.by_id.get(id).map_or(0, |v| v.len())
    }
}

impl Resolver for FileResolver {
    fn resolve(&self, reference: &str) -> Option<Manifest> {
        self.by_id
            .get(reference)?
            .first()
            .map(|c| c.manifest.clone())
    }

    fn resolve_pinned(&self, reference: &str, pinned: Option<&str>) -> Option<Manifest> {
        let versions = self.by_id.get(reference)?;
        pinned
            .and_then(|p| {
                versions
                    .iter()
                    .find(|c| c.manifest.manifest_hash.as_deref() == Some(p))
            })
            .or_else(|| versions.first())
            .map(|c| c.manifest.clone())
    }
}

/// Build the resolver for `file`, in resolution order: the product's **own directory** (siblings, the
/// layout `ingest --out` produces), then the members of an explicit `--collection`, then each
/// `--search` directory. First source to supply a given version wins, so a `--collection` cannot be
/// silently overridden by a stray copy in a later search root.
pub fn discover(
    file: &Path,
    collection: Option<&Path>,
    search: &[PathBuf],
) -> Result<FileResolver> {
    let mut r = FileResolver::default();
    if let Some(dir) = file.parent() {
        r.add_dir(if dir.as_os_str().is_empty() {
            Path::new(".")
        } else {
            dir
        });
    }
    if let Some(cj) = collection {
        // An explicit --collection that cannot be read IS fatal: the operator named it, so silently
        // resolving nothing from it would look like "the parents just aren't there".
        let text = std::fs::read_to_string(cj)?;
        let c = Collection::from_json(&text)?;
        let base = cj.parent().unwrap_or(Path::new("."));
        for m in &c.members {
            if m.kind == MemberKind::Product {
                r.add_path(&base.join(member_filename(&m.reference, m.kind)));
            }
        }
    }
    for dir in search {
        r.add_dir(dir);
    }
    Ok(r)
}

/// Does this reference name a Tessera **product**, or an external artifact?
///
/// A product reference IS a product `id` — an `<alg>:<hex>` content digest, which is what
/// `Source::new("derived_from", &parent.id)` records. An `ingested_from` / `ingested_via_spec` edge
/// instead carries a vendor path, filename or SOP UID. The distinction decides whether "nothing
/// resolved" is a gap in the chain or simply where provenance legitimately ends, so it is read off the
/// reference's *shape* rather than the role: roles are free-form strings the format never constrains,
/// and a producer is free to invent one.
fn looks_like_product_id(reference: &str) -> bool {
    // Specifically `blake3:<64 hex>`: that is the only shape a product `id` takes, so a digest-shaped
    // external reference (an OCI `sha256:…`, a vendor checksum) is an external leaf rather than a parent
    // this walk failed to find. Accepting any `<alg>:<hex>` turned those into phantom gaps.
    let Some(hex) = reference.strip_prefix("blake3:") else {
        return false;
    };
    hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit())
}

/// How a hop resolved, in the taxonomy the render and the exit code both key off. Five distinct
/// facts — collapsing any two of them is how "I can't see that parent from here" ends up looking like
/// "that parent is corrupt", or vice versa.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HopKind {
    /// The parent resolved at exactly the pinned version. The chain is proven across this hop.
    Proven,
    /// The right lineage at a different version — a gap in what can be proven here, not corruption.
    Skew,
    /// A product-shaped reference nothing could be found for. Fetch it and the gap closes.
    Missing,
    /// The parent this edge needs **is** on disk and is a Tessera product, but its manifest does not
    /// verify. Distinct from `Missing` because the responses differ: absence sends you looking for another
    /// copy, corruption ends the search and starts an incident.
    CorruptParent,
    /// A product-shaped reference whose edge pins no version, so there is nothing to prove against.
    Unpinned,
    /// Acquisition provenance: a vendor path / filename / SOP UID, never a Tessera product. This is
    /// where a chain is *supposed* to end.
    ExternalLeaf,
    /// A pointer to one specific version (see [`tessera_core::provenance::is_version_pointer`])
    /// that is not on hand. Shown, but
    /// not a gap — `publish` drops history on purpose, so its absence is the designed state.
    VersionPointer,
}

impl HopKind {
    /// Does this hop leave the chain unproven? `--require-complete` turns any `true` into a non-zero
    /// exit. An external leaf is deliberately *not* a gap: a raw DICOM file is where provenance
    /// legitimately ends, so counting it would make every real chain permanently incomplete and the
    /// flag worthless.
    fn is_gap(self) -> bool {
        matches!(
            self,
            HopKind::Skew | HopKind::Missing | HopKind::Unpinned | HopKind::CorruptParent
        )
    }

    fn label(self) -> &'static str {
        match self {
            HopKind::Proven => "pinned",
            HopKind::Skew => "different version",
            HopKind::Missing => "unresolved",
            HopKind::CorruptParent => "CORRUPT",
            HopKind::Unpinned => "no pinned version",
            HopKind::ExternalLeaf => "external leaf",
            HopKind::VersionPointer => "version not present",
        }
    }

    /// The stable machine token for `--json`.
    fn token(self) -> &'static str {
        match self {
            HopKind::Proven => "proven",
            HopKind::Skew => "version_skew",
            HopKind::Missing => "unresolved",
            HopKind::CorruptParent => "corrupt_parent",
            HopKind::Unpinned => "unpinned",
            HopKind::ExternalLeaf => "external_leaf",
            HopKind::VersionPointer => "version_not_present",
        }
    }
}

/// One rendered hop.
struct Hop {
    depth: usize,
    role: String,
    reference: String,
    kind: HopKind,
    revisited: bool,
    pinned: Option<String>,
    resolved: Option<String>,
    /// The resolved parent, when there is one.
    parent: Option<Manifest>,
    /// Where that parent was found.
    path: Option<PathBuf>,
    /// Other versions of this lineage the resolver holds — what makes a skew explicable.
    versions: usize,
    /// The corrupt file that claims this lineage, when that is why nothing resolved.
    corrupt_file: Option<PathBuf>,
}

/// Collects the walk into hops and never fails, so the whole reachable chain is rendered even when
/// part of it cannot be proven. The verdict is the caller's to draw from [`Hop::kind`].
struct Collect<'r> {
    hops: Vec<Hop>,
    resolver: &'r FileResolver,
}

impl Visit for Collect<'_> {
    fn edge(&mut self, step: &Step<'_>) -> Result<()> {
        let (kind, pinned, resolved) = match &step.outcome {
            EdgeOutcome::Pinned => (
                HopKind::Proven,
                step.source.content_hash.clone(),
                step.source.content_hash.clone(),
            ),
            EdgeOutcome::VersionSkew { pinned, resolved } => {
                (HopKind::Skew, Some(pinned.clone()), Some(resolved.clone()))
            }
            // Whether "nothing to check" / "nothing found" is a gap depends on what the reference
            // NAMES: a product id is a parent we were meant to reach, a vendor path is a leaf.
            EdgeOutcome::Unpinned if looks_like_product_id(&step.source.reference) => {
                (HopKind::Unpinned, None, None)
            }
            EdgeOutcome::Unpinned => (HopKind::ExternalLeaf, None, None),
            EdgeOutcome::Unresolved if is_version_pointer(step.source) => (
                HopKind::VersionPointer,
                step.source.content_hash.clone(),
                None,
            ),
            EdgeOutcome::Unresolved if looks_like_product_id(&step.source.reference) => {
                // Nothing resolved because the only candidate for this lineage failed verification: say
                // so on the edge that needed it, rather than calling the parent absent.
                let kind = if self
                    .resolver
                    .corrupt_claiming(&step.source.reference)
                    .is_some()
                {
                    HopKind::CorruptParent
                } else {
                    HopKind::Missing
                };
                (kind, step.source.content_hash.clone(), None)
            }
            EdgeOutcome::Unresolved => (
                HopKind::ExternalLeaf,
                step.source.content_hash.clone(),
                None,
            ),
        };
        // The parent the walk already resolved, carried on the `Step`. Resolving it again here would be
        // a second lookup that could disagree with the one the traversal made — the exact drift a single
        // shared walk exists to prevent.
        let parent = step.parent.cloned();
        let path = parent
            .as_ref()
            .and_then(|p| self.resolver.path_of(&p.id, p.manifest_hash.as_deref()))
            .map(Path::to_path_buf);
        let versions = self.resolver.versions(&step.source.reference);
        let corrupt_file = self
            .resolver
            .corrupt_claiming(&step.source.reference)
            .map(|c| c.path.clone());
        self.hops.push(Hop {
            depth: step.depth,
            role: step.source.role.clone(),
            reference: step.source.reference.clone(),
            kind,
            revisited: step.revisited,
            pinned,
            resolved,
            parent,
            path,
            versions,
            corrupt_file,
        });
        Ok(())
    }
}

/// Options for [`run`]. `--require-complete` is deliberately absent: it changes only the exit code,
/// which the caller owns, and this module's job is to render the chain and report whether it is whole.
pub struct Opts {
    pub collection: Option<PathBuf>,
    pub search: Vec<PathBuf>,
    pub depth: Option<usize>,
    pub json: bool,
    pub full: bool,
}

/// What a walk concluded. Two independent facts, because they warrant different responses: an
/// incomplete chain is usually "fetch more and look again", while corruption is "stop and investigate".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Verdict {
    /// Every hop proven or a legitimate leaf.
    pub complete: bool,
    /// Candidate files that are Tessera products whose manifests do not verify.
    pub corrupt: usize,
}

/// Walk `file`'s provenance DAG and render it.
///
/// Exit policy (the caller maps this to a code): a rendered gap is **not** a failure, because a chain
/// you cannot fully resolve from one directory is the normal case and exiting non-zero would make the
/// verb useless in a pipeline. `--require-complete` is the strict mode for a gate. A cycle is a
/// malformed DAG and always propagates as `Err`.
pub fn run(file: &Path, opts: &Opts, out: &mut impl Write) -> Result<Verdict> {
    let root = Reader::open(file)?.manifest().clone();
    let resolver = discover(file, opts.collection.as_deref(), &opts.search)?;
    let mut collect = Collect {
        hops: Vec::new(),
        resolver: &resolver,
    };
    // A cycle aborts the walk mid-way. Render what was collected first, so the operator sees where the
    // loop is instead of a bare error, then surface the error (the caller exits non-zero).
    let walked = walk(&root, &resolver, opts.depth, &mut collect);
    // A chain cannot be called complete while the store it was resolved against holds a product whose
    // seal is broken: the parent that file was meant to supply is, at best, unaccounted for.
    let complete = collect.hops.iter().all(|h| !h.kind.is_gap()) && resolver.corrupt.is_empty();
    let verdict = Verdict {
        complete,
        corrupt: resolver.corrupt.len(),
    };
    if opts.json {
        write_json(&root, &collect.hops, &resolver.corrupt, verdict, out)?;
    } else {
        write_text(
            &root,
            &collect.hops,
            &resolver.corrupt,
            verdict,
            opts.full,
            out,
        )?;
    }
    walked?;
    Ok(verdict)
}

/// Indent for a hop at `depth` — the DAG is a tree once diamonds are collapsed.
fn indent(depth: usize) -> String {
    "  ".repeat(depth)
}

fn write_text(
    root: &Manifest,
    hops: &[Hop],
    corrupt: &[Corrupt],
    verdict: Verdict,
    full: bool,
    out: &mut impl Write,
) -> Result<()> {
    writeln!(
        out,
        "{} · {}  [{}]",
        root.product,
        root.name,
        nav::short_hash(root.manifest_hash.as_deref().unwrap_or("-"))
    )?;
    for h in hops {
        let pad = indent(h.depth);
        let name = h
            .parent
            .as_ref()
            .map(|p| format!("{} · {}", p.product, p.name))
            .unwrap_or_else(|| nav::compact_reference(&h.reference, full));
        let mark = if h.revisited { " (already shown)" } else { "" };
        writeln!(out, "{pad}↳ {} {name}  [{}]{mark}", h.role, h.kind.label())?;
        match h.kind {
            HopKind::Skew => {
                writeln!(
                    out,
                    "{pad}    pins      {}",
                    nav::short_hash(h.pinned.as_deref().unwrap_or("-"))
                )?;
                writeln!(
                    out,
                    "{pad}    resolved  {} — {} version(s) of this lineage found, \
                     none at the pinned version",
                    nav::short_hash(h.resolved.as_deref().unwrap_or("-")),
                    h.versions
                )?;
            }
            HopKind::CorruptParent => {
                writeln!(
                    out,
                    "{pad}    pins      {}",
                    nav::short_hash(h.pinned.as_deref().unwrap_or("-"))
                )?;
                writeln!(
                    out,
                    "{pad}    CORRUPT   {} — this file claims the lineage but its manifest does not \
                     verify; do not trust it",
                    h.corrupt_file
                        .as_deref()
                        .unwrap_or(Path::new("-"))
                        .display()
                )?;
            }
            HopKind::Missing => {
                writeln!(
                    out,
                    "{pad}    pins      {} — no product with this lineage was found \
                     (try --search / --collection)",
                    nav::short_hash(h.pinned.as_deref().unwrap_or("-"))
                )?;
            }
            // The leaf's hash is the source artifact's own digest, not a product seal — it closes the
            // integrity chain to the bytes that were ingested, which is worth showing.
            HopKind::ExternalLeaf => {
                if let Some(hash) = &h.pinned {
                    writeln!(out, "{pad}    source    {}", nav::short_hash(hash))?;
                }
            }
            HopKind::VersionPointer => {
                writeln!(
                    out,
                    "{pad}    version   {} — not present (publish drops history by design)",
                    nav::short_hash(h.pinned.as_deref().unwrap_or("-"))
                )?;
            }
            HopKind::Proven | HopKind::Unpinned => {}
        }
        // Producer + generation per hop: the "what recipe made each hop" half of the question. Skipped
        // on a revisit, which by definition printed them already.
        if let (Some(p), false) = (&h.parent, h.revisited) {
            if let Some(path) = &h.path {
                writeln!(out, "{pad}    file      {}", path.display())?;
            }
            if let Some(pr) = &p.producer {
                writeln!(out, "{pad}    producer  {}", pr.display())?;
                if full {
                    // Every recorded build field, not just the folded one-liner (the #417 surface).
                    for line in nav::producer_lines(pr).into_iter().skip(1) {
                        writeln!(out, "{pad}    {}", line.trim_start())?;
                    }
                }
            }
            if let Some(g) = &p.generation {
                writeln!(
                    out,
                    "{pad}    recipe    {}",
                    nav::generation_summary(g, full)
                )?;
            }
            for line in inherited_lines(p, h.depth, hops, root) {
                writeln!(out, "{pad}    {line}")?;
            }
        }
    }
    let gaps = hops.iter().filter(|h| h.kind.is_gap()).count();
    let leaves = hops
        .iter()
        .filter(|h| h.kind == HopKind::ExternalLeaf)
        .count();
    // Named before the summary line, and never folded into the gap count: an operator must not read
    // "1 gap" and reach for another `--search` when the answer is already on disk and broken.
    if !corrupt.is_empty() {
        writeln!(
            out,
            "\nCORRUPT candidate(s) — a .tsra whose manifest does not verify:"
        )?;
        for c in corrupt {
            writeln!(out, "  ! {}", c.path.display())?;
            writeln!(out, "      {}", c.detail)?;
        }
    }
    let mut verdicts = Vec::new();
    if verdict.corrupt > 0 {
        verdicts.push(format!("{} CORRUPT candidate(s)", verdict.corrupt));
    }
    if gaps > 0 {
        verdicts.push(format!("{gaps} gap(s)"));
    }
    writeln!(
        out,
        "\n{} hop(s) · {leaves} external leaf/leaves · {}",
        hops.len(),
        if verdict.complete {
            "chain complete".to_string()
        } else {
            format!("{} — chain INCOMPLETE", verdicts.join(" · "))
        }
    )?;
    Ok(())
}

/// Which schema-flagged identity fields a child shares with its resolved parent (ADR-0058 §5).
///
/// Stated as "matches", not "inherited": inheritance happened at seal time and left no record of
/// having happened, so what is observable now is that the values agree. Which fields even count is the
/// **schema's** declaration (`FieldSpec::inheritable`), never a list held here — the engine is
/// domain-agnostic, so `modality` flowing raw→derived is schema data, not CLI opinion.
fn shared_identity(child: &Manifest, parent: &Manifest) -> Vec<String> {
    let Some(schema) = parent
        .schema
        .as_ref()
        .and_then(|v| tessera_core::ProductSchema::from_value(v).ok())
    else {
        return Vec::new();
    };
    let mut shared: Vec<String> = schema
        .inheritable_fields()
        .iter()
        .filter_map(|f| {
            let pv = parent.metadata.get(&f.id)?;
            (child.metadata.get(&f.id) == Some(pv)).then(|| format!("{}={pv}", f.id))
        })
        .collect();
    // `study` is the format's own grouping primitive (a manifest field, not schema metadata), so it is
    // checked directly rather than via the schema's field list.
    if child.study.is_some() && child.study == parent.study {
        shared.insert(
            0,
            format!("study={}", parent.study.clone().unwrap_or_default()),
        );
    }
    shared
}

/// The rendered identity line for one hop, or nothing when no identity field is shared.
fn inherited_lines(parent: &Manifest, depth: usize, hops: &[Hop], root: &Manifest) -> Vec<String> {
    // The child of this hop is the node the edge left from: the root at depth 1, else the parent
    // recorded by the nearest shallower hop.
    let child = if depth <= 1 {
        Some(root)
    } else {
        hops.iter()
            .rev()
            .find(|h| h.depth == depth - 1)
            .and_then(|h| h.parent.as_ref())
    };
    let Some(child) = child else {
        return Vec::new();
    };
    let shared = shared_identity(child, parent);
    if shared.is_empty() {
        return Vec::new();
    }
    vec![format!("identity  {} (matches parent)", shared.join(" · "))]
}

fn write_json(
    root: &Manifest,
    hops: &[Hop],
    corrupt: &[Corrupt],
    verdict: Verdict,
    out: &mut impl Write,
) -> Result<()> {
    let hops_json: Vec<serde_json::Value> = hops
        .iter()
        .map(|h| {
            let mut o = serde_json::json!({
                "depth": h.depth,
                "role": h.role,
                "reference": h.reference,
                "outcome": h.kind.token(),
                "revisited": h.revisited,
            });
            let m = o.as_object_mut().expect("json! built an object");
            if let Some(p) = &h.pinned {
                m.insert("pinned".into(), serde_json::json!(p));
            }
            if let Some(rr) = &h.resolved {
                m.insert("resolved".into(), serde_json::json!(rr));
            }
            if let Some(p) = &h.path {
                m.insert("file".into(), serde_json::json!(p.display().to_string()));
            }
            if let Some(p) = &h.corrupt_file {
                m.insert(
                    "corrupt_file".into(),
                    serde_json::json!(p.display().to_string()),
                );
            }
            if let Some(p) = &h.parent {
                m.insert(
                    "product".into(),
                    serde_json::json!({
                        "id": p.id,
                        "manifest_hash": p.manifest_hash,
                        "product": p.product,
                        "name": p.name,
                        "timestamp": p.timestamp,
                        "study": p.study,
                        "producer": p.producer,
                        "generation": p.generation,
                    }),
                );
            }
            o
        })
        .collect();
    let doc = serde_json::json!({
        "root": {
            "id": root.id,
            "manifest_hash": root.manifest_hash,
            "product": root.product,
            "name": root.name,
        },
        "complete": verdict.complete,
        "gaps": hops.iter().filter(|h| h.kind.is_gap()).count(),
        "corrupt": corrupt
            .iter()
            .map(|c| serde_json::json!({"file": c.path.display().to_string(), "detail": c.detail}))
            .collect::<Vec<_>>(),
        "hops": hops_json,
    });
    writeln!(out, "{}", serde_json::to_string_pretty(&doc)?)?;
    Ok(())
}

/// The `Err` a caller raises when a searched location holds a product whose manifest does not verify.
///
/// Unconditional, unlike the incomplete-chain error: corruption is not a "you may want more data" state
/// that a gate opts into checking. The chain has already been rendered, so this only decides the code.
pub fn corruption_error(n: usize) -> Error {
    Error::Invalid(format!(
        "{n} candidate .tsra file(s) in the searched locations are Tessera products whose manifests do \
         not verify — listed above. This is corruption or tampering, not a missing parent: do not \
         resolve provenance against these files until they are re-fetched from a trusted source."
    ))
}

/// The `Err` a caller raises when `--require-complete` meets an incomplete chain — named so the message
/// says which knob to drop, not just that something failed.
pub fn incomplete_error(file: &Path) -> Error {
    Error::Invalid(format!(
        "provenance chain for {} is incomplete (--require-complete): \
         some parents resolved to a different version or were not found. \
         Point --search / --collection at them, or drop --require-complete to see the chain anyway.",
        file.display()
    ))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    /// Whether "nothing found" is a **gap** or a **leaf** is read off the reference's shape, because the
    /// alternative is reading the `role`, and roles are free-form strings the format never constrains — a
    /// producer that names its edges differently would have every one misclassified.
    ///
    /// The shape is specifically `blake3:<64 hex>`, the only form a product `id` takes. Accepting any
    /// `<alg>:<hex>` turned a `sha256:…` OCI digest or a vendor checksum into a phantom missing parent
    /// (#452 review).
    #[test]
    fn a_product_reference_is_blake3_shaped_and_nothing_else_is() {
        let hex64 = "ec73a94b678c7606ef782686054b595a9e17e892383ef8d54892eca0d17f1a81";
        assert!(looks_like_product_id(&format!("blake3:{hex64}")));
        for external in [
            // Other digest algorithms are not product ids, however digest-shaped they look.
            &format!("sha256:{hex64}") as &str,
            &format!("md5:{hex64}"),
            &format!("BLAKE3:{hex64}"), // the prefix is exact, not case-folded
            // Wrong width, even with the right prefix.
            "blake3:abc",
            &format!("blake3:{hex64}ff"),
            // Acquisition provenance: paths, filenames, SOP UIDs.
            "acq.cfg",
            "/scanner/raw.dcm",
            "1.2.840.113619.2.55.3.1234",
            "spec.toml",
            "nocolon",
            "",
        ] {
            assert!(
                !looks_like_product_id(external),
                "{external:?} must not read as a product id"
            );
        }
    }

    /// A search root names *where to look*, so a symlink must not quietly widen it (#452 review). Without
    /// the check, one link in a staging directory makes `provenance --search` read products from somewhere
    /// the operator never named.
    #[test]
    fn a_symlink_out_of_the_search_root_is_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let inside = dir.path().join("inside");
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&inside).unwrap();
        std::fs::create_dir_all(&outside).unwrap();

        // A real product outside the root, and a link to it from within.
        let real = outside.join("elsewhere.tsra");
        crate::tests::sample_tsra(&real);
        std::os::unix::fs::symlink(&real, inside.join("linked.tsra")).unwrap();
        // A copy genuinely inside the root, to prove the filter is not simply rejecting everything.
        std::fs::copy(&real, inside.join("local.tsra")).unwrap();

        let mut r = FileResolver::default();
        r.add_dir(&inside);
        // Distinct paths: one manifest is indexed under both its lineage id and its version, so the
        // index holds two entries per file.
        let indexed: BTreeSet<&Path> = r
            .by_id
            .values()
            .flatten()
            .map(|c| c.path.as_path())
            .collect();
        assert_eq!(
            indexed.len(),
            1,
            "only the file that really lives in the root may be indexed, got {indexed:?}"
        );
        assert!(
            indexed.iter().all(|p| p.ends_with("local.tsra")),
            "got {indexed:?}"
        );
        assert!(
            r.corrupt.is_empty(),
            "an excluded symlink is not corruption"
        );
    }

    /// Which hops count against `--require-complete`. The load-bearing claim is the *negative* one:
    /// an external leaf and an absent version pointer must NOT be gaps, or every real chain (which
    /// ends at vendor files) and every published artifact (which drops history) reads as incomplete
    /// and the flag stops meaning anything.
    #[test]
    fn only_unprovable_derivation_hops_count_as_gaps() {
        for k in [
            HopKind::Skew,
            HopKind::Missing,
            HopKind::CorruptParent,
            HopKind::Unpinned,
        ] {
            assert!(k.is_gap(), "{k:?} leaves the chain unproven");
        }
        for k in [
            HopKind::Proven,
            HopKind::ExternalLeaf,
            HopKind::VersionPointer,
        ] {
            assert!(!k.is_gap(), "{k:?} must not count against completeness");
        }
        // Every kind has a distinct machine token, so a `--json` consumer can switch on it.
        let all = [
            HopKind::Proven,
            HopKind::Skew,
            HopKind::Missing,
            HopKind::CorruptParent,
            HopKind::Unpinned,
            HopKind::ExternalLeaf,
            HopKind::VersionPointer,
        ];
        let tokens: BTreeSet<&str> = all.iter().map(|k| k.token()).collect();
        assert_eq!(
            tokens.len(),
            all.len(),
            "hop outcome tokens must be distinct"
        );
    }

    /// Which identity fields a hop reports as matching its parent is the **schema's** call, not the
    /// CLI's: `recon` marks `modality` inheritable (ADR-0058 §5), so it is reported, while a field the
    /// schema does not flag is ignored even when both sides happen to agree. A domain-agnostic engine
    /// cannot hold its own list of "identity" fields, and a CLI that hardcoded one would drift from the
    /// schema the moment a product added a field.
    #[test]
    fn shared_identity_reports_only_schema_flagged_fields() {
        let schema = tessera_core::SchemaRegistry::builtin()
            .get("recon")
            .expect("recon is a builtin schema")
            .clone();
        assert!(
            schema
                .inheritable_fields()
                .iter()
                .any(|f| f.id == "modality"),
            "this test rests on `recon` flagging modality inheritable"
        );
        let mut parent = Manifest::new("recon", "p", "d", "2024-01-01T00:00:00Z");
        parent.schema = Some(serde_json::to_value(&schema).unwrap());
        parent
            .metadata
            .insert("modality".into(), serde_json::json!("CT"));
        // Not schema-flagged: identical on both sides, and still must not be claimed as identity.
        parent
            .metadata
            .insert("description".into(), serde_json::json!("same on both"));
        let mut child = parent.clone();

        assert_eq!(shared_identity(&child, &parent), vec!["modality=\"CT\""]);

        // A child that overrides the field shares nothing — an explicit child value always wins.
        child
            .metadata
            .insert("modality".into(), serde_json::json!("PT"));
        assert!(shared_identity(&child, &parent).is_empty());

        // `study` is the format's own grouping primitive, so it is reported without being a schema
        // field — but only when the child actually carries one (absent != inherited).
        child
            .metadata
            .insert("modality".into(), serde_json::json!("CT"));
        parent.study = Some("DEMO-2024-01".into());
        assert_eq!(shared_identity(&child, &parent), vec!["modality=\"CT\""]);
        child.study = Some("DEMO-2024-01".into());
        assert_eq!(
            shared_identity(&child, &parent),
            vec!["study=DEMO-2024-01", "modality=\"CT\""]
        );
    }

    /// A resolver holding two versions of one lineage hands back the **pinned** one. Without
    /// `resolve_pinned`, `resolve` would return whichever version happens to be first and the walk
    /// would report skew by coin flip — the failure mode the trait's default impl documents.
    #[test]
    fn resolve_pinned_picks_the_pinned_version_not_the_first() {
        let mut v1 = Manifest::new("recon", "p", "d", "2024-01-01T00:00:00Z");
        v1.manifest_hash = Some("blake3:v1".into());
        let mut v2 = v1.clone();
        v2.manifest_hash = Some("blake3:v2".into());
        let id = v1.id.clone();

        let mut r = FileResolver::default();
        r.add(v1, PathBuf::from("v1.tsra"));
        r.add(v2, PathBuf::from("v2.tsra"));
        assert_eq!(r.versions(&id), 2);

        for want in ["blake3:v1", "blake3:v2"] {
            let got = r.resolve_pinned(&id, Some(want)).unwrap();
            assert_eq!(got.manifest_hash.as_deref(), Some(want));
        }
        // A pin nothing matches falls back to *a* version, which the walk then reports as skew
        // rather than as "not found" — the parent lineage IS here, just not at that version.
        let fallback = r.resolve_pinned(&id, Some("blake3:absent")).unwrap();
        assert!(fallback.manifest_hash.is_some());
        // Each version is also addressable by its own seal, for `snapshot_of`-shaped edges.
        assert_eq!(
            r.resolve("blake3:v2").and_then(|m| m.manifest_hash),
            Some("blake3:v2".to_string())
        );
    }
}
