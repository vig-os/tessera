//! Provenance as a DAG — each [`Source`] is a typed edge to an upstream artifact.
//!
//! e.g. a `recon` product has a `Source { role: "ingested_from", reference: "<DICOM path>",
//! content_hash: Some(...) }`; a lifetime `spectrum` has a `Source` to the `listmode` product
//! it was histogrammed from. This is fd5's `sources/` model, carried into the manifest.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::manifest::Manifest;
use crate::{Error, Result};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Source {
    /// Typed role of this edge, e.g. "ingested_from", "emission_data", "calibration".
    pub role: String,
    /// Identifier or path of the upstream artifact.
    pub reference: String,
    /// Content hash of the upstream artifact, when known (closes the integrity chain). Per SPEC §8
    /// this is the parent product's `manifest_hash` — the seal the edge commits to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
}

impl Source {
    pub fn new(role: impl Into<String>, reference: impl Into<String>) -> Self {
        Source {
            role: role.into(),
            reference: reference.into(),
            content_hash: None,
        }
    }

    /// Builder: pin the upstream's seal hash on this edge (closes the integrity chain).
    pub fn with_content_hash(mut self, hash: impl Into<String>) -> Self {
        self.content_hash = Some(hash.into());
        self
    }
}

/// Structured producer identity (ADR-0058 §1) — *who/what generated a product*. The universal,
/// domain-agnostic keys the format fixes; a generator (tessera itself, or an external DAQ/SIM/recon)
/// fills its own. Sealed inside the manifest, so it is tamper-evident and part of the product's id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Producer {
    /// The generating tool, e.g. `tessera`, `ge-listmode-daq`, a sim name.
    pub tool: String,
    pub version: String,
    /// Exact source commit of the tool, when known. tessera stamps its own via the
    /// `TESSERA_GIT_COMMIT` build env (absent in a sandboxed build ⇒ `None`, keeping the build
    /// deterministic); an external producer fills its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_repo: Option<String>,
    /// Working-tree dirty state at build, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dirty: Option<bool>,
}

impl Producer {
    /// An external producer's minimal identity (tool + version).
    pub fn new(tool: impl Into<String>, version: impl Into<String>) -> Self {
        Producer {
            tool: tool.into(),
            version: version.into(),
            git_commit: None,
            git_repo: None,
            dirty: None,
        }
    }

    /// Tessera's own identity, stamped at seal. `git_commit` is captured from the optional
    /// `TESSERA_GIT_COMMIT` build env — `None` when unset (e.g. the Nix sandbox), so the default
    /// stamp stays writer-deterministic within a build.
    pub fn tessera() -> Self {
        Producer {
            tool: "tessera".into(),
            version: crate::manifest::TESSERA_VERSION.into(),
            git_commit: option_env!("TESSERA_GIT_COMMIT").map(str::to_string),
            git_repo: option_env!("TESSERA_GIT_REPO").map(str::to_string),
            dirty: None,
        }
    }
}

/// The manifest `producer` slot: a structured [`Producer`] (ADR-0058) **or** a legacy bare string
/// (`"tessera/0.0.0"`, pre-ADR-0058). Untagged so a legacy string round-trips **byte-identically**
/// — existing seals hold, no corpus break — while newly-sealed products carry the struct.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ProducerRef {
    Structured(Producer),
    Legacy(String),
}

impl ProducerRef {
    /// Tessera's own structured identity (the default seal stamp).
    pub fn tessera() -> Self {
        ProducerRef::Structured(Producer::tessera())
    }

    /// The generating tool name, whichever form. A legacy `"tool/version"` splits on the first `/`.
    pub fn tool(&self) -> &str {
        match self {
            ProducerRef::Structured(p) => &p.tool,
            ProducerRef::Legacy(s) => s.split('/').next().unwrap_or(s),
        }
    }

    /// The tool version, whichever form.
    pub fn version(&self) -> &str {
        match self {
            ProducerRef::Structured(p) => &p.version,
            ProducerRef::Legacy(s) => s.split_once('/').map(|(_, v)| v).unwrap_or(""),
        }
    }

    /// One-line `tool/version` for display (round-trips a legacy string verbatim).
    pub fn display(&self) -> String {
        match self {
            ProducerRef::Structured(p) => match &p.git_commit {
                Some(c) => format!("{}/{} ({c})", p.tool, p.version),
                None => format!("{}/{}", p.tool, p.version),
            },
            ProducerRef::Legacy(s) => s.clone(),
        }
    }
}

