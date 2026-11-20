//! Collections — a content-addressed catalog of member products (ADR-0033, #223).
//!
//! Per the design: physical products stay **flat** (one `.tsra` = one raw OR one derived stage);
//! **logical** collections nest *by reference* — a `Collection` lists its members by their product
//! `id` + `manifest_hash`. The collection itself is content-addressed in the same idiom as a
//! [`crate::manifest::Manifest`]:
//!
//! - [`id`](Collection::id) — `blake3` over canonical-JSON of [`id_inputs`] (default keys
//!   `product="collection"`, `name`, `timestamp`). Identity is logical — independent of the
//!   members or their descriptive metadata.
//! - [`content_hash`](Collection::content_hash) — the **MMR root** over the members'
//!   `manifest_hash`es in declared order, reusing [`crate::hash::merkle_root`]. Same construction
//!   the manifest uses for block digests, so the collection is itself verifiable + has cheap
//!   inclusion/consistency proofs once the streaming-write engine wants them. Member order is
//!   significant.
//! - [`manifest_hash`](Collection::manifest_hash) — `blake3` over the stored bytes of
//!   `collection.json`, kept beside them in `collection.seal` (ADR-0066 rule 5). The seal —
//!   transitively commits to every member's `manifest_hash` (which itself commits to that member's
//!   payload), so tampering with any member or with the catalog changes the collection seal.
//!
//! **What the seal covers** is the product manifest's rule (SPEC §4, ADR-0066): the stored bytes,
//! which must be their own JCS form, never a re-serialisation of this struct. So a key a newer
//! writer added and sealed verifies and is named by [`Collection::newer_fields`], a key spliced in
//! under an old seal fails, and a repeated key is refused ([`crate::canonical::check_canonical`]).
//!
//! [`Role`] tags a member as `Raw` (acquisition data, irreplaceable) or `Derived` (regenerable
//! output). Storage layers map this to WORM retention mode — `Role::Raw` → Compliance (immutable),
//! `Role::Derived` → Governance (regenerable). The mapping helper lives next to the WORM module in
//! `tessera-io` so the core stays I/O-free; see `tessera_io::collection::retention_mode`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::field_path::FieldPath;
use crate::manifest::{
    check, recorded, refuse_newer, stored_object, unknown_keys, unknown_keys_each, SUPPORTED_MAJOR,
    TESSERA_VERSION,
};

/// Raw acquisition data vs derived/regenerable product. Drives storage's WORM retention mapping
/// (raw → Compliance, derived → Governance) — see `tessera_io::collection::retention_mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Raw,
    Derived,
}

/// Whether a [`CollectionMember`] resolves to a flat product (`<ref>.tsra`) or a nested
/// sub-collection (`<ref>.collection.json`) — the recursion discriminator (ADR-0049 §1). A member is
/// pinned by `manifest_hash` exactly like a product either way; `kind` only says how to *resolve* and
/// *verify* it. `#[non_exhaustive]` so a future kind (external / virtual reference) stays additive and
/// downstream matchers cannot lock the variant set; an unknown `kind` is *refused* by serde (never
/// swallowed), gated first by [`Collection::check_version`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum MemberKind {
    /// A flat physical product — one `.tsra` (the default; skipped on serialize so pre-recursion
    /// collections stay byte-identical).
    #[default]
    Product,
    /// A nested sub-collection — a child `collection.json`, verified recursively.
    Collection,
}

impl MemberKind {
    /// The default-kind predicate for `#[serde(skip_serializing_if)]` — keeps existing all-product
    /// collections byte-identical (the `kind` key is omitted entirely). A free-standing associated
    /// fn (not a trait method) so serde's macro is happy. ADR-0049 §1.
    pub fn is_product(&self) -> bool {
        matches!(self, MemberKind::Product)
    }

    /// A fixed **1-byte** content-hash leaf tag — unambiguous for any current *or future* kind (a
    /// byte tag can't collide or prefix-alias the way a string separator could). The exhaustive match
    /// forces a new variant to declare its own tag at compile time. ADR-0049 §2.
    fn leaf_tag(self) -> u8 {
        match self {
            MemberKind::Product => 0x00,
            MemberKind::Collection => 0x01,
        }
    }
}

/// Filesystem-safe stem for a member's content-addressed `reference` (a `blake3:<hex>` id): the
/// single `:` — and any `/` or `\` — becomes `_`. Lossless because the prefix is the fixed
/// `blake3:`, so `blake3_<hex>` round-trips unambiguously.
///
/// This is the **single source of truth** for the on-disk member stem, so every writer (the ingest
/// engine, `collection new`) and every reader (`collection verify` / `ls`) derive the same name.
/// Before this existed the ingest engine sanitized (`blake3_…`) while the consumer verbs resolved the
/// raw reference (`blake3:…`), so `verify`/`ls` could never find an ingested member (#323).
pub fn sanitize_reference(reference: &str) -> String {
    reference.replace([':', '/', '\\'], "_")
}

/// The on-disk filename of a member beside its `collection.json`, resolved by `kind` (ADR-0049 §4): a
/// product is `<stem>.tsra`, a sub-collection is `<stem>.collection.json`, where `<stem>` is the
/// [`sanitize_reference`] of the member's `reference`. The one place the member layout is spelled.
pub fn member_filename(reference: &str, kind: MemberKind) -> String {
    let stem = sanitize_reference(reference);
    match kind {
        MemberKind::Collection => format!("{stem}.collection.json"),
        _ => format!("{stem}.tsra"),
    }
}

/// One member of a [`Collection`]: a flat physical product referenced by its content-addressed `id`
/// and pinned to its `manifest_hash` (so the collection seal transitively commits to the member).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CollectionMember {
    /// The member product's logical [`crate::manifest::Manifest::id`].
    pub reference: String,
    /// The member product's seal — pinned here so changing the member changes the collection.
    pub manifest_hash: String,
    pub role: Role,
    /// Product or nested sub-collection (ADR-0049 §1). Skipped on serialize when `Product` (the
    /// default) → pre-recursion collections serialize byte-identical.
    #[serde(default, skip_serializing_if = "MemberKind::is_product")]
    pub kind: MemberKind,
    /// Other members in this collection this one is derived from (their `reference`s). Forms an
    /// in-collection DAG that mirrors the manifest's `sources` edges, used by the RO-Crate
    /// projection to render `wasDerivedFrom`. Peers-only — a cross-collection dependency is
    /// materialized as a member of the enclosing collection, never a dangling reference (ADR-0049 §4).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub derived_from: Vec<String>,
}

impl CollectionMember {
    /// The JSON keys this type models (ADR-0060 §2.2 newer-field walk); kept equal to a fully
    /// populated instance's keys by a test.
    pub(crate) const FIELDS: &'static [&'static str] =
        &["reference", "manifest_hash", "role", "kind", "derived_from"];

    pub fn new(reference: impl Into<String>, manifest_hash: impl Into<String>, role: Role) -> Self {
        CollectionMember {
            reference: reference.into(),
            manifest_hash: manifest_hash.into(),
            role,
            kind: MemberKind::Product,
            derived_from: Vec::new(),
        }
    }

    /// Builder: mark this member as a nested sub-collection (vs the default flat product).
    pub fn with_kind(mut self, kind: MemberKind) -> Self {
        self.kind = kind;
        self
    }

    /// Builder: record the in-collection edges this member is derived from.
    pub fn with_derived_from(mut self, refs: Vec<String>) -> Self {
        self.derived_from = refs;
        self
    }
}

