//! The manifest — the self-describing, hashable spine of a Tessera product (see ADR-0020).
//!
//! Three hashes, three jobs:
//! - [`id`](Manifest::id) — *logical identity*: `blake3` over canonical-JSON of [`id_inputs`].
//!   Rename- and re-encode-stable; survives re-ingest of the same logical product.
//! - [`content_hash`](Manifest::content_hash) — *data fingerprint*: Merkle root over the ordered
//!   block digests. Changes iff payload bytes change.
//! - [`manifest_hash`](Manifest::manifest_hash) — *the seal*: `blake3` over the **stored bytes** of
//!   `manifest.json` (ADR-0066). Covers metadata + sources + block refs (which carry the block
//!   digests), so it transitively commits to all payloads — tamper anything, it changes.
//!
//! **What the seal covers (SPEC §4, ADR-0066).** `manifest.json` is stored as exactly its RFC 8785
//! (JCS) bytes, with no `manifest_hash` key, and the seal is `blake3` over those bytes, kept beside
//! them in a `manifest.seal` member ([`crate::stored`]). A reader never re-serialises what it read:
//! [`Manifest::from_stored`] refuses bytes that are not their own JCS form, then compares the seal
//! with the digest of the bytes as stored, then builds the typed view. The view drops keys it doesn't
//! model (a newer writer's fields, reported by [`Manifest::newer_fields`], ADR-0060 §2.2), so a
//! product is re-emitted by copying its stored bytes, never by serialising the view. A writer stores
//! [`Manifest::to_stored`]: the bytes it seals are the bytes it writes.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::block::BlockRef;
use crate::field_path::FieldPath;
use crate::provenance::{Generation, IngestTransform, Producer, ProducerRef, Source};

/// Format/spec version this build writes.
pub const TESSERA_VERSION: &str = "0.0.0";

/// Highest major spec version this reader understands. A manifest with a higher major
/// version is refused rather than silently misread.
pub const SUPPORTED_MAJOR: u64 = 0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    /// Format/spec version.
    pub tessera_version: String,
    /// Stable, content-derived logical identity (see [`crate::identity`]).
    pub id: String,
    /// Declared identity inputs (key→value) that `id` is hashed over — recorded so identity is
    /// transparent and independently verifiable. Default keys: `product`/`name`/`timestamp`.
    pub id_inputs: BTreeMap<String, String>,
    /// Product schema name, e.g. "recon", "listmode", "spectrum".
    pub product: String,
    pub name: String,
    pub description: String,
    /// RFC 3339 timestamp, normalized to UTC.
    pub timestamp: String,
    /// The tool/build that produced this product — sealed provenance so a reader knows *what wrote
    /// the file* (ADR-0058 §1). A structured [`ProducerRef::Structured`] on newly-sealed products;
    /// a legacy bare string (`"tessera/0.0.0"`) round-trips unchanged on pre-ADR-0058 files. `None`
    /// only for pre-stamp legacy files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub producer: Option<ProducerRef>,
    /// Generation record (ADR-0058 §2) — *how* this product was made: producer's config/settings as
    /// a generic bag (inline or a `config_ref` to a carried block). Sealed. Required at validate for
    /// products whose schema sets `requires_generation`; absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<Generation>,
    /// Embedded JSON Schema for the product (opaque to the core).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<serde_json::Value>,
    /// Typed storage blocks composing this product (order is significant for the Merkle root).
    #[serde(default)]
    pub blocks: Vec<BlockRef>,
    /// Provenance DAG: where this product came from.
    #[serde(default)]
    pub sources: Vec<Source>,
    /// Optional study/grouping id (fd5 `study`) — ties together the products of one exam
    /// (a study's CT + PET + listmode share a `study`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub study: Option<String>,
    /// Values for the product schema's declared metadata fields (fd5 field model), keyed by each
    /// field's stable `id`. Schema-governed — distinct from `extra` (non-standard).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, serde_json::Value>,
    /// Extension namespace (fd5 `extra/`) for non-standard / vendor metadata. Preserved through
    /// round-trips and covered by `manifest_hash`, but never schema-validated and never allowed
    /// to collide with core keys (it is a nested object).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, serde_json::Value>,
    /// **Recorded transforms** applied when this product was normalised at the door (ADR-0056 §6.2).
    ///
    /// The receipt for the ladder's middle lane: every mapping that changed the *representation* of a
    /// value while preserving the value (decimal → integer + scale, tz → UTC ticks, f16 → f32,
    /// dictionary → materialised values, struct → dotted columns). Empty for products whose ingest was
    /// bit-faithful, and therefore for every vendor path that existed before generic ingest.
    ///
    /// Sealed, and for the §6 reason: it **changes what the values mean**, so a reader who cannot see
    /// it cannot reconstruct the source's semantics. `skip_serializing_if` keeps an untransformed
    /// product's JSON byte-identical to a pre-ADR-0056 manifest, which is what lets the committed
    /// conformance corpus keep its goldens.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ingest_transform: Vec<crate::provenance::IngestTransform>,
    /// Merkle root over block digests; `Some` once sealed, `None` while building.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    /// The seal: `blake3` over this manifest's stored bytes ([`to_stored`](Self::to_stored)).
    /// `Some` once sealed, `None` while building. **Never serialised** (ADR-0066 rule 1): the seal
    /// is kept outside the bytes it seals, in the `manifest.seal` member, and a reader sets it from
    /// there ([`from_stored`](Self::from_stored)).
    #[serde(skip)]
    pub manifest_hash: Option<String>,
    /// Keys of the parsed manifest this build does not model (see [`Self::newer_fields`]). Never
    /// serialised: it describes the received object, not the product.
    #[serde(skip)]
    newer_fields: Vec<String>,
}