/// One **recorded transform** applied at the door (ADR-0056 §2/§6.2) — the middle lane of the
/// normalise-vs-preserve ladder, made honest.
///
/// Generic ingest maps a foreign type system onto Tessera's flat one, and some of those mappings are
/// *reversible transformations* rather than identities: an Arrow `Decimal128(18,4)` becomes an `i8`
/// column plus a `scale`, a `Timestamp(_, Some("America/New_York"))` becomes UTC ticks, a
/// `Dictionary` becomes its materialised values. Each is lossless **only because its parameters are
/// recorded**, and ADR-0056 §2 rejected the alternative of printing a warning: a warning does not
/// travel with the artifact, and a seal over silently-degraded values asserts under a signature that
/// those values are the truth. So the transform list rides **inside the seal** and the artifact
/// carries its own recovery instructions.
///
/// `params` is a bag rather than a typed union on purpose — the set of transforms grows with the
/// source formats we accept, and a closed enum in the *format* would make every new decoder a format
/// revision. The names ADR-0056 §6.2 fixes are `tz_to_utc`, `decimal_fixed_point`, `f16_widen`,
/// `dictionary_materialised`, `fixed_list_expand`, `null_slot_normalisation`, `struct_flatten` and
/// `csv_explicit_schema`; a transform that names a single column puts it in `params["column"]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IngestTransform {
    /// The transform's stable name (`"decimal_fixed_point"`, `"tz_to_utc"`, …).
    pub name: String,
    /// The transform's parameters — everything a reader needs to invert it, plus `column` when the
    /// transform applies to one. `BTreeMap` so canonical JSON is key-ordered and the seal is stable.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, serde_json::Value>,
}

impl IngestTransform {
    /// A transform with no parameters (e.g. `f16_widen`).
    pub fn new(name: impl Into<String>) -> Self {
        IngestTransform {
            name: name.into(),
            params: BTreeMap::new(),
        }
    }

    /// Builder: record one parameter.
    pub fn with(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.params.insert(key.into(), value);
        self
    }

    /// Builder: record the column this transform applied to (the `column` parameter).
    pub fn on_column(self, column: &str) -> Self {
        self.with("column", serde_json::Value::String(column.to_string()))
    }
}

/// Generation record (ADR-0058 §2) — *how a product was made*: a generic, **non-opinionated bag**.
/// The format enforces only that it is present + non-empty for products whose schema requires a
/// recipe; the `config` keys are the generator's business and are never inspected by the engine.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Generation {
    /// Free-form settings the generator used (energy window, coincidence window, TOF cal, quant
    /// scales, sim seed, cmd, …). Keys are opaque to the format.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub config: BTreeMap<String, serde_json::Value>,
    /// Alternative to inline `config`: a `blake3:` digest of a config / `.ini` / `.cfg` **block
    /// carried in this `.tsra`** (ADR-0038 Blob) — bit-faithful and dedup'd for large vendor config.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_ref: Option<String>,
}

impl Generation {
    /// A recipe is "present" iff it carries inline settings or points at a carried config block.
    pub fn is_empty(&self) -> bool {
        self.config.is_empty() && self.config_ref.is_none()
    }

    /// Builder: inline settings.
    pub fn with(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.config.insert(key.into(), value);
        self
    }

    /// Builder: point at a carried config block by digest.
    pub fn with_config_ref(mut self, digest: impl Into<String>) -> Self {
        self.config_ref = Some(digest.into());
        self
    }
}

/// Copy **inheritable identity** fields from `parent` into `child`'s metadata (ADR-0058 §5),
/// schema-driven: a field flows iff the `schema` marks it [`inherit`](crate::schema::FieldSpec::inherit)
/// **and** the child does not already set it (an explicit child value always wins). The engine holds
/// no field list — *which* fields are identity is the schema's declaration. The first-class `study`
/// grouping key (a manifest field, not schema metadata) is inherited when unset, as it is the
/// format's own grouping primitive rather than domain opinion.
pub fn inherit_identity(
    child: &mut Manifest,
    parent: &Manifest,
    schema: &crate::schema::ProductSchema,
) {
    for f in schema.inheritable_fields() {
        if !child.metadata.contains_key(&f.id) {
            if let Some(v) = parent.metadata.get(&f.id) {
                child.metadata.insert(f.id.clone(), v.clone());
            }
        }
    }
    if child.study.is_none() {
        child.study = parent.study.clone();
    }
}

/// Resolves a provenance `reference` (a parent product's `id`) to its manifest, so a chain can be
/// walked and verified.
///
/// **A `reference` is a lineage handle, not a version.** ADR-0036 makes `id` stable across a product's
/// commits while `manifest_hash` is the version, so one `reference` legitimately addresses *many*
/// manifests and a resolver has to choose. Returning a different version than an edge pinned is a
/// normal outcome, reported as [`EdgeOutcome::VersionSkew`] — not corruption. A resolver that wants an
/// exact version should match on the edge's pinned hash; the `BTreeMap<id, Manifest>` impl below holds
/// one manifest per id and so cannot, which is why it never exercises the skew path.
pub trait Resolver {
    fn resolve(&self, reference: &str) -> Option<Manifest>;

    /// Resolve `reference`, preferring the exact version an edge pinned when this store holds more
    /// than one.
    ///
    /// Default: ignore the hint and defer to [`Resolver::resolve`] — correct for a store with one
    /// manifest per `id`, which cannot choose anyway. A store that indexes by `(id, manifest_hash)`
    /// should override it; otherwise a walk over a store holding two versions of one parent reports
    /// skew by coin flip, which is worse than not knowing.
    fn resolve_pinned(&self, reference: &str, pinned: Option<&str>) -> Option<Manifest> {
        let _ = pinned;
        self.resolve(reference)
    }
}

impl Resolver for BTreeMap<String, Manifest> {
    fn resolve(&self, reference: &str) -> Option<Manifest> {
        self.get(reference).cloned()
    }
}