/// A content-addressed catalog over a set of member products (ADR-0033). Mirrors the manifest's
/// identity discipline: `id` (logical) + `content_hash` (MMR root over members in order) +
/// `manifest_hash` (the seal). Three projections of one logical collection live in `tessera-io`:
/// RO-Crate (FAIR discovery), OCI image index (registry-native), and S3-prefix layout.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Collection {
    /// Format/spec version.
    pub tessera_version: String,
    /// Stable, content-derived logical identity.
    pub id: String,
    /// Declared identity inputs (key→value) that `id` is hashed over. Default keys:
    /// `product="collection"`, `name`, `timestamp` — recorded so identity is transparent.
    pub id_inputs: BTreeMap<String, String>,
    pub name: String,
    pub description: String,
    /// RFC 3339 timestamp, normalized to UTC.
    pub timestamp: String,
    /// The declared **collection level schema** — the product/`schema` analogue for collections
    /// (#294). Resolves a registered, versioned [`CollectionSchema`] whose `member_rule` the engine
    /// validates at seal + verify. Defaults to the generic, permissive `"collection"`; `"dataset"`
    /// (products only) / `"project"` (sub-collections, recursive) are the built-in stricter levels,
    /// and a **domain** may register its own (`exam`/`cohort`/… ) — taxonomy is data, not an engine
    /// enum. Sealed (part of `manifest_hash`). An unknown schema is permitted (open-world).
    #[serde(default = "default_collection_schema")]
    pub collection_schema: String,
    /// Optional study/grouping id (fd5 `study`) — ties this collection to other products of the
    /// same exam. Same field as on [`crate::manifest::Manifest`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub study: Option<String>,
    /// Optional, **open, domain-supplied** level tag (`_vocabulary`/`_code`). The engine carries it
    /// but never interprets it — level *taxonomies* (exam / cohort / site / …) are domain
    /// profiles/data, NOT an engine enum (ADR-0049 §5, RFC §12). `None` by default; part of the seal
    /// when present. This is what makes a nesting level self-describing without Tessera opining on it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<crate::schema::Coded>,
    /// Member products, in declared order — order is significant (folded into `content_hash`).
    #[serde(default)]
    pub members: Vec<CollectionMember>,
    /// MMR root over the members' `manifest_hash`es; `Some` once sealed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    /// The seal: blake3 over this collection's stored bytes ([`to_stored`](Self::to_stored)).
    /// **Never serialised** (ADR-0066 rules 1 and 5): it is kept in `collection.seal`, and a reader
    /// sets it from there ([`from_stored`](Self::from_stored)).
    #[serde(skip)]
    pub manifest_hash: Option<String>,
    /// Keys of the parsed collection this build does not model (see [`Self::newer_fields`]).
    /// Never serialised: it describes the received object, not the collection.
    #[serde(skip)]
    newer_fields: Vec<String>,
}

/// The paths of every key in a collection object that this build's types don't declare: the top
/// level, `level` and each `members[i]`. `id_inputs` is an opaque map and is not walked.
fn newer_fields_of(object: &Value) -> Vec<FieldPath> {
    let mut out = Vec::new();
    let root = FieldPath::root();
    unknown_keys(object, Collection::FIELDS, &root, &mut out);
    let get = |k: &str| object.get(k).unwrap_or(&Value::Null);
    unknown_keys(
        get("level"),
        crate::schema::Coded::FIELDS,
        &root.key("level"),
        &mut out,
    );
    unknown_keys_each(
        get("members"),
        CollectionMember::FIELDS,
        &root.key("members"),
        &mut out,
    );
    out
}

/// The generic, permissive default collection schema (accepts product *and* sub-collection members).
/// `dataset`/`project` (and domain-registered levels) are the opt-in stricter schemas.
pub fn default_collection_schema() -> String {
    "collection".to_string()
}

/// Build the default identity-input map for a collection — mirrors
/// [`crate::identity::default_id_inputs`] but with `product = "collection"`.
pub fn default_collection_id_inputs(name: &str, timestamp: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("product".to_string(), "collection".to_string()),
        ("name".to_string(), name.to_string()),
        ("timestamp".to_string(), timestamp.to_string()),
    ])
}

