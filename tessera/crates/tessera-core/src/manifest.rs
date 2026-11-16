//! The manifest — the self-describing, hashable spine of a Tessera product (see ADR-0020).
//!
//! Three hashes, three jobs:
//! - [`id`](Manifest::id) — *logical identity*: `blake3` over canonical-JSON of [`id_inputs`].
//!   Rename- and re-encode-stable; survives re-ingest of the same logical product.
//! - [`content_hash`](Manifest::content_hash) — *data fingerprint*: Merkle root over the ordered
//!   block digests. Changes iff payload bytes change.
//! - [`manifest_hash`](Manifest::manifest_hash) — *the seal*: `blake3` over canonical-JSON of the
//!   whole manifest (this field excluded). Covers metadata + sources + block refs (which carry the
//!   block digests), so it transitively commits to all payloads — tamper anything, it changes.
//!
//! **What the seal covers (SPEC §4).** The seal is computed over the manifest JSON *object*, with
//! `manifest_hash` removed and nothing else changed ([`seal_of`]). A reader recomputes it over the
//! object it parsed, never over this struct: the struct is a *view* that drops keys it doesn't model,
//! turns an explicit `null` into an absent field and fills omitted defaults, and a seal over the view
//! would pass a spliced-in key and fail a newer writer's valid product (#573). A writer seals the
//! object it is about to write through the same function, so the two can't drift. Keys the view
//! doesn't model are reported by [`Manifest::newer_fields`] (ADR-0060 §2.2).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::block::BlockRef;
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
    /// The seal: `blake3` over canonical-JSON of this manifest with `manifest_hash` itself
    /// omitted. `Some` once sealed, `None` while building.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_hash: Option<String>,
    /// Keys of the parsed manifest this build does not model (see [`Self::newer_fields`]). Never
    /// serialised: it describes the received object, not the product.
    #[serde(skip)]
    newer_fields: Vec<String>,
}

/// The seal of a manifest JSON object (SPEC §4): `digest(JCS(object − "manifest_hash"))`.
///
/// The one seal function: [`Manifest::compute_manifest_hash`] applies it to the object a writer
/// emits, and [`Manifest::from_json_verified`] to the object a reader parsed.
///
/// # Errors
/// [`crate::Error::Canonicalization`] if `object` is not a JSON object.
pub fn seal_of(object: &Value) -> crate::Result<String> {
    crate::canonical::hash_without(object, SEAL_KEY)
}

/// The one key the seal leaves out (SPEC §4). A [`crate::Collection`] is sealed under the same key.
pub(crate) const SEAL_KEY: &str = "manifest_hash";

/// Every key of `obj` outside `declared`, as `prefix + key`: the declared-field walk of ADR-0060
/// §2.2, shared by the manifest, the collection and the embedded product schema. Only keys count,
/// never values. A non-object `obj` (absent, `null`) contributes nothing.
pub(crate) fn unknown_keys(obj: &Value, declared: &[&str], prefix: &str, out: &mut Vec<String>) {
    if let Value::Object(map) = obj {
        let unknown = map.keys().filter(|k| !declared.contains(&k.as_str()));
        out.extend(unknown.map(|k| format!("{prefix}{k}")));
    }
}