/// What resolving one provenance edge produced. The walk's whole job is telling these apart, because
/// collapsing them is how a version mismatch gets reported as corruption (or corruption as routine).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EdgeOutcome {
    /// The parent resolved and seals exactly the version the edge pinned. The chain holds here.
    Pinned,
    /// The parent resolved by lineage (`id` matched) but seals a *different* version than the edge
    /// pinned. ADR-0036 makes `id` a lineage handle addressing many versions, so this is the ordinary
    /// result of resolving against a store holding a newer commit of the parent — a gap in what this
    /// walk can prove, not evidence that anything is wrong with the bytes.
    VersionSkew {
        /// The `manifest_hash` the edge committed to.
        pinned: String,
        /// The `manifest_hash` the resolved parent actually carries.
        resolved: String,
    },
    /// Nothing resolved. Either an external (non-Tessera) leaf — a raw DICOM SOP UID, a vendor file
    /// path — or a parent product this walk simply cannot reach from where it is looking.
    Unresolved,
    /// The edge carries no pinned hash, so there is nothing to check even if it resolves.
    Unpinned,
}

/// The role `publish` writes for its version breadcrumb (ADR-0036) — the one version-pointer role the
/// format itself emits.
pub const SNAPSHOT_OF_ROLE: &str = "snapshot_of";

/// Is this edge a **version pointer** rather than a derivation edge?
///
/// A derivation edge references a parent's *lineage* `id` and pins that parent's `manifest_hash`, so
/// the two values differ. ADR-0036's `snapshot_of` breadcrumb instead references the version itself, so
/// reference and pin are the same string.
///
/// Both halves are required, and the role half is the security-relevant one. Shape alone is **forgeable**:
/// a crafted `derived_from` whose reference equals its pin would be exempted from descent and from
/// completeness, quietly passing a `--require-complete` gate it never satisfied (#452 review). So the
/// role must be the one the format writes for this purpose. That is not "guessing semantics from a
/// free-form label" — it is recognising *our own* emitted breadcrumb; a producer's arbitrary role gets no
/// exemption. [`is_self_snapshot`] adds the third check once a parent actually resolves.
pub fn is_version_pointer(s: &Source) -> bool {
    s.role == SNAPSHOT_OF_ROLE && s.content_hash.as_deref() == Some(s.reference.as_str())
}

/// A resolved version pointer that really points **into its own lineage** — the case [`walk`] must not
/// recurse through, because a published artifact keeps its lineage `id` while naming an earlier version
/// of *itself*, so descending re-walks the same product and trips the cycle guard on a well-formed DAG.
/// An edge that resolves to a *different* lineage is not a self-snapshot whatever it is called, and is
/// walked like any other parent.
fn is_self_snapshot(s: &Source, from: &Manifest, parent: &Manifest) -> bool {
    is_version_pointer(s) && parent.id == from.id
}

/// A node reached during a [`walk`], with the edge that led to it.
#[derive(Debug, Clone)]
pub struct Step<'a> {
    /// The manifest the edge leaves *from*.
    pub from: &'a Manifest,
    /// The edge itself.
    pub source: &'a Source,
    /// What resolving it produced.
    pub outcome: EdgeOutcome,
    /// The parent this edge resolved to, when one did. Carried so a visitor never has to resolve the
    /// edge a second time — a second lookup is a second chance for the walk and the render to disagree
    /// about what the DAG is, which is the whole reason there is only one traversal.
    pub parent: Option<&'a Manifest>,
    /// Hops from the root (the root's own edges are depth 1).
    pub depth: usize,
    /// True when this parent was already reached by another path — a diamond. Reported once so a
    /// renderer can mark the repeat instead of printing the subtree twice.
    pub revisited: bool,
}