impl Collection {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        timestamp: impl Into<String>,
    ) -> Self {
        let name = name.into();
        let description = description.into();
        // Normalise to UTC so equivalent instants in different offsets share one id (mirrors
        // [`crate::manifest::Manifest::new`]).
        let timestamp = crate::identity::normalize_timestamp(&timestamp.into());
        let id_inputs = default_collection_id_inputs(&name, &timestamp);
        // Infallible: string id_inputs always canonicalize.
        let id =
            crate::identity::compute_id(&id_inputs).expect("string id_inputs always canonicalize");
        Collection {
            tessera_version: TESSERA_VERSION.to_string(),
            id,
            id_inputs,
            name,
            description,
            timestamp,
            collection_schema: default_collection_schema(),
            study: None,
            level: None,
            members: Vec::new(),
            content_hash: None,
            manifest_hash: None,
            newer_fields: Vec::new(),
        }
    }

    /// The JSON keys this type models; kept equal to a fully populated instance's keys by a test.
    pub(crate) const FIELDS: &'static [&'static str] = &[
        "tessera_version",
        "id",
        "id_inputs",
        "name",
        "description",
        "timestamp",
        "collection_schema",
        "study",
        "level",
        "members",
        "content_hash",
    ];

    /// Fields of the parsed collection that this build does not model: keys a **newer format
    /// revision** added, as paths (`annotations`, `level.uri`, `members[1].weight`), rendered as
    /// [`crate::Manifest::newer_fields`] renders them: a top-level key named `members[1].weight` is
    /// `["members[1].weight"]`, never the member's field. Empty for
    /// a collection this build wrote and for one not parsed by [`from_json`](Self::from_json) /
    /// [`from_stored`](Self::from_stored).
    ///
    /// They are covered by the seal (it is over the stored bytes) but not interpreted, and this
    /// struct doesn't carry them, so every operation that would build a new seal from this view or
    /// store it ([`to_stored`](Self::to_stored)) refuses with
    /// [`crate::Error::NewerFormatFields`] instead of dropping them (ADR-0060 §2.2). Only the display
    /// form [`to_json`](Self::to_json) leaves them out.
    pub fn newer_fields(&self) -> &[String] {
        &self.newer_fields
    }

    /// True once the collection has been sealed (carries a manifest hash).
    pub fn is_sealed(&self) -> bool {
        self.manifest_hash.is_some()
    }

    /// Pretty JSON, for humans / display. NOT the stored form — see [`to_stored`](Self::to_stored).
    pub fn to_json(&self) -> crate::Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Canonical (RFC 8785 JCS) bytes of this struct: the bytes [`to_stored`](Self::to_stored)
    /// stores and [`compute_manifest_hash`](Self::compute_manifest_hash) hashes.
    ///
    /// # Errors
    /// [`crate::Error::NewerFormatFields`] for a parsed collection with
    /// [`newer_fields`](Self::newer_fields): bytes of this view would leave them out.
    pub fn canonical_bytes(&self) -> crate::Result<Vec<u8>> {
        refuse_newer(&self.newer_fields)?;
        crate::canonical::to_bytes(&crate::canonical::to_json_value(self)?)
    }

    /// Recompute the logical `id` from `id_inputs`.
    pub fn recompute_id(&self) -> crate::Result<String> {
        crate::identity::compute_id(&self.id_inputs)
    }

    /// Recompute the MMR root over the members' `manifest_hash`es in declared order. Reuses the
    /// same [`crate::hash::merkle_root`] the manifest uses for block digests, so a collection is
    /// content-addressed in the exact same idiom as a product (and gets the same inclusion +
    /// consistency proofs as a free corollary).
    pub fn recompute_content_hash(&self) -> String {
        // Domain-separate the leaf by `kind` (ADR-0049 §2): each leaf is `BLAKE3(leaf-domain ‖
        // kind_byte ‖ manifest_hash)`, so an MMR inclusion proof is **kind-bound** (product-`H` and
        // collection-`H` fold to different leaves) and `kind` cannot be flipped in the JSON without
        // moving `content_hash`. A fixed 1-byte `kind_byte` is unambiguous for any future kind — no
        // string-separator prefix-collision. `merkle_root`'s own leaf/node domains (ADR-0028) are
        // unchanged. Pre-1.0 with no pinned collection goldens, adopted outright (no compat shim).
        let leaves: Vec<String> = self
            .members
            .iter()
            .map(|m| {
                let mut pre = Vec::with_capacity(m.manifest_hash.len() + 32);
                pre.extend_from_slice(b"tessera-collection-member-v1");
                pre.push(m.kind.leaf_tag());
                pre.extend_from_slice(m.manifest_hash.as_bytes());
                crate::hash::digest(&pre)
            })
            .collect();
        crate::hash::merkle_root(&leaves)
    }

    /// The seal (`manifest_hash`) this struct gets when written out: `blake3` over
    /// [`canonical_bytes`](Self::canonical_bytes).
    ///
    /// # Errors
    /// As [`canonical_bytes`](Self::canonical_bytes): a re-seal of a view with
    /// [`newer_fields`](Self::newer_fields) is refused, since it would drop sealed content.
    pub fn compute_manifest_hash(&self) -> crate::Result<String> {
        Ok(crate::hash::digest(&self.canonical_bytes()?))
    }

    /// The typed view of a strictly parsed collection object: declared form, version, newer fields.
    fn from_object(object: &Value) -> crate::Result<Self> {
        let newer = newer_fields_of(object);
        let mut c: Collection = crate::canonical::from_declared_form(object, &newer)?;
        c.check_version()?;
        c.newer_fields = crate::field_path::render(&newer);
        Ok(c)
    }

    /// Parse a collection JSON, refusing one whose major spec version this reader can't handle.
    /// **Does not verify anything** (a display of a possibly tampered catalog); a reader verifies
    /// with [`from_stored_members`](Self::from_stored_members).
    ///
    /// # Errors
    /// [`crate::Error::Serde`] for malformed JSON, a **duplicate key** in any object
    /// ([`crate::canonical::parse_strict`]), a known field of the wrong type (an unknown member
    /// `kind` included), the collection, its `level` or a `members[i]` in array form instead of an
    /// object, or a member's `role`/`kind` spelled as an object instead of a string, the default
    /// `kind` (`product`, which a writer omits) included (SPEC §4, one spelling per value; the
    /// message names its path);
    /// [`crate::Error::InexactInteger`] for a number that is an integer beyond ±(2^53−1) anywhere in
    /// it (SPEC §1, I-JSON: no seal pins it; the message names its path);
    /// [`crate::Error::UnsupportedVersion`] for a newer major version.
    pub fn from_json(s: &str) -> crate::Result<Self> {
        Self::from_object(&crate::canonical::parse_strict(s)?)
    }

    /// The checks a reader runs on a received collection's typed view besides its seal: `id`, then
    /// `content_hash`, which must be present.
    fn verify_received_id_and_content(&self) -> crate::Result<()> {
        check("id", &self.id, &self.recompute_id()?)?;
        check(
            "content_hash",
            &recorded(&self.content_hash),
            &self.recompute_content_hash(),
        )
    }

    /// Read a collection from its **stored bytes** (`collection.json`) and the seal recorded beside
    /// them (`collection.seal`), under ADR-0066 rules 4 and 5. Mirrors
    /// [`crate::Manifest::from_stored`]; the reader's half of [`to_stored`](Self::to_stored).
    ///
    /// In order: [`crate::canonical::check_canonical`] (strict parse, then the bytes must equal
    /// their own JCS form); no top-level `manifest_hash` key (a member's `manifest_hash` pin is
    /// data and stays); `seal` equals `blake3` over the bytes, compared byte for byte; then the
    /// typed layer: declared form, version, `id`, `content_hash` (which must be present) and the
    /// level schema's `member_rule`.
    /// The returned view carries `seal` as its [`manifest_hash`](Self::manifest_hash).
    ///
    /// # Errors
    /// In that order: [`crate::Error::Serde`], [`crate::Error::InexactInteger`] or
    /// [`crate::Error::NonCanonical`] (offset of the first differing byte) from the canonical check;
    /// [`crate::Error::InBandSeal`]; [`crate::Error::Integrity`] with `what` = `manifest_hash`,
    /// `expected` = `seal` and `actual` = the bytes' digest; then the declared-form and
    /// [`crate::Error::UnsupportedVersion`] errors of [`from_json`](Self::from_json),
    /// [`crate::Error::Integrity`] for `id` or `content_hash`, and [`crate::Error::Invalid`] for a
    /// member kind the level schema doesn't permit.
    pub fn from_stored(bytes: &[u8], seal: &str) -> crate::Result<Self> {
        let object = stored_object(bytes, seal, "collection")?;
        let mut c = Self::from_object(&object)?;
        c.verify_received_id_and_content()?;
        c.validate_schema()?;
        c.manifest_hash = Some(seal.to_owned());
        Ok(c)
    }

    /// Read a collection from its two stored files, `collection.json` and `collection.seal`
    /// (`None` when the seal file is absent): the one door every collection reader goes through.
    /// Mirrors [`crate::Manifest::from_stored_members`]: a missing seal is the alpha layout, refused
    /// naming what was found and what is required (ADR-0066 rule 10); a present one must be
    /// well formed, then [`from_stored`](Self::from_stored) runs.
    ///
    /// # Errors
    /// [`crate::Error::UnsupportedLayout`], [`crate::Error::MalformedSeal`], then everything
    /// [`from_stored`](Self::from_stored) refuses.
    pub fn from_stored_members(json: &[u8], seal: Option<&[u8]>) -> crate::Result<Self> {
        Self::from_stored_members_as(crate::stored::COLLECTION_FILE, json, seal)
    }

    /// [`from_stored_members`](Self::from_stored_members) for a collection stored under the file
    /// name `document`: `collection.json`, or `<stem>.collection.json` for a sub-collection member
    /// ([`member_filename`]). The name only labels the refusals: a missing seal names
    /// `<stem>.collection.seal`, the seal file beside that document
    /// ([`crate::stored::seal_name_for`]).
    ///
    /// # Errors
    /// [`crate::Error::Invalid`] when `document` does not end in `.json`; then as
    /// [`from_stored_members`](Self::from_stored_members).
    pub fn from_stored_members_as(
        document: &str,
        json: &[u8],
        seal: Option<&[u8]>,
    ) -> crate::Result<Self> {
        let seal = crate::stored::require_seal(document, json, seal)?;
        Self::from_stored(json, seal)
    }

    /// This collection's **stored form** under ADR-0066: `(bytes, seal)`, where `bytes` is its RFC
    /// 8785 (JCS) serialisation with no top-level `manifest_hash` key and `seal` is
    /// `"blake3:" + hex(blake3(bytes))`. Mirrors [`crate::Manifest::to_stored`]: `bytes` are
    /// [`canonical_bytes`](Self::canonical_bytes), the bytes the alpha layout's in-band seal was
    /// computed over, so a collection sealed before ADR-0066 keeps its seal's value, and
    /// [`from_stored`](Self::from_stored) reads them back.
    ///
    /// The collection must carry a `content_hash` and pass [`verify`](Self::verify) (`id`,
    /// `content_hash`, the level schema, and a `manifest_hash` already set must equal `seal`).
    ///
    /// # Errors
    /// [`crate::Error::NewerFormatFields`] for a parsed collection with
    /// [`newer_fields`](Self::newer_fields); [`crate::Error::Invalid`] without a `content_hash`;
    /// everything [`verify`](Self::verify) raises, including [`crate::Error::Integrity`] for a
    /// `manifest_hash` that is not `seal`; [`crate::Error::InexactInteger`] or
    /// [`crate::Error::NonFiniteNumber`] for a value no seal can hold.
    pub fn to_stored(&self) -> crate::Result<(Vec<u8>, String)> {
        refuse_newer(&self.newer_fields)?;
        if self.content_hash.is_none() {
            return Err(crate::Error::Invalid(
                "refusing to store a collection without content_hash: a stored collection is \
                 sealed"
                    .into(),
            ));
        }
        self.verify()?;
        let bytes = self.canonical_bytes()?;
        let seal = crate::hash::digest(&bytes);
        Ok((bytes, seal))
    }

    /// Verify this **struct's** three hashes against their recomputed values (and the spec version):
    /// the writer's check on a collection it built. Any mismatch is a typed
    /// [`crate::Error::Integrity`] naming the field. An unsealed collection verifies its `id` only; a
    /// sealed one verifies all three. Mirrors [`crate::manifest::Manifest::verify`].
    ///
    /// The seal is recomputed over this struct's JSON. For a collection read from bytes, verify the
    /// bytes with [`from_stored`](Self::from_stored) instead: a view with
    /// [`newer_fields`](Self::newer_fields) fails here with [`crate::Error::NewerFormatFields`], and
    /// one parsed from an encoding the struct normalises fails with `Integrity`.
    pub fn verify(&self) -> crate::Result<()> {
        self.check_version()?;
        check("id", &self.id, &self.recompute_id()?)?;
        if let Some(ch) = &self.content_hash {
            check("content_hash", ch, &self.recompute_content_hash())?;
        }
        if let Some(mh) = &self.manifest_hash {
            check("manifest_hash", mh, &self.compute_manifest_hash()?)?;
        }
        // Structural validation: the members satisfy the declared level schema's `member_rule` (#294).
        self.validate_schema()?;
        Ok(())
    }

    /// Validate this collection's members against its declared [`collection_schema`](Self::collection_schema)'s
    /// `member_rule`, using the built-in [`CollectionSchemaRegistry`]. An unknown level id is permitted
    /// (open-world — a domain-registered level this binary doesn't ship is not an error). #294.
    pub fn validate_schema(&self) -> crate::Result<()> {
        CollectionSchemaRegistry::builtin().validate(self)
    }

    /// Error if `tessera_version`'s major exceeds [`SUPPORTED_MAJOR`] (forward-incompat), or if it
    /// is unparseable. Never panics. Same policy as the manifest's version check.
    pub fn check_version(&self) -> crate::Result<()> {
        let major = self
            .tessera_version
            .split('.')
            .next()
            .and_then(|x| x.parse::<u64>().ok())
            .ok_or_else(|| {
                crate::Error::UnsupportedVersion(format!(
                    "unparseable tessera_version: {}",
                    self.tessera_version
                ))
            })?;
        if major > SUPPORTED_MAJOR {
            return Err(crate::Error::UnsupportedVersion(format!(
                "{} (reader supports major <= {SUPPORTED_MAJOR})",
                self.tessera_version
            )));
        }
        Ok(())
    }
}