/// [`unknown_keys`] for each entry of the array `list`, as `{prefix}[i].key`.
pub(crate) fn unknown_keys_each(
    list: &Value,
    declared: &[&str],
    prefix: &str,
    out: &mut Vec<String>,
) {
    if let Value::Array(items) = list {
        for (i, item) in items.iter().enumerate() {
            unknown_keys(item, declared, &format!("{prefix}[{i}]."), out);
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

/// The dotted paths of every key in a manifest object that this build's types don't declare
/// (ADR-0060 §2.2): top level, `producer`, `generation`, and each entry of `blocks`, `sources` and
/// `ingest_transform`. Opaque maps (`id_inputs`, `metadata`, `extra`, `schema`, `generation.config`,
/// `blocks[].spec`, `ingest_transform[].params`) are data, not format, and are never walked. Only
/// keys count, never values: a known key holding `null` is not a newer field.
fn newer_fields_of(object: &Value) -> Vec<String> {
    let mut out = Vec::new();
    unknown_keys(object, Manifest::FIELDS, "", &mut out);
    let get = |k: &str| object.get(k).unwrap_or(&Value::Null);
    unknown_keys(get("producer"), Producer::FIELDS, "producer.", &mut out);
    unknown_keys(
        get("generation"),
        Generation::FIELDS,
        "generation.",
        &mut out,
    );
    let lists = [
        ("blocks", BlockRef::FIELDS),
        ("sources", Source::FIELDS),
        ("ingest_transform", IngestTransform::FIELDS),
    ];
    for (key, declared) in lists {
        unknown_keys_each(get(key), declared, key, &mut out);
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
        "manifest_hash",
    ];

    /// Fields of the parsed manifest that this build does not model: keys a **newer format
    /// revision** added (RFC §13, additive-only evolution), as dotted paths (`actors`,
    /// `producer.agents`, `blocks[0].stats`). Empty for a manifest this build wrote, and for every
    /// manifest not parsed by [`from_json`](Self::from_json) / [`from_json_verified`](Self::from_json_verified).
    ///
    /// These fields are covered by the seal ([`from_json_verified`](Self::from_json_verified) hashes
    /// the parsed object), but not interpreted, and this struct does not carry them. So every
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

    /// Canonical (RFC 8785 JCS) bytes of this manifest with `manifest_hash` excluded — the
    /// exact bytes [`manifest_hash`](Self::manifest_hash) is computed over when this struct is
    /// written out.
    ///
    /// # Errors
    /// As [`compute_manifest_hash`](Self::compute_manifest_hash).
    pub fn canonical_bytes(&self) -> crate::Result<Vec<u8>> {
        self.refuse_newer_fields()?;
        crate::canonical::bytes_without(&serde_json::to_value(self)?, SEAL_KEY)
    }

    /// This manifest's JSON, for a writer to emit **as a sealed product**: refused unless the bytes
    /// it returns carry their own seal, i.e. [`seal_of`] over them equals `manifest_hash`.
    ///
    /// Every writer that puts a manifest into a product or a repository goes through this, so no
    /// tessera writer emits a manifest whose seal doesn't hold.
    ///
    /// # Errors
    /// - [`crate::Error::NewerFormatFields`] for a parsed manifest with [`newer_fields`](Self::newer_fields):
    ///   this struct doesn't carry them, so its JSON would drop sealed content.
    /// - [`crate::Error::Invalid`] if the manifest is unsealed, or its JSON doesn't reproduce its seal:
    ///   it was edited after sealing, or it was parsed from an encoding this struct normalises (an
    ///   explicit `null`, an omitted `sources`). Copy such a product's original bytes instead.
    pub fn to_sealed_json(&self) -> crate::Result<String> {
        self.refuse_newer_fields()?;
        let Some(mh) = &self.manifest_hash else {
            return Err(crate::Error::Invalid(
                "refusing to emit an unsealed manifest as a product".into(),
            ));
        };
        let json = self.to_json()?;
        let resealed = seal_of(&serde_json::from_str(&json)?)?;
        if &resealed != mh {
            return Err(crate::Error::Invalid(format!(
                "refusing to emit a manifest whose JSON does not reproduce its seal {mh} (it \
                 seals {resealed}): it was changed after sealing, or it was parsed from an \
                 encoding this build normalises; copy the product's original bytes instead"
            )));
        }
        Ok(json)
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

    /// The seal (`manifest_hash`) this struct gets when written out: [`seal_of`] over its JSON
    /// object. The same function a reader applies to the object it parses.
    ///
    /// # Errors
    /// [`crate::Error::NewerFormatFields`] for a parsed manifest with [`newer_fields`](Self::newer_fields):
    /// a seal over this struct would leave them out, which is a re-seal that drops sealed content.
    pub fn compute_manifest_hash(&self) -> crate::Result<String> {
        Ok(crate::hash::digest(&self.canonical_bytes()?))
    }

    /// Parse the text strictly ([`crate::canonical::parse_strict`]) into the object and its typed
    /// view, recording the view's [`newer_fields`](Self::newer_fields).
    fn parse(s: &str) -> crate::Result<(Self, Value)> {
        let object = crate::canonical::parse_strict(s)?;
        let mut m: Manifest = crate::canonical::from_declared_form(&object)?;
        m.check_version()?;
        m.newer_fields = newer_fields_of(&object);
        Ok((m, object))
    }

    /// Parse a manifest, refusing one whose major spec version this reader can't handle.
    /// Does not verify hashes — use [`from_json_verified`](Self::from_json_verified) for that.
    ///
    /// # Errors
    /// [`crate::Error::Serde`] for malformed JSON, a **duplicate key** in any object
    /// ([`crate::canonical::parse_strict`]), a known field of the wrong type, or the manifest or a
    /// record in it (`producer`, `generation`, `blocks[i]`, `sources[i]`, `ingest_transform[i]`) in
    /// array form instead of an object, or a block `kind` spelled as an object instead of a string
    /// (SPEC §4, one spelling per value; the message names its path);
    /// [`crate::Error::UnsupportedVersion`] for a newer major version.
    pub fn from_json(s: &str) -> crate::Result<Self> {
        Ok(Self::parse(s)?.0)
    }

    /// Parse, version-check and verify the three hashes of a manifest **as received** (SPEC §4).
    ///
    /// `id` is recomputed from `id_inputs`, `content_hash` from the block digests, and
    /// `manifest_hash` over the parsed JSON object with only `manifest_hash` removed. So a key a
    /// newer writer added verifies (and is listed by [`newer_fields`](Self::newer_fields)), a key
    /// spliced into a sealed manifest fails, and `"study": null` or an omitted `sources` verifies as
    /// sealed — the same verdict as any SPEC-conformant reader.
    ///
    /// The manifest must be **sealed**: bytes without `content_hash` or `manifest_hash` have no seal
    /// to verify, so a missing hash is an integrity failure, not a pass (otherwise deleting the seal
    /// would launder any edit). Check a manifest still being built with [`verify`](Self::verify).
    ///
    /// # Errors
    /// Everything [`from_json`](Self::from_json) refuses, then [`crate::Error::DuplicateBlockName`]
    /// for a block list naming a block twice (ADR-0063 §2a, SPEC §5; before any hash), then
    /// [`crate::Error::Integrity`] naming the first hash that is missing (`actual` is the recomputed
    /// value, `expected` reads `(absent)`) or doesn't match, or [`crate::Error::MissingDigest`] for an
    /// undigested block.
    pub fn from_json_verified(s: &str) -> crate::Result<Self> {
        let (m, object) = Self::parse(s)?; // parse() checks the version
        crate::block::check_unique_names(m.blocks.iter().map(|b| &b.name))?;
        check("id", &m.id, &m.recompute_id()?)?;
        let recorded = |h: &Option<String>| h.clone().unwrap_or_else(|| "(absent)".into());
        check(
            "content_hash",
            &recorded(&m.content_hash),
            &m.recompute_content_hash()?,
        )?;
        check(
            "manifest_hash",
            &recorded(&m.manifest_hash),
            &seal_of(&object)?,
        )?;
        Ok(m)
    }

    /// Verify this **struct's** three hashes against their recomputed values (and the spec version):
    /// the writer's check on a manifest it built. Any mismatch is a typed [`crate::Error::Integrity`]
    /// naming the field. A building (unsealed) manifest verifies its `id` only; a sealed one verifies
    /// all three.
    ///
    /// The seal is recomputed over this struct's JSON. For a manifest read from bytes, verify the bytes
    /// with [`from_json_verified`](Self::from_json_verified) instead: the parsed struct is a view, and
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

/// Compare an expected vs recomputed hash, raising a typed integrity error on mismatch.
fn check(what: &'static str, expected: &str, actual: &str) -> crate::Result<()> {
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
        m.manifest_hash = Some(m.compute_manifest_hash().unwrap());
        let mut v = serde_json::to_value(&m).unwrap();
        v["sources"] = json!([{"role": "r", "reference": "x", "weight": 0.5}]);
        v["actors"] = json!([]);
        let spliced = v.to_string();
        let err = Manifest::from_json_verified(&spliced).unwrap_err();
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
        v["manifest_hash"] = json!(seal_of(&v).unwrap());
        let ok = Manifest::from_json_verified(&v.to_string()).unwrap();
        assert_eq!(ok.newer_fields(), ["actors", "sources[0].weight"]);
    }
}