/// Observes a [`walk`]. One walk, many uses: `verify_chain` is the strict visitor that refuses any
/// edge it cannot prove, while an operator view collects the same steps and renders the gaps.
pub trait Visit {
    /// Called once per edge, parent-first, before descending into it. Returning `Err` aborts the walk
    /// and the error reaches the [`walk`] caller — which is exactly how the strict verifier fails.
    fn edge(&mut self, step: &Step<'_>) -> Result<()>;
}

/// Walk the provenance DAG rooted at `manifest`, reporting every edge to `visitor`.
///
/// One traversal shared by every consumer (#452): the verifier and the operator view must agree about
/// what the DAG *is*, and two walks would drift. Properties:
/// - **Cycles are a hard error** — a product cannot descend from itself, so this is a malformed DAG
///   rather than something to render.
/// - **Diamonds are visited once**, flagged via [`Step::revisited`]. Re-walking a shared subtree can
///   only reach the same verdict, so skipping it is both cheaper and what a reader wants to see.
/// - **Self-lineage version pointers are not recursed through** (see [`is_self_snapshot`]) — they name a
///   version of the walked product's own lineage, not a parent to visit.
/// - `max_depth` caps how far to descend (`None` = unlimited). Edges *at* the cap are still reported;
///   their children are not.
pub fn walk<R: Resolver, V: Visit>(
    manifest: &Manifest,
    resolver: &R,
    max_depth: Option<usize>,
    visitor: &mut V,
) -> Result<()> {
    let mut on_stack = BTreeSet::new();
    let mut seen = BTreeSet::new();
    walk_inner(
        manifest,
        resolver,
        max_depth,
        visitor,
        &mut on_stack,
        &mut seen,
        0,
    )
}

/// A node's identity for traversal bookkeeping: lineage `id` **plus** version.
///
/// Keyed by the pair, not the `id`, because an `id` addresses many versions (ADR-0036). Two paths that
/// pin *different* versions of one lineage are different nodes and both deserve a walk, and a cycle then
/// means the same **version** re-entered — unambiguously a loop, rather than a lineage that legitimately
/// appears twice at different versions.
fn node_key(m: &Manifest) -> (String, String) {
    (m.id.clone(), m.manifest_hash.clone().unwrap_or_default())
}

fn walk_inner<R: Resolver, V: Visit>(
    m: &Manifest,
    resolver: &R,
    max_depth: Option<usize>,
    visitor: &mut V,
    on_stack: &mut BTreeSet<(String, String)>,
    seen: &mut BTreeSet<(String, String)>,
    depth: usize,
) -> Result<()> {
    let key = node_key(m);
    if !on_stack.insert(key.clone()) {
        return Err(Error::Invalid(format!(
            "provenance cycle detected at product '{}'",
            m.id
        )));
    }
    for s in &m.sources {
        let resolved = resolver.resolve_pinned(&s.reference, s.content_hash.as_deref());
        let outcome = match (&s.content_hash, &resolved) {
            (None, _) => EdgeOutcome::Unpinned,
            (Some(_), None) => EdgeOutcome::Unresolved,
            (Some(pinned), Some(parent)) => {
                let actual = parent.manifest_hash.clone().unwrap_or_default();
                if &actual == pinned {
                    EdgeOutcome::Pinned
                } else {
                    EdgeOutcome::VersionSkew {
                        pinned: pinned.clone(),
                        resolved: actual,
                    }
                }
            }
        };
        let parent_key = resolved.as_ref().map(node_key);
        // **Cycle before diamond.** A parent enters `seen` before we descend into it, so every ancestor
        // on the current stack is in `seen` too. Testing `seen` first therefore makes the edge that
        // *closes* a loop look like the second arm of a diamond — reported, skipped, walk returns Ok,
        // cycle masked (#452 review). The real distinction: a diamond's shared parent is **finished**,
        // a cycle's target is **still on the stack**. So exclude on-stack nodes from `revisited` and let
        // them fall through to the recursive entry guard, which is the one place a cycle is raised.
        let on_stack_now = parent_key.as_ref().is_some_and(|k| on_stack.contains(k));
        let revisited = !on_stack_now && parent_key.as_ref().is_some_and(|k| seen.contains(k));
        visitor.edge(&Step {
            from: m,
            source: s,
            outcome,
            parent: resolved.as_ref(),
            depth: depth + 1,
            revisited,
        })?;
        let Some(parent) = resolved.as_ref() else {
            continue;
        };
        if revisited || is_self_snapshot(s, m, parent) || max_depth.is_some_and(|d| depth + 1 >= d)
        {
            continue;
        }
        seen.insert(parent_key.expect("a resolved parent always has a key"));
        walk_inner(
            parent,
            resolver,
            max_depth,
            visitor,
            on_stack,
            seen,
            depth + 1,
        )?;
    }
    on_stack.remove(&key); // pop: only a genuine cycle fails, a diamond does not
    Ok(())
}

/// The strict visitor behind [`verify_chain`]: an edge it cannot *prove* is an error.
struct StrictVerifier;

impl Visit for StrictVerifier {
    fn edge(&mut self, step: &Step<'_>) -> Result<()> {
        match &step.outcome {
            // Nothing pinned, or an external leaf that is acquisition provenance rather than a
            // Tessera product — neither is a claim this can check, so neither is a failure.
            EdgeOutcome::Pinned | EdgeOutcome::Unpinned | EdgeOutcome::Unresolved => Ok(()),
            EdgeOutcome::VersionSkew { pinned, resolved } => Err(Error::ProvenanceVersionSkew {
                role: step.source.role.clone(),
                reference: step.source.reference.clone(),
                pinned: pinned.clone(),
                resolved: resolved.clone(),
            }),
        }
    }
}

/// Verify the provenance chain rooted at `manifest`: every [`Source`] edge that carries a
/// `content_hash` **and** resolves to a parent product must have that hash equal the parent's
/// `manifest_hash` (SPEC §8 — the edge commits to the parent's seal), recursively to the roots.
/// Edges that don't resolve (external leaves — e.g. a raw DICOM SOP UID or file path) are skipped:
/// they are acquisition provenance, not Tessera products. A genuine cycle is a hard error.
///
/// Strict by design: it answers "is this chain proven?", so a resolver handing back a *different
/// version* of the right parent fails. That failure is [`Error::ProvenanceVersionSkew`], never
/// [`Error::Integrity`] — the bytes are fine, the version is not the pinned one, and an auditor has to
/// be able to tell those apart. Callers that want the gap rendered instead of raised should drive
/// [`walk`] with their own [`Visit`].
pub fn verify_chain<R: Resolver>(manifest: &Manifest, resolver: &R) -> Result<()> {
    walk(manifest, resolver, None, &mut StrictVerifier)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ProductBuilder;

    const TS: &str = "2024-01-01T00:00:00Z";

    fn sealed(name: &str) -> Manifest {
        ProductBuilder::new("recon", name, "d", TS).seal().unwrap()
    }

    #[test]
    fn verify_chain_accepts_correct_edge_and_skips_external_leaf() {
        let parent = sealed("parent");
        let mut store = BTreeMap::new();
        store.insert(parent.id.clone(), parent.clone());

        let mut cb = ProductBuilder::new("recon", "child", "d", TS);
        cb.add_source(
            Source::new("derived_from", &parent.id)
                .with_content_hash(parent.manifest_hash.clone().unwrap()),
        );
        cb.add_source(Source::new("ingested_from", "/scanner/raw.dcm")); // external leaf — skipped
        let child = cb.seal().unwrap();

        verify_chain(&child, &store).unwrap();
    }

    /// An edge whose pinned hash is not the seal the resolved parent carries fails `verify_chain` —
    /// but as [`Error::ProvenanceVersionSkew`], NOT [`Error::Integrity`].
    ///
    /// The distinction is the whole point (#452). A chain check compares a pinned hash to a resolved
    /// parent's seal; it cannot see bytes, so it cannot distinguish "hand-edited to deadbeef" from
    /// "pinned an older version of a product that has since been committed again". Reporting both as
    /// `Integrity` — "tampering, corruption, or a producer bug" — makes routine version skew look like
    /// an attack, and teaches operators to wave away the one error that should never be waved away.
    /// Real corruption is caught by payload verification (`Error::BlockIntegrity`), a different
    /// mechanism reading actual bytes.
    #[test]
    fn verify_chain_reports_a_hash_mismatch_as_version_skew_not_corruption() {
        let parent = sealed("parent");
        let mut store = BTreeMap::new();
        store.insert(parent.id.clone(), parent.clone());

        let mut cb = ProductBuilder::new("recon", "child", "d", TS);
        cb.add_source(Source::new("derived_from", &parent.id).with_content_hash("blake3:deadbeef"));
        let child = cb.seal().unwrap();

        match verify_chain(&child, &store) {
            Err(Error::ProvenanceVersionSkew {
                role,
                reference,
                pinned,
                resolved,
            }) => {
                assert_eq!(role, "derived_from");
                assert_eq!(reference, parent.id);
                assert_eq!(pinned, "blake3:deadbeef");
                assert_eq!(resolved, parent.manifest_hash.clone().unwrap());
            }
            other => panic!("expected ProvenanceVersionSkew, got {other:?}"),
        }
    }

    /// The real-world shape of skew, and the reason it must not be called corruption: **one `id`
    /// addresses many versions** (ADR-0036 — `id` is the lineage handle, `manifest_hash` the version).
    /// A child pins the version of the parent it was actually built from; a store that has since
    /// committed the parent again resolves the *newer* one. Nothing is corrupt, and the walk says so.
    #[test]
    fn one_lineage_id_addresses_two_versions_and_the_newer_one_reads_as_skew() {
        // Two versions of ONE product: same identity inputs (product/name/timestamp) ⇒ same `id`,
        // different metadata ⇒ different `manifest_hash`. This is what `tessera commit --set` makes.
        let v1 = ProductBuilder::new("recon", "parent", "d", TS)
            .seal()
            .unwrap();
        let mut b2 = ProductBuilder::new("recon", "parent", "d", TS);
        b2.with_field("study", serde_json::json!("RESTAGED-02"));
        let v2 = b2.seal().unwrap();
        assert_eq!(
            v1.id, v2.id,
            "same identity inputs must give the same lineage id"
        );
        assert_ne!(
            v1.manifest_hash, v2.manifest_hash,
            "different content must give a different version"
        );

        // The child was built from v1 and pins it.
        let mut cb = ProductBuilder::new("recon", "child", "d", TS);
        cb.add_source(
            Source::new("derived_from", &v1.id)
                .with_content_hash(v1.manifest_hash.clone().unwrap()),
        );
        let child = cb.seal().unwrap();

        // Resolved against a store holding v1: proven.
        let mut at_v1 = BTreeMap::new();
        at_v1.insert(v1.id.clone(), v1.clone());
        verify_chain(&child, &at_v1).expect("the pinned version must verify");

        // Resolved against a store holding v2: the same lineage, a different version.
        let mut at_v2 = BTreeMap::new();
        at_v2.insert(v2.id.clone(), v2.clone());
        let mut seen = Vec::new();
        walk(&child, &at_v2, None, &mut Collector(&mut seen)).unwrap();
        assert_eq!(
            seen,
            vec![EdgeOutcome::VersionSkew {
                pinned: v1.manifest_hash.clone().unwrap(),
                resolved: v2.manifest_hash.clone().unwrap(),
            }],
            "the newer version of the right lineage is skew, not corruption"
        );
    }

    /// A visitor that records outcomes and never fails — the operator view's shape, and what lets a
    /// caller render a gap instead of raising it.
    struct Collector<'a>(&'a mut Vec<EdgeOutcome>);