/// The in-band seal key of the alpha layout. Stored bytes that carry it at their top level are
/// refused ([`crate::Error::InBandSeal`]); a [`crate::Collection`] is held to the same rule.
pub(crate) const SEAL_KEY: &str = "manifest_hash";

/// Every key of the object `obj` at `at` that its type doesn't declare (`declared`, the type's
/// `FIELDS`), as `at` + key: the declared-field walk of ADR-0060 §2.2, shared by the manifest, the
/// collection and the embedded product schema. A key is a newer field iff its parent type does not
/// declare it, and that is decided here, per parent, on the structured path. Only keys count, never
/// values. A non-object `obj` (absent, `null`) contributes nothing.
pub(crate) fn unknown_keys(
    obj: &Value,
    declared: &[&str],
    at: &FieldPath,
    out: &mut Vec<FieldPath>,
) {
    if let Value::Object(map) = obj {
        let unknown = map.keys().filter(|k| !declared.contains(&k.as_str()));
        out.extend(unknown.map(|k| at.key(k)));
    }
}

/// [`unknown_keys`] for each entry of the array `list` at `at`, as `at[i]` + key.
pub(crate) fn unknown_keys_each(
    list: &Value,
    declared: &[&str],
    at: &FieldPath,
    out: &mut Vec<FieldPath>,
) {
    if let Value::Array(items) = list {
        for (i, item) in items.iter().enumerate() {
            unknown_keys(item, declared, &at.index(i), out);
        }
    }
}

/// `Err(NewerFormatFields)` when `fields` is non-empty: the one refusal every view of a parsed
/// object raises before it would build a seal from, validate against or re-emit what it dropped.
pub(crate) fn refuse_newer(fields: &[String]) -> crate::Result<()> {
    if fields.is_empty() {
        return Ok(());
    }
    Err(crate::Error::NewerFormatFields {
        fields: fields.to_vec(),
    })
}

/// The paths of every key in a manifest object that this build's types don't declare
/// (ADR-0060 §2.2): top level, `producer`, `generation`, and each entry of `blocks`, `sources` and
/// `ingest_transform`. Opaque maps (`id_inputs`, `metadata`, `extra`, `schema`, `generation.config`,
/// `blocks[].spec`, `ingest_transform[].params`) are data, not format, and are never walked. Only
/// keys count, never values: a known key holding `null` is not a newer field.
fn newer_fields_of(object: &Value) -> Vec<FieldPath> {
    let mut out = Vec::new();
    let root = FieldPath::root();
    unknown_keys(object, Manifest::FIELDS, &root, &mut out);
    let get = |k: &str| object.get(k).unwrap_or(&Value::Null);
    unknown_keys(
        get("producer"),
        Producer::FIELDS,
        &root.key("producer"),
        &mut out,
    );
    unknown_keys(
        get("generation"),
        Generation::FIELDS,
        &root.key("generation"),
        &mut out,
    );
    let lists = [
        ("blocks", BlockRef::FIELDS),
        ("sources", Source::FIELDS),
        ("ingest_transform", IngestTransform::FIELDS),
    ];
    for (key, declared) in lists {
        unknown_keys_each(get(key), declared, &root.key(key), &mut out);
    }
    out
}

