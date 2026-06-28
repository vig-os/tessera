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
//! - [`manifest_hash`](Collection::manifest_hash) — `blake3` over canonical-JSON of the whole
//!   collection with `manifest_hash` itself omitted. The seal — transitively commits to every
//!   member's `manifest_hash` (which itself commits to that member's payload), so tampering with
//!   any member or with the catalog changes the collection seal.
//!
//! [`Role`] tags a member as `Raw` (acquisition data, irreplaceable) or `Derived` (regenerable
//! output). Storage layers map this to WORM retention mode — `Role::Raw` → Compliance (immutable),
//! `Role::Derived` → Governance (regenerable). The mapping helper lives next to the WORM module in
//! `tessera-io` so the core stays I/O-free; see `tessera_io::collection::retention_mode`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::manifest::{SUPPORTED_MAJOR, TESSERA_VERSION};

/// Raw acquisition data vs derived/regenerable product. Drives storage's WORM retention mapping
/// (raw → Compliance, derived → Governance) — see `tessera_io::collection::retention_mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Raw,
    Derived,
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
    /// Other members in this collection this one is derived from (their `reference`s). Forms an
    /// in-collection DAG that mirrors the manifest's `sources` edges, used by the RO-Crate
    /// projection to render `wasDerivedFrom`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub derived_from: Vec<String>,
}

impl CollectionMember {
    pub fn new(reference: impl Into<String>, manifest_hash: impl Into<String>, role: Role) -> Self {
        CollectionMember {
            reference: reference.into(),
            manifest_hash: manifest_hash.into(),
            role,
            derived_from: Vec::new(),
        }
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
    /// Optional study/grouping id (fd5 `study`) — ties this collection to other products of the
    /// same exam. Same field as on [`crate::manifest::Manifest`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub study: Option<String>,
    /// Member products, in declared order — order is significant (folded into `content_hash`).
    #[serde(default)]
    pub members: Vec<CollectionMember>,
    /// MMR root over the members' `manifest_hash`es; `Some` once sealed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    /// The seal: blake3 over canonical-JSON of this collection with `manifest_hash` omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_hash: Option<String>,
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
            study: None,
            members: Vec::new(),
            content_hash: None,
            manifest_hash: None,
        }
    }

    /// True once the collection has been sealed (carries a manifest hash).
    pub fn is_sealed(&self) -> bool {
        self.manifest_hash.is_some()
    }

    /// Pretty JSON, for humans / display. NOT the hashed form — see [`canonical_bytes`].
    pub fn to_json(&self) -> crate::Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Canonical (RFC 8785 JCS) bytes of this collection with `manifest_hash` excluded — the
    /// exact bytes [`manifest_hash`](Self::manifest_hash) is computed over.
    pub fn canonical_bytes(&self) -> crate::Result<Vec<u8>> {
        let mut bare = self.clone();
        bare.manifest_hash = None;
        crate::canonical::to_bytes(&bare)
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
        let leaves: Vec<String> = self
            .members
            .iter()
            .map(|m| m.manifest_hash.clone())
            .collect();
        crate::hash::merkle_root(&leaves)
    }

    /// Recompute the seal (`manifest_hash`) over the canonical bytes.
    pub fn compute_manifest_hash(&self) -> crate::Result<String> {
        Ok(crate::hash::digest(&self.canonical_bytes()?))
    }

    /// Parse a collection JSON, refusing one whose major spec version this reader can't handle.
    /// Does not verify hashes — use [`from_json_verified`](Self::from_json_verified) for that.
    pub fn from_json(s: &str) -> crate::Result<Self> {
        let c: Collection = serde_json::from_str(s)?;
        c.check_version()?;
        Ok(c)
    }

    /// Parse + version-check + full integrity [`verify`](Self::verify).
    pub fn from_json_verified(s: &str) -> crate::Result<Self> {
        let c = Self::from_json(s)?;
        c.verify()?;
        Ok(c)
    }

    /// Verify the three hashes against their recomputed values (and the spec version). Any
    /// mismatch is a typed [`crate::Error::Integrity`] naming the field. An unsealed collection
    /// verifies its `id` only; a sealed one verifies all three. Mirrors
    /// [`crate::manifest::Manifest::verify`].
    pub fn verify(&self) -> crate::Result<()> {
        self.check_version()?;
        check("id", &self.id, &self.recompute_id()?)?;
        if let Some(ch) = &self.content_hash {
            check("content_hash", ch, &self.recompute_content_hash())?;
        }
        if let Some(mh) = &self.manifest_hash {
            check("manifest_hash", mh, &self.compute_manifest_hash()?)?;
        }
        Ok(())
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

/// Compare an expected vs recomputed hash, raising a typed integrity error on mismatch. Same shape
/// as the manifest's check.
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

    /// Set the study/grouping id (ties this collection to the products of the same exam).
    pub fn with_study(&mut self, study: impl Into<String>) -> &mut Self {
        self.collection.study = Some(study.into());
        self
    }

    /// Append a member. Order is preserved — the seal folds members' `manifest_hash`es in this
    /// exact order, so two builders that add the same members in different orders seal to
    /// different `content_hash`es (intentional: order is part of the catalog's identity).
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
        let parsed = Collection::from_json_verified(&c.to_json().unwrap()).unwrap();
        assert_eq!(parsed.manifest_hash, c.manifest_hash);
        assert_eq!(parsed.content_hash, c.content_hash);
        assert_eq!(parsed.members.len(), 2);
        assert_eq!(parsed.members[1].derived_from, vec![raw.id]);
    }
}