    impl Visit for Collector<'_> {
        fn edge(&mut self, step: &Step<'_>) -> Result<()> {
            self.0.push(step.outcome.clone());
            Ok(())
        }
    }

    /// A diamond (one parent reached by two paths) is reported once and never walked twice, while a
    /// genuine cycle is still a hard error. Both are properties of the shared walk, so both consumers
    /// inherit them rather than each re-deriving the traversal.
    #[test]
    fn walk_visits_a_diamond_once_and_still_rejects_a_cycle() {
        let root = sealed("shared-root");
        let pin = root.manifest_hash.clone().unwrap();
        let mk_mid = |name: &str| {
            let mut b = ProductBuilder::new("recon", name, "d", TS);
            b.add_source(Source::new("derived_from", &root.id).with_content_hash(pin.clone()));
            b.seal().unwrap()
        };
        let left = mk_mid("left");
        let right = mk_mid("right");

        let mut store = BTreeMap::new();
        for m in [&root, &left, &right] {
            store.insert(m.id.clone(), m.clone());
        }
        let mut leaf = ProductBuilder::new("recon", "leaf", "d", TS);
        for m in [&left, &right] {
            leaf.add_source(
                Source::new("derived_from", &m.id)
                    .with_content_hash(m.manifest_hash.clone().unwrap()),
            );
        }
        let leaf = leaf.seal().unwrap();

        // 4 edges: leaf→left, left→root, leaf→right, right→root. The second arrival at `root` is
        // reported (so a renderer can mark the diamond) but its subtree is not re-walked.
        let mut revisits = 0;
        struct CountRevisits<'a>(&'a mut usize, &'a mut usize);
        impl Visit for CountRevisits<'_> {
            fn edge(&mut self, step: &Step<'_>) -> Result<()> {
                *self.1 += 1;
                if step.revisited {
                    *self.0 += 1;
                }
                Ok(())
            }
        }
        let mut edges = 0;
        walk(
            &leaf,
            &store,
            None,
            &mut CountRevisits(&mut revisits, &mut edges),
        )
        .unwrap();
        assert_eq!(edges, 4, "every edge is reported");
        assert_eq!(
            revisits, 1,
            "the shared root is flagged on its second arrival"
        );
        verify_chain(&leaf, &store).expect("a diamond is a valid DAG, not a cycle");

        // A genuine cycle: a -> b -> a, with both edges pinning the seal the other side carries, so
        // the traversal reaches the cycle check rather than stopping at a version mismatch first.
        // Built by hand because a sealing builder cannot produce mutually-pinned hashes.
        let mut a = Manifest::new("recon", "a", "d", TS);
        let mut b = Manifest::new("recon", "b", "d", TS);
        a.manifest_hash = Some("blake3:aaa".into());
        b.manifest_hash = Some("blake3:bbb".into());
        a.sources
            .push(Source::new("derived_from", &b.id).with_content_hash("blake3:bbb"));
        b.sources
            .push(Source::new("derived_from", &a.id).with_content_hash("blake3:aaa"));
        let mut cyc = BTreeMap::new();
        cyc.insert(a.id.clone(), a.clone());
        cyc.insert(b.id.clone(), b.clone());
        assert!(
            matches!(verify_chain(&a, &cyc), Err(Error::Invalid(m)) if m.contains("cycle")),
            "a -> b -> a must stay a hard error"
        );
    }

    /// A published artifact's `snapshot_of` breadcrumb points back at an earlier version of its OWN
    /// lineage (ADR-0036 — `publish` keeps `id`, drops history). Recursing through it re-walks the same
    /// product and trips the cycle guard, so a well-formed published artifact would be rejected as a
    /// malformed DAG the moment its source version is also on hand. Report the edge, never descend it.
    #[test]
    fn a_version_pointer_at_a_present_version_is_not_a_cycle() {
        // One lineage, two versions; the published copy points back at the original version.
        let original = sealed("published-product");
        let mut snapshot = original.clone();
        snapshot.manifest_hash = Some("blake3:snapshotseal".into());
        let pin = original.manifest_hash.clone().unwrap();
        snapshot
            .sources
            .push(Source::new("snapshot_of", &pin).with_content_hash(&pin));
        assert_eq!(snapshot.id, original.id, "publish keeps the lineage id");

        // A store where the pointed-at version IS resolvable — by its own seal.
        struct ByVersion(Manifest);
        impl Resolver for ByVersion {
            fn resolve(&self, reference: &str) -> Option<Manifest> {
                (self.0.manifest_hash.as_deref() == Some(reference)).then(|| self.0.clone())
            }
        }
        let store = ByVersion(original);

        verify_chain(&snapshot, &store)
            .expect("a version pointer into one's own lineage is not a cycle");
        let mut seen = Vec::new();
        walk(&snapshot, &store, None, &mut Collector(&mut seen)).unwrap();
        assert_eq!(
            seen,
            vec![EdgeOutcome::Pinned],
            "the edge is reported once, and not recursed through"
        );
    }

    /// The version-pointer exemption must not be **forgeable**. Skipping descent and skipping
    /// completeness is a real privilege, so shape alone cannot earn it: a crafted `derived_from` whose
    /// reference happens to equal its pin would otherwise go unwalked and pass a `--require-complete`
    /// gate it never satisfied (#452 review). Two further checks close that: the role must be the one
    /// the format itself writes, and a resolved pointer must land in the walked product's OWN lineage.
    #[test]
    fn the_version_pointer_exemption_is_not_forgeable() {
        let parent = sealed("a-real-parent");
        let pin = parent.manifest_hash.clone().unwrap();

        // Forged: pointer SHAPE (reference == pin) but a derivation role. Not exempt.
        let forged = Source::new("derived_from", &pin).with_content_hash(&pin);
        assert!(!is_version_pointer(&forged));
        // The format's own breadcrumb role, same shape. Exempt by shape+role.
        let genuine = Source::new(SNAPSHOT_OF_ROLE, &pin).with_content_hash(&pin);
        assert!(is_version_pointer(&genuine));

        // …but only into its OWN lineage. Pointing at a different lineage is an ordinary parent, so it
        // is still walked — the `snapshot_of` label buys nothing on someone else's product.
        let mut child = ProductBuilder::new("recon", "child", "d", TS);
        child.add_source(genuine.clone());
        let child = child.seal().unwrap();
        assert_ne!(child.id, parent.id);
        assert!(!is_self_snapshot(&genuine, &child, &parent));
        assert!(is_self_snapshot(&genuine, &parent, &parent));

        // End to end: a forged edge is walked, so the parent it names is verified rather than waved
        // through. Resolving the forged reference (a manifest_hash) to a DIFFERENT version proves the
        // walk really descended instead of exempting it.
        let mut skewed = parent.clone();
        skewed.manifest_hash = Some("blake3:adifferentversion".into());
        struct ByRef(String, Manifest);
        impl Resolver for ByRef {
            fn resolve(&self, reference: &str) -> Option<Manifest> {
                (reference == self.0).then(|| self.1.clone())
            }
        }
        let mut forged_child = ProductBuilder::new("recon", "forged", "d", TS);
        forged_child.add_source(forged);
        let forged_child = forged_child.seal().unwrap();
        assert!(
            matches!(
                verify_chain(&forged_child, &ByRef(pin.clone(), skewed)),
                Err(Error::ProvenanceVersionSkew { .. })
            ),
            "a forged version pointer must be checked like any other edge"
        );
    }

    /// A cycle that does **not** pass through the root must still be a hard error: `r → b → c → b`.
    ///
    /// Regression guard for the diamond optimisation (#452 review). Marking a parent "already seen"
    /// before descending into it puts every *ancestor* in that set too, so testing it before the
    /// cycle guard makes the edge that closes a loop look like the second arm of a diamond — reported,
    /// skipped, and the walk returns `Ok`. The distinction is that a diamond's shared parent has been
    /// *finished*, while a cycle's target is still on the stack. Cycle first, diamond second.
    #[test]
    fn a_cycle_below_the_root_is_still_rejected() {
        // r → b → c → b, all edges pinning the seal each parent carries.
        let mut r = Manifest::new("recon", "r", "d", TS);
        let mut b = Manifest::new("recon", "b", "d", TS);
        let mut c = Manifest::new("recon", "c", "d", TS);
        r.manifest_hash = Some("blake3:rrr".into());
        b.manifest_hash = Some("blake3:bbb".into());
        c.manifest_hash = Some("blake3:ccc".into());
        r.sources
            .push(Source::new("derived_from", &b.id).with_content_hash("blake3:bbb"));
        b.sources
            .push(Source::new("derived_from", &c.id).with_content_hash("blake3:ccc"));
        c.sources
            .push(Source::new("derived_from", &b.id).with_content_hash("blake3:bbb"));
        let mut store = BTreeMap::new();
        for m in [&r, &b, &c] {
            store.insert(m.id.clone(), m.clone());
        }

        assert!(
            matches!(verify_chain(&r, &store), Err(Error::Invalid(m)) if m.contains("cycle")),
            "r -> b -> c -> b is a cycle even though it never revisits the root"
        );
        // And a visitor-driven walk must not quietly succeed either.
        let mut seen = Vec::new();
        assert!(
            walk(&r, &store, None, &mut Collector(&mut seen)).is_err(),
            "the operator walk must surface the cycle, not render a clean chain"
        );
    }

    /// `max_depth` reports the edges at the cap but stops descending — the `--depth N` operator knob.
    #[test]
    fn walk_depth_cap_reports_the_boundary_edge_without_descending() {
        let gp = sealed("grandparent");
        let mut pb = ProductBuilder::new("recon", "parent", "d", TS);
        pb.add_source(
            Source::new("derived_from", &gp.id)
                .with_content_hash(gp.manifest_hash.clone().unwrap()),
        );
        let parent = pb.seal().unwrap();
        let mut cb = ProductBuilder::new("recon", "child", "d", TS);
        cb.add_source(
            Source::new("derived_from", &parent.id)
                .with_content_hash(parent.manifest_hash.clone().unwrap()),
        );
        let child = cb.seal().unwrap();

        let mut store = BTreeMap::new();
        for m in [&gp, &parent] {
            store.insert(m.id.clone(), m.clone());
        }
        for (cap, want) in [(Some(1), 1), (Some(2), 2), (None, 2)] {
            let mut seen = Vec::new();
            walk(&child, &store, cap, &mut Collector(&mut seen)).unwrap();
            assert_eq!(
                seen.len(),
                want,
                "depth cap {cap:?} should report {want} edge(s)"
            );
        }
    }

    /// Back-compat (ADR-0058): a pre-ADR-0058 manifest carries `producer` as a bare string. Reading
    /// then re-serializing MUST reproduce the exact string (not a struct), so the seal over the old
    /// bytes still holds and the conformance corpus / DP01 archive are not broken.
    #[test]
    fn legacy_producer_string_round_trips_byte_identical() {
        let json = r#"{"tessera_version":"0.0.0","id":"blake3:x","id_inputs":{},"product":"recon","name":"n","description":"d","timestamp":"2024-01-01T00:00:00Z","producer":"tessera/0.0.0"}"#;
        let m: Manifest = serde_json::from_str(json).unwrap();
        assert!(
            matches!(&m.producer, Some(ProducerRef::Legacy(s)) if s == "tessera/0.0.0"),
            "a bare string must parse as Legacy, got {:?}",
            m.producer
        );
        let back = serde_json::to_value(&m).unwrap();
        assert_eq!(
            back["producer"],
            serde_json::json!("tessera/0.0.0"),
            "a legacy string must NOT be rewritten as a struct (would break the seal)"
        );
        // The accessors read it uniformly.
        let p = m.producer.unwrap();
        assert_eq!(p.tool(), "tessera");
        assert_eq!(p.version(), "0.0.0");

        // The load-bearing claim: a manifest SEALED with a legacy string producer still VERIFIES
        // after a JSON round-trip — i.e. the canonical bytes the seal is computed over are reproduced
        // byte-for-byte, so an existing sealed .tsra never fails integrity under the new reader.
        let mut sealed = Manifest::new("recon", "n", "d", TS);
        sealed.producer = Some(ProducerRef::Legacy("acme-daq/1.2".into()));
        sealed.manifest_hash = Some(sealed.compute_manifest_hash().unwrap());
        let reparsed = Manifest::from_json(&sealed.to_json().unwrap()).unwrap();
        reparsed
            .verify()
            .expect("legacy-producer seal must still verify after round-trip");
        assert!(
            matches!(&reparsed.producer, Some(ProducerRef::Legacy(s)) if s == "acme-daq/1.2"),
            "the legacy producer survives the round-trip unchanged"
        );
    }

    /// A newly-sealed product carries the structured producer (tessera stamps its own tool+version),
    /// serialized as a map.
    #[test]
    fn structured_producer_is_stamped_on_seal() {
        let m = ProductBuilder::new("recon", "n", "d", TS).seal().unwrap();
        match &m.producer {
            Some(ProducerRef::Structured(p)) => assert_eq!(p.tool, "tessera"),
            other => panic!("expected a structured producer, got {other:?}"),
        }
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(v["producer"]["tool"], "tessera");
    }

    /// ADR-0058 §5: identity inheritance is **schema-driven** — only fields the schema flags
    /// `inherit` flow from parent to child; a non-flagged field (a transform setting) does not; an
    /// explicit child value always wins; the first-class `study` grouping key flows when unset.
    #[test]
    fn inherit_identity_is_schema_driven_and_child_wins() {
        use crate::schema::{FieldSpec, ProductSchema};

        let mut parent = Manifest::new("listmode", "raw", "d", TS);
        parent.study = Some("DP01".into());
        parent
            .metadata
            .insert("patient_id".into(), serde_json::json!("ANON9297"));
        parent
            .metadata
            .insert("exam".into(), serde_json::json!("9297"));
        parent
            .metadata
            .insert("energy_window".into(), serde_json::json!("425-650")); // not inheritable

        let mut child = Manifest::new("listmode", "events", "d", TS);
        child
            .metadata
            .insert("patient_id".into(), serde_json::json!("KEEP")); // child override

        let schema = ProductSchema {
            product: "listmode".into(),
            version: "1".into(),
            description: String::new(),
            fields: vec![
                FieldSpec::optional("patient_id", "", "string").inheritable(),
                FieldSpec::optional("exam", "", "string").inheritable(),
                FieldSpec::optional("energy_window", "", "string"), // NOT inheritable
            ],
            blocks: Vec::new(),
            requires_generation: false,
        };

        inherit_identity(&mut child, &parent, &schema);

        assert_eq!(
            child.metadata.get("patient_id"),
            Some(&serde_json::json!("KEEP")),
            "an explicit child value wins over the parent's"
        );
        assert_eq!(
            child.metadata.get("exam"),
            Some(&serde_json::json!("9297")),
            "a schema-flagged field is inherited"
        );
        assert_eq!(
            child.metadata.get("energy_window"),
            None,
            "a non-inheritable field (a transform setting) does not flow down"
        );
        assert_eq!(
            child.study.as_deref(),
            Some("DP01"),
            "the study grouping key is inherited when unset"
        );
    }
}