impl Manifest {
    pub fn new(
        product: impl Into<String>,
        name: impl Into<String>,
        description: impl Into<String>,
        timestamp: impl Into<String>,
    ) -> Self {
        let product = product.into();
        let name = name.into();
        let description = description.into();
        // Normalise to UTC so equivalent instants in different offsets share one id.
        let timestamp = crate::identity::normalize_timestamp(&timestamp.into());
        let id_inputs = crate::identity::default_id_inputs(&product, &name, &timestamp);
        // Infallible: a string→string map always canonicalizes (see compute_id).
        let id =
            crate::identity::compute_id(&id_inputs).expect("string id_inputs always canonicalize");
        Manifest {
            tessera_version: TESSERA_VERSION.to_string(),
            id,
            id_inputs,
            product,
            name,
            description,
            timestamp,
            producer: None,
            generation: None,
            schema: None,
            blocks: Vec::new(),
            sources: Vec::new(),
            study: None,
            metadata: BTreeMap::new(),
            extra: BTreeMap::new(),
            ingest_transform: Vec::new(),
            content_hash: None,
            manifest_hash: None,
            newer_fields: Vec::new(),
        }
    }

    /// The JSON keys this type models. A test keeps it equal to the keys of a fully populated
    /// manifest, so a new field can't be added without it.
    pub(crate) const FIELDS: &'static [&'static str] = &[
        "tessera_version",
        "id",
        "id_inputs",
        "product",
        "name",
        "description",
        "timestamp",
        "producer",
        "generation",
        "schema",
        "blocks",
        "sources",
        "study",
        "metadata",
        "extra",
        "ingest_transform",
        "content_hash",
    ];

    /// Fields of the parsed manifest that this build does not model: keys a **newer format
    /// revision** added (RFC §13, additive-only evolution), as paths (`actors`, `producer.agents`,
    /// `blocks[0].stats`). A key that is not a non-empty run of ASCII letters, digits and `_`
    /// starting with a letter or `_` is quoted, so a path never reads as another: a top-level key
    /// named `producer.agents` is `["producer.agents"]` (the grammar is in SPEC §4, "Forward
    /// reading"). Empty for a manifest this build wrote, and for every
    /// manifest not parsed by [`from_json`](Self::from_json) / [`from_stored`](Self::from_stored).
    ///
    /// These fields are covered by the seal (it is over the stored bytes), but not interpreted, and this struct does not carry them. So every
    /// operation that would build a new seal from this manifest refuses with
    /// [`crate::Error::NewerFormatFields`] instead of dropping them (ADR-0060 §2.2).
    pub fn newer_fields(&self) -> &[String] {
        &self.newer_fields
    }

    /// [`crate::Error::NewerFormatFields`] if [`newer_fields`](Self::newer_fields) is non-empty.
    pub fn refuse_newer_fields(&self) -> crate::Result<()> {
        refuse_newer(&self.newer_fields)
    }

    /// True once the product has been sealed (carries a manifest hash).
    pub fn is_sealed(&self) -> bool {
        self.manifest_hash.is_some()
    }

    /// Pretty JSON, for humans / display. NOT the hashed form — see [`canonical_bytes`].
    pub fn to_json(&self) -> crate::Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Canonical (RFC 8785 JCS) bytes of this struct: the bytes [`to_stored`](Self::to_stored)
    /// stores and [`compute_manifest_hash`](Self::compute_manifest_hash) hashes. They never carry
    /// `manifest_hash` (the field is not serialised).
    ///
    /// # Errors
    /// As [`compute_manifest_hash`](Self::compute_manifest_hash).
    pub fn canonical_bytes(&self) -> crate::Result<Vec<u8>> {
        self.refuse_newer_fields()?;
        crate::canonical::to_bytes(&crate::canonical::to_json_value(self)?)
    }

    /// Recompute the logical `id` from `id_inputs`.
    pub fn recompute_id(&self) -> crate::Result<String> {
        crate::identity::compute_id(&self.id_inputs)
    }

    /// Recompute the content Merkle root from the block digests. Errors if any block ref is
    /// missing its digest.
    pub fn recompute_content_hash(&self) -> crate::Result<String> {
        let mut digests = Vec::with_capacity(self.blocks.len());
        for b in &self.blocks {
            match &b.digest {
                Some(d) => digests.push(d.clone()),
                None => return Err(crate::Error::MissingDigest(b.name.to_string())),
            }
        }
        Ok(crate::hash::merkle_root(&digests))
    }

    /// The seal (`manifest_hash`) this struct gets when written out: `blake3` over
    /// [`canonical_bytes`](Self::canonical_bytes), the bytes [`to_stored`](Self::to_stored) stores.
    ///
    /// # Errors
    /// [`crate::Error::NewerFormatFields`] for a parsed manifest with [`newer_fields`](Self::newer_fields):
    /// a seal over this struct would leave them out, which is a re-seal that drops sealed content.
    pub fn compute_manifest_hash(&self) -> crate::Result<String> {
        Ok(crate::hash::digest(&self.canonical_bytes()?))
    }

    /// The typed view of a strictly parsed manifest object: declared form, version, newer fields.
    fn from_object(object: &Value) -> crate::Result<Self> {
        let newer = newer_fields_of(object);
        let mut m: Manifest = crate::canonical::from_declared_form(object, &newer)?;
        m.check_version()?;
        m.newer_fields = crate::field_path::render(&newer);
        Ok(m)
    }

    /// Parse a manifest, refusing one whose major spec version this reader can't handle.
    /// **Does not verify anything**: the result is an untrusted claim (a diagnostic read, an
    /// unverified id). A reader verifies with [`from_stored_members`](Self::from_stored_members).
    ///
    /// # Errors
    /// [`crate::Error::Serde`] for malformed JSON, a **duplicate key** in any object
    /// ([`crate::canonical::parse_strict`]), a known field of the wrong type, or the manifest or a
    /// record in it (`producer`, `generation`, `blocks[i]`, `sources[i]`, `ingest_transform[i]`) in
    /// array form instead of an object, or a block `kind` spelled as an object instead of a string
    /// (SPEC §4, one spelling per value; the message names its path);
    /// [`crate::Error::InexactInteger`] for a number that is an integer beyond ±(2^53−1) anywhere in
    /// it (SPEC §1, I-JSON: no seal pins it; the message names its path);
    /// [`crate::Error::UnsupportedVersion`] for a newer major version.
    pub fn from_json(s: &str) -> crate::Result<Self> {
        Self::from_object(&crate::canonical::parse_strict(s)?)
    }

    /// The checks a reader runs on a received manifest's typed view before (or after) its seal:
    /// unique block names, then `id`, then `content_hash`, which must be present.
    fn verify_received_id_and_content(&self) -> crate::Result<()> {
        crate::block::check_unique_names(self.blocks.iter().map(|b| &b.name))?;
        check("id", &self.id, &self.recompute_id()?)?;
        check(
            "content_hash",
            &recorded(&self.content_hash),
            &self.recompute_content_hash()?,
        )
    }

    /// Read a manifest from its **stored bytes** and the seal recorded beside them (ADR-0066): the
    /// verification order of ADR-0066 rule 4, and the reader's half of [`to_stored`](Self::to_stored).
    ///
    /// Under ADR-0066 `manifest.json` is stored as exactly its RFC 8785 (JCS) bytes, with no
    /// `manifest_hash` key, and the seal is `"blake3:" + hex(blake3(bytes))`, kept outside them (a
    /// `manifest.seal` member). In order:
    ///
    /// 1. [`crate::canonical::check_canonical`]: strict parse (UTF-8, duplicate keys, lone
    ///    surrogates, I-JSON integers, depth), then the bytes must equal their own JCS form;
    /// 2. the bytes carry no top-level `manifest_hash` key;
    /// 3. `seal` equals `blake3` over the bytes: a byte-for-byte comparison of the two strings, so a
    ///    seal in any other spelling (upper-case hex, no `blake3:` prefix) does not match;
    /// 4. the typed layer: declared form, version, unique block names, `id` and `content_hash`
    ///    (which must be present).
    ///
    /// The returned view carries `seal` as its [`manifest_hash`](Self::manifest_hash), so it
    /// [`is_sealed`](Self::is_sealed), and lists any [`newer_fields`](Self::newer_fields): a key a
    /// newer writer sealed verifies here and is reported.
    ///
    /// The seal covers the stored bytes, not the typed view. Canonical bytes the view normalises
    /// (an explicit `"study": null`, which the view drops) are accepted here;
    /// [`verify`](Self::verify) and [`to_stored`](Self::to_stored) on the result then fail with
    /// [`crate::Error::Integrity`], because the view's bytes are no longer the sealed bytes. A
    /// reader that re-emits a product copies its stored bytes.
    ///
    /// A container reader calls [`from_stored_members`](Self::from_stored_members), which also
    /// refuses a missing or malformed `manifest.seal`.
    ///
    /// # Errors
    /// In the order above: everything [`crate::canonical::check_canonical`] refuses
    /// ([`crate::Error::Serde`], [`crate::Error::InexactInteger`],
    /// [`crate::Error::NonCanonical`] with the offset of the first differing byte);
    /// [`crate::Error::InBandSeal`] for a `manifest_hash` key; [`crate::Error::Integrity`] with
    /// `what` = `manifest_hash`, `expected` = `seal` and `actual` = the stored bytes' digest; then
    /// everything [`from_json`](Self::from_json) refuses past the parse (declared form,
    /// [`crate::Error::UnsupportedVersion`]), [`crate::Error::DuplicateBlockName`],
    /// [`crate::Error::Integrity`] for `id` or `content_hash` (`expected` reads `(absent)` for a
    /// missing `content_hash`), and [`crate::Error::MissingDigest`] for an undigested block.
    pub fn from_stored(bytes: &[u8], seal: &str) -> crate::Result<Self> {
        let object = stored_object(bytes, seal, "manifest")?;
        let mut m = Self::from_object(&object)?;
        m.verify_received_id_and_content()?;
        m.manifest_hash = Some(seal.to_owned());
        Ok(m)
    }

    /// Read a product's manifest from its two stored members as a container holds them:
    /// `manifest.json` and `manifest.seal` (`None` when the container has no such member). The one
    /// door every reader of a `.tsra`, an exploded directory or a repository object goes through.
    ///
    /// A missing seal is the pre-ADR-0066 alpha layout, refused with no migration (ADR-0066 rule 10)
    /// by an error that names what was found (the declared `tessera_version`, the missing
    /// `manifest.seal`, an in-band `manifest_hash`, a non-JCS layout) and the layout required. A
    /// present seal must be the 71 ASCII bytes `blake3:<64 lower-case hex>`; then
    /// [`from_stored`](Self::from_stored) runs.
    ///
    /// # Errors
    /// [`crate::Error::UnsupportedLayout`] without a seal; [`crate::Error::MalformedSeal`] for a
    /// seal in another format; then everything [`from_stored`](Self::from_stored) refuses.
    pub fn from_stored_members(json: &[u8], seal: Option<&[u8]>) -> crate::Result<Self> {
        let seal = crate::stored::require_seal(crate::stored::MANIFEST_LAYOUT, json, seal)?;
        Self::from_stored(json, seal)
    }

    /// This manifest's **stored form** under ADR-0066: `(bytes, seal)`, where `bytes` is its RFC
    /// 8785 (JCS) serialisation with no `manifest_hash` key and `seal` is
    /// `"blake3:" + hex(blake3(bytes))`. The writer's half of [`from_stored`](Self::from_stored):
    /// `from_stored(&bytes, &seal)` reads back a manifest with the same stored bytes and seal.
    ///
    /// `bytes` are [`canonical_bytes`](Self::canonical_bytes), the bytes the alpha layout's
    /// in-band seal was computed over, so a product sealed before ADR-0066 keeps its seal's value
    /// (ADR-0066 rule 2): moving the seal out of the document did not change it.
    ///
    /// The manifest must carry a `content_hash` (a stored manifest is a sealed product) and pass
    /// [`verify`](Self::verify): `id` and `content_hash` hold, block names are unique, and a
    /// `manifest_hash` already set must equal `seal`. An unset `manifest_hash` is not required:
    /// `seal` is computed here.
    ///
    /// # Errors
    /// [`crate::Error::NewerFormatFields`] for a parsed manifest with
    /// [`newer_fields`](Self::newer_fields) (its bytes would drop sealed content: copy the stored
    /// bytes instead); [`crate::Error::Invalid`] without a `content_hash`; everything
    /// [`verify`](Self::verify) raises, including [`crate::Error::Integrity`] for a `manifest_hash`
    /// that is not `seal` (the struct was changed after sealing, or was parsed from an encoding it
    /// normalises, such as an explicit `"study": null`); [`crate::Error::InexactInteger`] or
    /// [`crate::Error::NonFiniteNumber`] for a value no seal can hold.
    pub fn to_stored(&self) -> crate::Result<(Vec<u8>, String)> {
        self.refuse_newer_fields()?;
        if self.content_hash.is_none() {
            return Err(crate::Error::Invalid(
                "refusing to store a manifest without content_hash: a stored manifest is a sealed \
                 product"
                    .into(),
            ));
        }
        self.verify()?;
        let bytes = self.canonical_bytes()?;
        let seal = crate::hash::digest(&bytes);
        Ok((bytes, seal))
    }

    /// Verify this **struct's** three hashes against their recomputed values (and the spec version):
    /// the writer's check on a manifest it built. Any mismatch is a typed [`crate::Error::Integrity`]
    /// naming the field. A building (unsealed) manifest verifies its `id` only; a sealed one verifies
    /// all three.
    ///
    /// The seal is recomputed over this struct's JSON. For a manifest read from bytes, verify the bytes
    /// with [`from_stored`](Self::from_stored) instead: the parsed struct is a view, and
    /// a view of a valid product with [`newer_fields`](Self::newer_fields) fails here with
    /// [`crate::Error::NewerFormatFields`], and one parsed from an encoding the struct normalises
    /// (`"study": null`) fails with `Integrity`.
    ///
    /// First, the block list must name each block once ([`crate::Error::DuplicateBlockName`],
    /// ADR-0063 §2a, SPEC §5): a reader resolves a name to one listed digest, so a second entry's
    /// digest would cover no bytes, however well the list is sealed.
    pub fn verify(&self) -> crate::Result<()> {
        self.verify_id_and_content()?;
        if let Some(mh) = &self.manifest_hash {
            check("manifest_hash", mh, &self.compute_manifest_hash()?)?;
        }
        Ok(())
    }

    fn verify_id_and_content(&self) -> crate::Result<()> {
        self.check_version()?;
        crate::block::check_unique_names(self.blocks.iter().map(|b| &b.name))?;
        check("id", &self.id, &self.recompute_id()?)?;
        if let Some(ch) = &self.content_hash {
            check("content_hash", ch, &self.recompute_content_hash()?)?;
        }
        Ok(())
    }

    /// Error if `tessera_version`'s major exceeds [`SUPPORTED_MAJOR`] (forward-incompat),
    /// or if it is unparsable. Never panics.
    pub fn check_version(&self) -> crate::Result<()> {
        let major = self
            .tessera_version
            .split('.')
            .next()
            .and_then(|x| x.parse::<u64>().ok())
            .ok_or_else(|| {
                crate::Error::UnsupportedVersion(format!(
                    "unparsable tessera_version: {}",
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

/// A recorded hash for an [`crate::Error::Integrity`] message: the value, or `(absent)`.
pub(crate) fn recorded(h: &Option<String>) -> String {
    h.clone().unwrap_or_else(|| "(absent)".into())
}

/// Steps 1–3 of reading a stored document (ADR-0066 rule 4), shared by the manifest and the
/// collection: the canonical check, no in-band seal key, and the seal over the bytes. Returns the
/// parsed object for the typed layer.
pub(crate) fn stored_object(
    bytes: &[u8],
    seal: &str,
    document: &'static str,
) -> crate::Result<Value> {
    let object = crate::canonical::check_canonical(bytes)?;
    if object.get(SEAL_KEY).is_some() {
        return Err(crate::Error::InBandSeal { document });
    }
    check(SEAL_KEY, seal, &crate::hash::digest(bytes))?;
    Ok(object)
}

/// Compare an expected vs recomputed hash, raising a typed integrity error on mismatch.
pub(crate) fn check(what: &'static str, expected: &str, actual: &str) -> crate::Result<()> {
    if expected != actual {
        return Err(crate::Error::Integrity {
            what,
            expected: expected.to_string(),
            actual: actual.to_string(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::{BlockKind, BlockName};
    use crate::Error;
    use serde_json::json;

    fn keys(v: &Value) -> Vec<String> {
        let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
        k.sort();
        k
    }

    fn sorted(fields: &[&str]) -> Vec<String> {
        let mut k: Vec<String> = fields.iter().map(|s| s.to_string()).collect();
        k.sort();
        k
    }

    /// Each declared-field list equals the keys a fully populated instance serialises to, so a new
    /// field can't be added to a type without the `newer_fields` walk learning it (ADR-0060 §2.2).
    #[test]
    fn declared_field_lists_match_fully_populated_instances() {
        let mut producer = Producer::new("tool", "1");
        producer.git_commit = Some("c".into());
        producer.git_repo = Some("r".into());
        producer.dirty = Some(false);
        let mut generation = Generation::default().with_config_ref("blake3:00");
        generation.config.insert("k".into(), json!(1));
        let mut m = Manifest::new("recon", "n", "d", "2024-01-01T00:00:00Z");
        m.producer = Some(ProducerRef::Structured(producer.clone()));
        m.generation = Some(generation.clone());
        m.schema = Some(json!({}));
        m.blocks = vec![BlockRef {
            name: BlockName::new("b").unwrap(),
            kind: BlockKind::Blob,
            digest: Some("blake3:00".into()),
            spec: json!({}),
        }];
        m.sources = vec![Source::new("r", "x").with_content_hash("blake3:00")];
        m.study = Some("s".into());
        m.metadata.insert("k".into(), json!(1));
        m.extra.insert("k".into(), json!(1));
        m.ingest_transform = vec![IngestTransform::new("t").with("k", json!(1))];
        m.content_hash = Some("blake3:00".into());
        m.manifest_hash = Some("blake3:00".into());

        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(keys(&v), sorted(Manifest::FIELDS));
        assert_eq!(keys(&v["producer"]), sorted(Producer::FIELDS));
        assert_eq!(keys(&v["generation"]), sorted(Generation::FIELDS));
        assert_eq!(keys(&v["blocks"][0]), sorted(BlockRef::FIELDS));
        assert_eq!(keys(&v["sources"][0]), sorted(Source::FIELDS));
        assert_eq!(
            keys(&v["ingest_transform"][0]),
            sorted(IngestTransform::FIELDS)
        );
        // A fully known manifest has no newer fields, and opaque maps are never walked.
        let mut opaque = v.clone();
        opaque["metadata"]["anything"] = json!(1);
        opaque["blocks"][0]["spec"]["anything"] = json!(1);
        opaque["generation"]["config"]["anything"] = json!(1);
        assert!(newer_fields_of(&opaque).is_empty());
        // A legacy string producer has no keys to walk.
        opaque["producer"] = json!("tessera/0.0.0");
        assert!(newer_fields_of(&opaque).is_empty());
    }

    /// A newer field verifies when sealed and is listed by path; the same field spliced in under the
    /// old seal is tampering (#573 cases B and C on a manifest built here, not only the corpus).
    #[test]
    fn a_newer_field_is_sealed_and_named_and_a_spliced_one_is_tampering() {
        let mut m = Manifest::new("recon", "n", "d", "2024-01-01T00:00:00Z");
        m.content_hash = Some(m.recompute_content_hash().unwrap());
        let (_, seal) = m.to_stored().unwrap();
        let mut v = serde_json::to_value(&m).unwrap();
        v["sources"] = json!([{"role": "r", "reference": "x", "weight": 0.5}]);
        v["actors"] = json!([]);
        let spliced = crate::canonical::to_bytes(&v).unwrap();
        let err = Manifest::from_stored(&spliced, &seal).unwrap_err();
        assert!(
            matches!(
                err,
                Error::Integrity {
                    what: "manifest_hash",
                    ..
                }
            ),
            "{err}"
        );
        let ok = Manifest::from_stored(&spliced, &crate::hash::digest(&spliced)).unwrap();
        assert_eq!(ok.newer_fields(), ["actors", "sources[0].weight"]);
    }
}