/// The structural rule a [`CollectionSchema`] imposes on a collection's members (#294): which member
/// **kinds** the level permits (`dataset` ⇒ products only; `project` ⇒ sub-collections only).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemberRule {
    /// Member kinds this level permits; a member whose `kind` is not listed fails validation.
    pub allowed_member_kinds: Vec<MemberKind>,
    /// Whether sub-collection members are expected to nest further — records the level's intent
    /// (recursion is a *mechanism* property the engine always supports; this is the schema's stance).
    #[serde(default)]
    pub recursive: bool,
}

/// A collection **level schema** — the collection analogue of [`crate::schema::ProductSchema`],
/// registered + versioned so "what a `dataset` / `project` / `cohort` IS" is domain *data*, not an
/// engine enum (#294). The engine validates the `member_rule` at seal + verify; the taxonomy (the set
/// of level ids and their rules) is registered, additive, and extensible per domain — core stays
/// generic (two structural types + recursion), all opinion lives in registered schemas.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CollectionSchema {
    /// The level id — the value a [`Collection::collection_schema`] carries (e.g. `"dataset"`).
    pub collection_schema: String,
    /// Schema version (additive evolution; the id never changes meaning — stable-id discipline).
    pub version: String,
    pub description: String,
    pub member_rule: MemberRule,
}

impl CollectionSchema {
    /// Validate a collection's members against this level's `member_rule` — a typed
    /// [`crate::Error::Invalid`] naming the first member whose `kind` the level does not permit.
    pub fn validate(&self, c: &Collection) -> crate::Result<()> {
        for (i, m) in c.members.iter().enumerate() {
            if !self.member_rule.allowed_member_kinds.contains(&m.kind) {
                return Err(crate::Error::Invalid(format!(
                    "collection schema '{}' does not permit a {:?} member (member {i} '{}')",
                    self.collection_schema, m.kind, m.reference
                )));
            }
        }
        Ok(())
    }
}

/// Registry of built-in collection level schemas (mirrors [`crate::schema::SchemaRegistry`]). Core
/// ships the permissive generic `collection` + the two structural levels `dataset` / `project`; a
/// **domain** registers its own opinionated levels (`exam`/`cohort`/…). An unknown level id →
/// permissive (open-world: a level this binary doesn't ship is allowed, not an error).
#[derive(Debug, Clone)]
pub struct CollectionSchemaRegistry {
    schemas: BTreeMap<String, CollectionSchema>,
}

impl CollectionSchemaRegistry {
    /// The built-in levels: `collection` (generic, any kind), `dataset` (products only), `project`
    /// (sub-collections only, recursive).
    pub fn builtin() -> Self {
        let schemas = [
            CollectionSchema {
                collection_schema: "collection".into(),
                version: "1.0".into(),
                description: "A generic catalog — any member kind (the permissive default).".into(),
                member_rule: MemberRule {
                    allowed_member_kinds: vec![MemberKind::Product, MemberKind::Collection],
                    recursive: true,
                },
            },
            CollectionSchema {
                collection_schema: "dataset".into(),
                version: "1.0".into(),
                description: "A collection of products (acq + derived) from one physical setup."
                    .into(),
                member_rule: MemberRule {
                    allowed_member_kinds: vec![MemberKind::Product],
                    recursive: false,
                },
            },
            CollectionSchema {
                collection_schema: "project".into(),
                version: "1.0".into(),
                description: "A recursive collection of datasets or projects.".into(),
                member_rule: MemberRule {
                    allowed_member_kinds: vec![MemberKind::Collection],
                    recursive: true,
                },
            },
        ];
        CollectionSchemaRegistry {
            schemas: schemas
                .into_iter()
                .map(|s| (s.collection_schema.clone(), s))
                .collect(),
        }
    }

    pub fn get(&self, id: &str) -> Option<&CollectionSchema> {
        self.schemas.get(id)
    }

    pub fn levels(&self) -> impl Iterator<Item = &str> {
        self.schemas.keys().map(String::as_str)
    }

    /// Validate a collection against its declared level schema. Unknown level → `Ok` (open-world).
    pub fn validate(&self, c: &Collection) -> crate::Result<()> {
        match self.get(&c.collection_schema) {
            Some(s) => s.validate(c),
            None => Ok(()),
        }
    }
}

impl Default for CollectionSchemaRegistry {
    fn default() -> Self {
        Self::builtin()
    }
}

/// A resolved reference to a sealed **product** — its logical `id` + `manifest_hash` carried as one
/// unit, so a caller can't pair the wrong reference with the wrong hash (ADR-0049 §4).
#[derive(Debug, Clone)]
pub struct ProductHandle {
    // Private so `of()` is the ONLY construction path — a caller cannot pair a wrong reference with
    // a wrong hash (ADR-0049 §4). Read via `reference()` / `manifest_hash()`.
    pub(crate) reference: String,
    pub(crate) manifest_hash: String,
}

impl ProductHandle {
    /// The pinned product reference (its logical `id`).
    pub fn reference(&self) -> &str {
        &self.reference
    }
    /// The pinned product `manifest_hash`.
    pub fn manifest_hash(&self) -> &str {
        &self.manifest_hash
    }

    /// Build from a sealed product manifest (errors if it isn't sealed yet).
    pub fn of(m: &crate::manifest::Manifest) -> crate::Result<Self> {
        Ok(ProductHandle {
            reference: m.id.clone(),
            manifest_hash: m
                .manifest_hash
                .clone()
                .ok_or_else(|| crate::Error::Invalid("product is not sealed".into()))?,
        })
    }
}

/// A resolved reference to a sealed **sub-collection** — the recursion analogue of [`ProductHandle`].
#[derive(Debug, Clone)]
pub struct CollectionHandle {
    pub(crate) reference: String,
    pub(crate) manifest_hash: String,
}

impl CollectionHandle {
    /// The pinned child-collection reference (its logical `id`).
    pub fn reference(&self) -> &str {
        &self.reference
    }
    /// The pinned child-collection `manifest_hash`.
    pub fn manifest_hash(&self) -> &str {
        &self.manifest_hash
    }

    /// Build from a sealed child collection (errors if it isn't sealed yet).
    pub fn of(c: &Collection) -> crate::Result<Self> {
        Ok(CollectionHandle {
            reference: c.id.clone(),
            manifest_hash: c
                .manifest_hash
                .clone()
                .ok_or_else(|| crate::Error::Invalid("sub-collection is not sealed".into()))?,
        })
    }
}

/// Build a [`Collection`] by adding members in order then sealing — mirrors
/// [`crate::ProductBuilder`]. Member order is preserved and is significant for `content_hash`, so
/// the same members in the same order seal to byte-identical bytes (writer-determinism).
pub struct CollectionBuilder {
    collection: Collection,
}

impl CollectionBuilder {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        timestamp: impl Into<String>,
    ) -> Self {
        CollectionBuilder {
            collection: Collection::new(name, description, timestamp),
        }
    }

    /// Declare the collection **level schema** (#294) — e.g. `"dataset"` (products only) or
    /// `"project"` (sub-collections, recursive), or a domain-registered level. `seal` validates the
    /// members against its `member_rule`. Defaults to the permissive `"collection"`.
    pub fn with_collection_schema(&mut self, collection_schema: impl Into<String>) -> &mut Self {
        self.collection.collection_schema = collection_schema.into();
        self
    }

    /// Set the study/grouping id (ties this collection to the products of the same exam).
    pub fn with_study(&mut self, study: impl Into<String>) -> &mut Self {
        self.collection.study = Some(study.into());
        self
    }

    /// Set the open, domain-supplied `level` tag (the engine carries it, never interprets it —
    /// ADR-0049 §5). Level taxonomies are domain data, not an engine enum.
    pub fn with_level(&mut self, level: crate::schema::Coded) -> &mut Self {
        self.collection.level = Some(level);
        self
    }

    /// Append a **product** member from its resolved handle (ADR-0049 §4) — the type-safe front for
    /// [`add_member`](Self::add_member) that can't mismatch reference/hash.
    pub fn add_product(
        &mut self,
        product: &ProductHandle,
        role: Role,
        derived_from: Vec<String>,
    ) -> &mut Self {
        self.collection.members.push(
            CollectionMember::new(&product.reference, &product.manifest_hash, role)
                .with_derived_from(derived_from),
        );
        self
    }

    /// Append a **sub-collection** member — the recursion entry point (ADR-0049 §1). Pinned by the
    /// child's `manifest_hash`; `verify` resolves it as `<ref>.collection.json` and recurses.
    pub fn add_subcollection(
        &mut self,
        sub: &CollectionHandle,
        role: Role,
        derived_from: Vec<String>,
    ) -> &mut Self {
        self.collection.members.push(
            CollectionMember::new(&sub.reference, &sub.manifest_hash, role)
                .with_kind(MemberKind::Collection)
                .with_derived_from(derived_from),
        );
        self
    }

    /// Low-level append (pins `kind = Product`). Order is preserved — the seal folds members'
    /// `manifest_hash`es in this exact order, so two builders that add the same members in different
    /// orders seal to different `content_hash`es (order is part of the catalog's identity).
    ///
    /// **Prefer the typed [`add_product`](Self::add_product) / [`add_subcollection`](Self::add_subcollection)** —
    /// this raw form cannot express a sub-collection (using it to add a nested collection would silently
    /// seal a *product*-kind leaf and the wrong `content_hash`).
    pub fn add_member(
        &mut self,
        reference: impl Into<String>,
        manifest_hash: impl Into<String>,
        role: Role,
        derived_from: Vec<String>,
    ) -> &mut Self {
        self.collection.members.push(
            CollectionMember::new(reference, manifest_hash, role).with_derived_from(derived_from),
        );
        self
    }

    /// Seal: compute the MMR root over the members, then the canonical-JSON seal. Mirrors
    /// [`crate::ProductBuilder::seal`] — the same input always yields byte-identical bytes.
    pub fn seal(mut self) -> crate::Result<Collection> {
        // Structural well-formedness of the open `level` tag (the engine still never *interprets* it,
        // ADR-0049 §5): an empty vocabulary or code is malformed, same class as an empty `name`.
        if let Some(level) = &self.collection.level {
            if level.vocabulary.is_empty() || level.code.is_empty() {
                return Err(crate::Error::Invalid(
                    "collection level tag must have a non-empty _vocabulary and _code".into(),
                ));
            }
        }
        // Fail-fast: the members satisfy the declared level schema's `member_rule` *before* we seal
        // (#294) — a `dataset` can't seal with a sub-collection member, a `project` can't with a
        // product member. Unknown level → permitted (open-world).
        self.collection.validate_schema()?;
        self.collection.content_hash = Some(self.collection.recompute_content_hash());
        // Computed last, over the canonical bytes with `manifest_hash` excluded, so the seal
        // transitively commits to id_inputs, study, members + their pinned manifest_hashes, and
        // the content_hash itself.
        self.collection.manifest_hash = None;
        self.collection.manifest_hash = Some(self.collection.compute_manifest_hash()?);
        Ok(self.collection)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::Manifest;
    use crate::provenance::Source;
    use crate::ProductBuilder;

    const TS: &str = "2024-01-01T00:00:00Z";

    fn raw_listmode() -> Manifest {
        ProductBuilder::new("listmode", "DP06-raw", "raw events", TS)
            .seal()
            .unwrap()
    }

    fn derived_recon(from: &Manifest) -> Manifest {
        let mut b = ProductBuilder::new("recon", "DP06-recon", "reconstructed", TS);
        b.add_source(
            Source::new("derived_from", &from.id)
                .with_content_hash(from.manifest_hash.clone().unwrap()),
        );
        b.seal().unwrap()
    }

    fn build_collection(raw: &Manifest, recon: &Manifest) -> Collection {
        let mut cb = CollectionBuilder::new("DP06-study", "DUPLET DP06 study", TS);
        cb.with_study("DP06-2024-01");
        cb.add_member(
            &raw.id,
            raw.manifest_hash.clone().unwrap(),
            Role::Raw,
            Vec::new(),
        );
        cb.add_member(
            &recon.id,
            recon.manifest_hash.clone().unwrap(),
            Role::Derived,
            vec![raw.id.clone()],
        );
        cb.seal().unwrap()
    }

    #[test]
    fn collection_is_deterministic_and_content_addressed() {
        let raw = raw_listmode();
        let recon = derived_recon(&raw);
        let a = build_collection(&raw, &recon);
        let b = build_collection(&raw, &recon);
        // identical inputs (same members, same order) → byte-identical sealed bytes.
        assert!(a.is_sealed());
        assert_eq!(a.id, b.id);
        assert_eq!(a.content_hash, b.content_hash);
        assert_eq!(a.manifest_hash, b.manifest_hash);
        assert_eq!(a.canonical_bytes().unwrap(), b.canonical_bytes().unwrap());
        a.verify().unwrap();

        // a different member order changes content_hash + manifest_hash (id stays — it's logical).
        let mut cb = CollectionBuilder::new("DP06-study", "DUPLET DP06 study", TS);
        cb.with_study("DP06-2024-01");
        cb.add_member(
            &recon.id,
            recon.manifest_hash.clone().unwrap(),
            Role::Derived,
            vec![raw.id.clone()],
        );
        cb.add_member(
            &raw.id,
            raw.manifest_hash.clone().unwrap(),
            Role::Raw,
            Vec::new(),
        );
        let reordered = cb.seal().unwrap();
        assert_eq!(
            reordered.id, a.id,
            "id is logical — order doesn't change it"
        );
        assert_ne!(reordered.content_hash, a.content_hash);
        assert_ne!(reordered.manifest_hash, a.manifest_hash);

        // tampering with a member's pinned manifest_hash changes content_hash + manifest_hash.
        let mut tampered = a.clone();
        tampered.members[0].manifest_hash = "blake3:deadbeef".into();
        assert_ne!(
            tampered.recompute_content_hash(),
            a.recompute_content_hash()
        );
        match tampered.verify() {
            Err(crate::Error::Integrity { what, .. }) => assert_eq!(what, "content_hash"),
            other => panic!("expected content_hash integrity error, got {other:?}"),
        }
    }

    #[test]
    fn collection_id_independent_of_descriptive_metadata() {
        // Mirrors the manifest's identity discipline: changing description (non-identity-bearing)
        // does NOT change id; changing name (identity-bearing) does.
        let a = CollectionBuilder::new("DP06", "first description", TS)
            .seal()
            .unwrap();
        let b = CollectionBuilder::new("DP06", "different description", TS)
            .seal()
            .unwrap();
        let c = CollectionBuilder::new("DP07", "first description", TS)
            .seal()
            .unwrap();
        assert_eq!(a.id, b.id, "description is not identity-bearing");
        assert_ne!(a.id, c.id, "name IS identity-bearing");
        // a sealed empty collection still verifies (empty MMR root is well-defined).
        a.verify().unwrap();
    }

    #[test]
    fn collection_round_trips_through_canonical_json() {
        let raw = raw_listmode();
        let recon = derived_recon(&raw);
        let c = build_collection(&raw, &recon);
        let (bytes, seal) = c.to_stored().unwrap();
        let parsed = Collection::from_stored(&bytes, &seal).unwrap();
        assert_eq!(parsed.manifest_hash, c.manifest_hash);
        assert_eq!(parsed.content_hash, c.content_hash);
        assert_eq!(parsed.members.len(), 2);
        assert_eq!(parsed.members[1].derived_from, vec![raw.id]);
    }

    /// The declared-field lists stay equal to the keys a fully populated instance writes, so a new
    /// field can't be added without the newer-field walk learning it (ADR-0060 §2.2, #873).
    #[test]
    fn declared_fields_match_a_fully_populated_collection() {
        let keys = |v: &Value| {
            let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
            k.sort();
            k
        };
        let sorted = |f: &[&str]| {
            let mut k: Vec<String> = f.iter().map(|s| s.to_string()).collect();
            k.sort();
            k
        };
        // Struct literals name every field (no `..`), so a new field fails to compile here until
        // the walk learns it.
        let c = Collection {
            tessera_version: TESSERA_VERSION.into(),
            id: "i".into(),
            id_inputs: BTreeMap::from([("name".to_string(), "c".to_string())]),
            name: "c".into(),
            description: "d".into(),
            timestamp: TS.into(),
            collection_schema: "dataset".into(),
            study: Some("s".into()),
            level: Some(crate::schema::Coded {
                vocabulary: "v".into(),
                code: "cohort".into(),
                label: Some("Cohort".into()),
            }),
            members: vec![CollectionMember {
                reference: "r".into(),
                manifest_hash: "blake3:h".into(),
                role: Role::Derived,
                kind: MemberKind::Collection,
                derived_from: vec!["p".into()],
            }],
            content_hash: Some("blake3:c".into()),
            manifest_hash: Some("blake3:m".into()),
            newer_fields: Vec::new(),
        };
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(keys(&v), sorted(Collection::FIELDS));
        assert_eq!(keys(&v["level"]), sorted(crate::schema::Coded::FIELDS));
        assert_eq!(keys(&v["members"][0]), sorted(CollectionMember::FIELDS));
        assert!(newer_fields_of(&v).is_empty());
    }

    /// `to_stored` emits only bytes that carry their own seal (#873), and a view with newer fields is
    /// refused rather than re-serialised without them.
    #[test]
    fn to_stored_refuses_a_view_that_does_not_reproduce_its_seal() {
        let sealed = CollectionBuilder::new("c", "d", TS).seal().unwrap();
        let (bytes, seal) = sealed.to_stored().unwrap();
        Collection::from_stored(&bytes, &seal).unwrap();
        let mut v: Value = serde_json::from_slice(&bytes).unwrap();
        v["annotations"] = serde_json::json!(1);
        let newer_bytes = crate::canonical::to_bytes(&v).unwrap();
        let newer =
            Collection::from_stored(&newer_bytes, &crate::hash::digest(&newer_bytes)).unwrap();
        assert!(matches!(
            newer.to_stored(),
            Err(crate::Error::NewerFormatFields { .. })
        ));
        let mut edited = sealed.clone();
        edited.description = "changed".into();
        assert!(matches!(
            edited.to_stored(),
            Err(crate::Error::Integrity { .. })
        ));
        assert!(Collection::new("c", "d", TS).to_stored().is_err());
    }

    // ---- ADR-0049 recursion mechanism (Part 1) ----

    /// Corpus-neutrality: a product-only collection serializes with NO `kind` key (default skipped),
    /// so pre-recursion collections are byte-identical. ADR-0049 §1.
    #[test]
    fn product_only_collection_omits_the_kind_key() {
        let raw = raw_listmode();
        let recon = derived_recon(&raw);
        let json = build_collection(&raw, &recon).to_json().unwrap();
        assert!(
            !json.contains("\"kind\""),
            "default Product kind must be skipped on serialize:\n{json}"
        );
    }

    /// `content_hash` is **kind-bound**: the same `reference`+`manifest_hash` folded as a product vs.
    /// a sub-collection yields DIFFERENT roots, and the sub-collection member emits `"kind"`. ADR-0049 §2.
    #[test]
    fn content_hash_is_domain_separated_by_kind() {
        let raw = raw_listmode();
        let handle_ref = &raw.id;
        let handle_mh = raw.manifest_hash.clone().unwrap();

        let mut as_product = CollectionBuilder::new("c", "d", TS);
        as_product.add_member(handle_ref, &handle_mh, Role::Raw, Vec::new());
        let p = as_product.seal().unwrap();

        let child = CollectionHandle {
            reference: handle_ref.clone(),
            manifest_hash: handle_mh.clone(),
        };
        let mut as_collection = CollectionBuilder::new("c", "d", TS);
        as_collection.add_subcollection(&child, Role::Derived, Vec::new());
        let c = as_collection.seal().unwrap();

        assert_ne!(
            p.content_hash, c.content_hash,
            "flipping a member's kind must move content_hash (leaf domain separation)"
        );
        c.verify().unwrap();
        assert!(c.to_json().unwrap().contains("\"kind\": \"collection\""));
    }

    /// The open `level` tag is optional (omitted when None) and **bound in the seal** when present —
    /// but the engine never interprets it. ADR-0049 §5.
    #[test]
    fn level_tag_is_optional_and_sealed_but_uninterpreted() {
        let plain = CollectionBuilder::new("cohort-A", "a cohort", TS)
            .seal()
            .unwrap();
        assert!(!plain.to_json().unwrap().contains("\"level\""));

        let mut b = CollectionBuilder::new("cohort-A", "a cohort", TS);
        b.with_level(crate::schema::Coded::new(
            "tessera-imaging-levels",
            "cohort",
        ));
        let leveled = b.seal().unwrap();
        let json = leveled.to_json().unwrap();
        assert!(json.contains("tessera-imaging-levels") && json.contains("\"cohort\""));
        // The level is part of the manifest_hash seal (same name/timestamp, only level differs).
        assert_ne!(
            leveled.manifest_hash, plain.manifest_hash,
            "level must be committed by the seal"
        );
        leveled.verify().unwrap();
    }

    /// An unknown `kind` is REFUSED by the parser (serde rejects the variant; never swallowed). ADR-0049 §1.
    #[test]
    fn unknown_member_kind_is_refused() {
        let child = CollectionHandle {
            reference: "recon-x".into(),
            manifest_hash: "blake3:abc".into(),
        };
        let mut b = CollectionBuilder::new("c", "d", TS);
        b.add_subcollection(&child, Role::Derived, Vec::new());
        let json = b
            .seal()
            .unwrap()
            .to_json()
            .unwrap()
            .replace("\"kind\": \"collection\"", "\"kind\": \"teleport\"");
        assert!(
            Collection::from_json(&json).is_err(),
            "an unknown member kind must be refused, not silently accepted"
        );
    }

    /// End-to-end: a two-level nest (a cohort with a product + a sub-collection member) built through
    /// the typed handles, sealed, round-tripped, and verified. ADR-0049 §1/§4.
    #[test]
    fn nested_collection_via_typed_handles_round_trips() {
        let raw = raw_listmode();
        let recon = derived_recon(&raw);
        let exam = build_collection(&raw, &recon); // an L1 "exam" sub-collection (sealed)

        let mut cohort = CollectionBuilder::new("cohort-2024", "a cohort", TS);
        cohort.add_product(
            &ProductHandle::of(&recon).unwrap(),
            Role::Derived,
            Vec::new(),
        );
        cohort.add_subcollection(
            &CollectionHandle::of(&exam).unwrap(),
            Role::Derived,
            Vec::new(),
        );
        let sealed = cohort.seal().unwrap();

        assert_eq!(sealed.members[0].kind, MemberKind::Product);
        assert_eq!(sealed.members[1].kind, MemberKind::Collection);
        assert_eq!(sealed.members[1].reference, exam.id);

        let (bytes, seal) = sealed.to_stored().unwrap();
        let parsed = Collection::from_stored(&bytes, &seal).unwrap();
        assert_eq!(parsed.members[1].kind, MemberKind::Collection);
        assert_eq!(parsed.content_hash, sealed.content_hash);
    }

    /// The property leaf domain-separation exists for: a **kind-flip on a *sealed document***
    /// (collection→product) is caught by `verify` as a `content_hash` mismatch — not merely builder
    /// divergence (round-2 review). ADR-0049 §2.
    #[test]
    fn kind_flip_on_a_sealed_doc_is_caught_by_verify() {
        let child = CollectionHandle {
            reference: "exam-1".into(),
            manifest_hash: "blake3:abc".into(),
        };
        let mut b = CollectionBuilder::new("cohort", "d", TS);
        b.add_subcollection(&child, Role::Derived, Vec::new());
        let sealed = b.seal().unwrap();
        let tampered = sealed
            .to_json()
            .unwrap()
            .replace("\"kind\": \"collection\"", "\"kind\": \"product\"");
        let parsed = Collection::from_json(&tampered).unwrap(); // product is a valid kind → parses
        match parsed.verify() {
            Err(crate::Error::Integrity { what, .. }) => assert_eq!(what, "content_hash"),
            other => panic!("expected content_hash integrity error on kind-flip, got {other:?}"),
        }
    }

    /// The `add_member` footgun is detectable: adding a sub-collection via the raw `add_member`
    /// (product-kind) seals a DIFFERENT `content_hash` than `add_subcollection`. ADR-0049 §4.
    #[test]
    fn add_member_and_add_subcollection_seal_different_content_hash() {
        let child = CollectionHandle {
            reference: "x".into(),
            manifest_hash: "blake3:z".into(),
        };
        let mut raw = CollectionBuilder::new("c", "d", TS);
        raw.add_member(
            child.reference(),
            child.manifest_hash(),
            Role::Derived,
            Vec::new(),
        );
        let mut typed = CollectionBuilder::new("c", "d", TS);
        typed.add_subcollection(&child, Role::Derived, Vec::new());
        assert_ne!(
            raw.seal().unwrap().content_hash,
            typed.seal().unwrap().content_hash
        );
    }

    /// An explicit `"kind": "product"` from another writer parses to the default (serde compat).
    #[test]
    fn explicit_product_kind_parses_to_default() {
        let m: CollectionMember = serde_json::from_str(
            r#"{"reference":"r","manifest_hash":"blake3:h","role":"raw","kind":"product"}"#,
        )
        .unwrap();
        assert_eq!(m.kind, MemberKind::Product);
    }

    /// The member-filename SSoT (#323): a `blake3:<hex>` reference sanitizes its single `:` to `_`
    /// (lossless) and the extension follows the kind — the one name writers and readers must agree on.
    #[test]
    fn member_filename_sanitizes_the_reference_and_picks_the_extension() {
        let r = "blake3:e4b561abc";
        assert_eq!(sanitize_reference(r), "blake3_e4b561abc");
        assert_eq!(
            member_filename(r, MemberKind::Product),
            "blake3_e4b561abc.tsra"
        );
        assert_eq!(
            member_filename(r, MemberKind::Collection),
            "blake3_e4b561abc.collection.json"
        );
        // Path-separators are neutralised too, so a reference can never escape the collection dir.
        assert_eq!(sanitize_reference("a/b\\c"), "a_b_c");
    }

    /// A malformed (empty) level tag is refused at seal — structural well-formedness (round-2 review).
    #[test]
    fn empty_level_tag_is_refused_at_seal() {
        let mut b = CollectionBuilder::new("c", "d", TS);
        b.with_level(crate::schema::Coded::new("", ""));
        assert!(b.seal().is_err());
    }

    // ---- #294 CollectionSchema: levels as registered, versioned schemas ----

    fn a_sealed_product() -> Manifest {
        ProductBuilder::new("recon", "p", "d", TS).seal().unwrap()
    }
    fn a_sealed_collection() -> Collection {
        let p = a_sealed_product();
        let mut cb = CollectionBuilder::new("child", "d", TS);
        cb.add_product(&ProductHandle::of(&p).unwrap(), Role::Raw, Vec::new());
        cb.seal().unwrap()
    }

    #[test]
    fn builtin_registry_has_the_three_core_levels() {
        let r = CollectionSchemaRegistry::builtin();
        for id in ["collection", "dataset", "project"] {
            assert!(r.get(id).is_some(), "missing built-in level '{id}'");
        }
        assert!(r.get("exam").is_none(), "domain levels are NOT built in");
    }

    #[test]
    fn dataset_accepts_products_and_rejects_sub_collections() {
        let p = a_sealed_product();
        let child = a_sealed_collection();
        // dataset + a product member → OK.
        let mut ok = CollectionBuilder::new("ds", "d", TS);
        ok.with_collection_schema("dataset").add_product(
            &ProductHandle::of(&p).unwrap(),
            Role::Raw,
            Vec::new(),
        );
        ok.seal().unwrap().validate_schema().unwrap();
        // dataset + a sub-collection member → refused at seal.
        let mut bad = CollectionBuilder::new("ds", "d", TS);
        bad.with_collection_schema("dataset").add_subcollection(
            &CollectionHandle::of(&child).unwrap(),
            Role::Derived,
            Vec::new(),
        );
        assert!(bad.seal().is_err(), "dataset must reject a sub-collection");
    }

    #[test]
    fn project_accepts_sub_collections_and_rejects_products() {
        let p = a_sealed_product();
        let child = a_sealed_collection();
        let mut ok = CollectionBuilder::new("pr", "d", TS);
        ok.with_collection_schema("project").add_subcollection(
            &CollectionHandle::of(&child).unwrap(),
            Role::Derived,
            Vec::new(),
        );
        ok.seal().unwrap().validate_schema().unwrap();
        let mut bad = CollectionBuilder::new("pr", "d", TS);
        bad.with_collection_schema("project").add_product(
            &ProductHandle::of(&p).unwrap(),
            Role::Raw,
            Vec::new(),
        );
        assert!(bad.seal().is_err(), "project must reject a product member");
    }

    #[test]
    fn generic_and_unknown_levels_are_permissive_and_schema_is_sealed() {
        let p = a_sealed_product();
        let child = a_sealed_collection();
        // Default generic "collection": both kinds allowed.
        let mut c = CollectionBuilder::new("mix", "d", TS);
        c.add_product(&ProductHandle::of(&p).unwrap(), Role::Raw, Vec::new())
            .add_subcollection(
                &CollectionHandle::of(&child).unwrap(),
                Role::Derived,
                Vec::new(),
            );
        let sealed = c.seal().unwrap();
        assert_eq!(sealed.collection_schema, "collection");

        // Unknown (domain) level → permissive (open-world), and the schema round-trips + is sealed.
        let mut d = CollectionBuilder::new("cohort-1", "d", TS);
        d.with_collection_schema("exam").add_product(
            &ProductHandle::of(&p).unwrap(),
            Role::Raw,
            Vec::new(),
        );
        let e = d.seal().unwrap();
        assert_eq!(e.collection_schema, "exam");
        let (bytes, seal) = e.to_stored().unwrap();
        let json = String::from_utf8(bytes).unwrap();
        assert!(json.contains("\"collection_schema\":\"exam\""));
        // The schema is bound in the seal: flipping it on-disk breaks manifest_hash verification.
        let tampered = json.replace("\"exam\"", "\"dataset\"");
        assert!(matches!(
            Collection::from_stored(tampered.as_bytes(), &seal),
            Err(crate::Error::Integrity { .. })
        ));
    }
}
