//! Three projections of a content-addressed [`Collection`] (ADR-0033, #223).
//!
//! The collection lives in `tessera-core` (it's part of the format). The projections live here in
//! `tessera-io` because they are *renderings into other ecosystems' wire formats* — same as
//! `oci.rs` and the `.tsra` container itself. Three projections of one logical collection:
//!
//! 1. **RO-Crate** ([`to_rocrate`]) — the FAIR discovery export: `ro-crate-metadata.json` JSON-LD
//!    with the collection as a root `Dataset` and each member as a `Dataset` with
//!    `hasPart`/`isPartOf` and `wasDerivedFrom` for in-collection derivation edges. Reuses
//!    [`tessera_core::export::dataset_entity`] so member entities match the standalone single-product
//!    export byte-for-byte (modulo `@id` + collection-specific properties).
//! 2. **OCI image index** ([`to_oci_index`]) — a registry-native manifest-of-manifests. Each entry
//!    references a member's pre-pushed OCI artifact manifest by sha256 digest, carrying the member
//!    Tessera identity in annotations. Reuses the media-type constants from [`crate::oci`].
//! 3. **S3/MinIO prefix layout** ([`prefix_layout`]) — a pure layout helper returning
//!    `(object_key, member_reference)` pairs (`<prefix>/<id>.tsra` + `<prefix>/collection.json`).
//!    The prefix of N independently range-readable `.tsra` objects IS the collection on S3.
//!
//! All three enumerate the same member set in the same declared order — a single source of truth
//! for what's in the collection, three encodings of how to find it.
//!
//! ## Raw → Derived → WORM
//!
//! [`retention_mode`] maps [`Role`] → [`RetentionMode`]: raw acquisition → Compliance (no bypass,
//! ever), derived → Governance (an authorized bypass for regenerable outputs). The storage layer
//! (S3 Object-Lock or [`crate::worm`]) reads this when placing the hold; the collection itself
//! never enforces WORM (a `Collection` is metadata, the bytes it points to are what's held).

use std::collections::BTreeMap;

use serde_json::{json, Value};

use tessera_core::collection::{Collection, Role};
use tessera_core::export::{dataset_entity, ro_crate_descriptor};
use tessera_core::manifest::Manifest;
use tessera_core::{Error, Result};

use crate::oci::{ARTIFACT_TYPE, MANIFEST_MEDIA_TYPE};
use crate::worm::RetentionMode;

/// OCI 1.1 image-index media type — the manifest-of-manifests wrapper that registries already
/// understand. A Tessera collection projects naturally onto this.
pub const OCI_INDEX_MEDIA_TYPE: &str = "application/vnd.oci.image.index.v1+json";

/// Map a member's [`Role`] to the [`RetentionMode`] its bytes should be held under (#209/ADR-0033):
/// raw data is irreplaceable → Compliance (no bypass, ever); derived output is regenerable →
/// Governance (an audited bypass for legal-hold corrections). This is the format-side contract
/// the storage layer consumes; the collection itself stores no policy.
pub fn retention_mode(role: Role) -> RetentionMode {
    match role {
        Role::Raw => RetentionMode::Compliance,
        Role::Derived => RetentionMode::Governance,
    }
}

fn role_str(role: Role) -> &'static str {
    match role {
        Role::Raw => "raw",
        Role::Derived => "derived",
    }
}

/// Render the collection as an RO-Crate 1.1 document: the collection is the root `Dataset` (at
/// `@id: "./"`); each member is a `Dataset` entity at `@id: <member.reference>` with `isPartOf` →
/// the collection and `wasDerivedFrom`/`isBasedOn` → its in-collection parents. Members whose
/// manifest is provided in `members` get the full single-product entity (reusing
/// [`tessera_core::export::dataset_entity`] — same scheme as the standalone export); members with
/// no manifest get a minimal entity that still identifies + parts.
pub fn to_rocrate(c: &Collection, members: &[(&str, &Manifest)]) -> Value {
    let resolved: BTreeMap<&str, &Manifest> = members.iter().map(|(r, m)| (*r, *m)).collect();

    let mut graph: Vec<Value> = Vec::with_capacity(2 + c.members.len());
    graph.push(ro_crate_descriptor());

    // Root entity = the collection. Members are hasPart references; order preserved.
    let has_part: Vec<Value> = c
        .members
        .iter()
        .map(|m| json!({ "@id": m.reference }))
        .collect();
    graph.push(json!({
        "@type": "Dataset",
        "@id": "./",
        "identifier": c.id,
        "name": c.name,
        "description": c.description,
        "datePublished": c.timestamp,
        "keywords": ["collection"],
        "hasPart": has_part,
    }));

    // One Dataset entity per member, in declared order.
    for member in &c.members {
        let mut entity = match resolved.get(member.reference.as_str()) {
            Some(mf) => dataset_entity(mf, &member.reference),
            None => json!({
                "@type": "Dataset",
                "@id": member.reference,
                "identifier": member.reference,
            }),
        };
        if let Some(obj) = entity.as_object_mut() {
            obj.insert("isPartOf".into(), json!({ "@id": c.id }));
            if !member.derived_from.is_empty() {
                let derived: Vec<Value> = member
                    .derived_from
                    .iter()
                    .map(|r| json!({ "@id": r }))
                    .collect();
                // Both predicates: PROV-O `wasDerivedFrom` (graph semantics) + schema.org
                // `isBasedOn` (RO-Crate convention). One source list, two encodings.
                obj.insert("wasDerivedFrom".into(), Value::Array(derived.clone()));
                obj.insert("isBasedOn".into(), Value::Array(derived));
            }
            obj.insert(
                "additionalProperty".into(),
                json!([
                    { "@type": "PropertyValue", "name": "role", "value": role_str(member.role) },
                    { "@type": "PropertyValue", "name": "manifest_hash", "value": member.manifest_hash },
                ]),
            );
        }
        graph.push(entity);
    }

    json!({
        "@context": "https://w3id.org/ro/crate/1.1/context",
        "@graph": graph,
    })
}

/// Render the collection as an OCI 1.1 image index — `mediaType` [`OCI_INDEX_MEDIA_TYPE`], a
/// `manifests[]` referencing each member's previously-pushed OCI artifact manifest by sha256
/// digest + size. Tessera identity (member id, `manifest_hash`, role) rides in annotations so the
/// collection is fully discoverable from the registry without unpacking. Reuses
/// [`crate::oci::ARTIFACT_TYPE`] / [`crate::oci::MANIFEST_MEDIA_TYPE`] — no media-type duplication.
///
/// `members` is `(member_reference, oci_artifact_digest, size)`; the digest is whatever the push
/// side recorded (e.g. from [`crate::oci::sha256_digest`] over the member's artifact manifest).
/// Order follows the collection's declared member order — the catalog's identity.
pub fn to_oci_index(c: &Collection, members: &[(&str, &str, u64)]) -> Result<Value> {
    let by_ref: BTreeMap<&str, (&str, u64)> =
        members.iter().map(|(r, d, s)| (*r, (*d, *s))).collect();

    // Every member MUST have a pushed-artifact descriptor — an index entry with an empty digest is
    // syntactically valid but registry-invalid, and the failure would only surface at push time.
    // Error loudly at construction instead of silently emitting a broken index.
    let manifests: Vec<Value> = c
        .members
        .iter()
        .map(|m| {
            let (digest, size) = by_ref.get(m.reference.as_str()).copied().ok_or_else(|| {
                Error::Container(format!(
                    "to_oci_index: no OCI artifact descriptor supplied for member '{}'",
                    m.reference
                ))
            })?;
            Ok(json!({
                "mediaType": MANIFEST_MEDIA_TYPE,
                "artifactType": ARTIFACT_TYPE,
                "digest": digest,
                "size": size,
                "annotations": {
                    "org.opencontainers.image.ref.name": m.reference,
                    "vnd.tessera.id": m.reference,
                    "vnd.tessera.manifest_hash": m.manifest_hash,
                    "vnd.tessera.role": role_str(m.role),
                },
            }))
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(json!({
        "schemaVersion": 2,
        "mediaType": OCI_INDEX_MEDIA_TYPE,
        "artifactType": ARTIFACT_TYPE,
        "manifests": manifests,
        "annotations": {
            "vnd.tessera.collection.id": c.id,
            "vnd.tessera.collection.manifest_hash": c.manifest_hash.clone().unwrap_or_default(),
            "org.opencontainers.image.title": c.name,
        },
    }))
}

/// One entry in the [`prefix_layout`] projection. Member entries carry `kind = Member` and the
/// member's `reference`; the catalog metadata entry carries `kind = Catalog` and the collection id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutKind {
    Member,
    Catalog,
}

/// Project the collection onto an S3/MinIO key prefix — pure layout, no I/O. Returns
/// `(object_key, member_reference)` pairs in declared member order, then a final
/// `("<prefix>/collection.json", <collection_id>)` for the catalog descriptor. The prefix of N
/// independently range-readable `.tsra` objects under this layout IS the collection on S3 (the
/// third projection): a reader range-reads `collection.json` once, then range-reads each member's
/// manifest + the blocks it actually needs — no whole-collection download.
pub fn prefix_layout(c: &Collection, prefix: &str) -> Vec<(String, String)> {
    let prefix = prefix.trim_end_matches('/');
    let mut out: Vec<(String, String)> = c
        .members
        .iter()
        .map(|m| {
            (
                format!("{prefix}/{}.tsra", m.reference),
                m.reference.clone(),
            )
        })
        .collect();
    out.push((format!("{prefix}/collection.json"), c.id.clone()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tessera_core::collection::CollectionBuilder;
    use tessera_core::provenance::Source;
    use tessera_core::ProductBuilder;

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

    fn build_collection() -> (Manifest, Manifest, Collection) {
        let raw = raw_listmode();
        let recon = derived_recon(&raw);
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
        let coll = cb.seal().unwrap();
        (raw, recon, coll)
    }

    #[test]
    fn three_projections_enumerate_the_same_members_in_collection_order() {
        let (raw, recon, coll) = build_collection();

        // RO-Crate: members appear after the descriptor (graph[0]) and the root collection entity
        // (graph[1]), in declared order at graph[2..].
        let rc = to_rocrate(&coll, &[(&raw.id, &raw), (&recon.id, &recon)]);
        let graph = rc["@graph"].as_array().unwrap();
        let rc_refs: Vec<&str> = graph[2..]
            .iter()
            .map(|e| e["@id"].as_str().unwrap())
            .collect();

        // OCI index: each entry's annotation carries the Tessera member id.
        let oci = to_oci_index(
            &coll,
            &[(&raw.id, "sha256:aaa", 100), (&recon.id, "sha256:bbb", 200)],
        )
        .unwrap();
        let oci_refs: Vec<&str> = oci["manifests"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| {
                m["annotations"]["org.opencontainers.image.ref.name"]
                    .as_str()
                    .unwrap()
            })
            .collect();

        // Prefix layout: take only member entries (collection.json is the tail catalog entry).
        let layout = prefix_layout(&coll, "s3://bucket/study/");
        let layout_refs: Vec<&str> = layout
            .iter()
            .take(coll.members.len())
            .map(|(_, r)| r.as_str())
            .collect();

        let expected: Vec<&str> = coll.members.iter().map(|m| m.reference.as_str()).collect();
        assert_eq!(rc_refs, expected);
        assert_eq!(oci_refs, expected);
        assert_eq!(layout_refs, expected);
        // and the catalog descriptor closes the prefix layout.
        assert_eq!(
            layout.last().unwrap().0,
            "s3://bucket/study/collection.json"
        );
        assert_eq!(layout.last().unwrap().1, coll.id);
    }

    #[test]
    fn rocrate_records_derived_from_edges_for_in_collection_derivations() {
        let (raw, recon, coll) = build_collection();
        let rc = to_rocrate(&coll, &[(&raw.id, &raw), (&recon.id, &recon)]);
        let graph = rc["@graph"].as_array().unwrap();

        // descriptor + root + 2 member entities.
        assert_eq!(graph.len(), 4);
        assert_eq!(graph[0]["@type"], "CreativeWork");
        assert_eq!(graph[1]["@id"], "./");
        assert_eq!(graph[1]["identifier"], coll.id);
        assert_eq!(graph[1]["hasPart"][0]["@id"], raw.id);
        assert_eq!(graph[1]["hasPart"][1]["@id"], recon.id);

        // Raw member: no derived_from edge.
        let raw_e = &graph[2];
        assert_eq!(raw_e["@id"], raw.id);
        assert_eq!(raw_e["isPartOf"]["@id"], coll.id);
        assert!(raw_e.get("wasDerivedFrom").is_none());

        // Derived member: references the raw member by both PROV `wasDerivedFrom` and RO-Crate's
        // `isBasedOn` (one source list, two predicates).
        let der_e = &graph[3];
        assert_eq!(der_e["@id"], recon.id);
        assert_eq!(der_e["wasDerivedFrom"][0]["@id"], raw.id);
        assert_eq!(der_e["isBasedOn"][0]["@id"], raw.id);
        // role + manifest_hash carried as additionalProperty.
        let props = der_e["additionalProperty"].as_array().unwrap();
        assert_eq!(props[0]["name"], "role");
        assert_eq!(props[0]["value"], "derived");
        assert_eq!(props[1]["name"], "manifest_hash");
        assert_eq!(props[1]["value"], recon.manifest_hash.clone().unwrap());
    }

    #[test]
    fn oci_index_uses_image_index_media_type_and_carries_member_digests() {
        let (raw, recon, coll) = build_collection();
        let oci = to_oci_index(
            &coll,
            &[(&raw.id, "sha256:aaa", 100), (&recon.id, "sha256:bbb", 200)],
        )
        .unwrap();
        assert_eq!(oci["schemaVersion"], 2);
        assert_eq!(oci["mediaType"], OCI_INDEX_MEDIA_TYPE);
        assert_eq!(oci["artifactType"], ARTIFACT_TYPE);
        // collection identity rides in top-level annotations.
        assert_eq!(oci["annotations"]["vnd.tessera.collection.id"], coll.id);
        assert_eq!(
            oci["annotations"]["vnd.tessera.collection.manifest_hash"],
            coll.manifest_hash.clone().unwrap()
        );
        // each member entry carries its OCI descriptor + Tessera annotations.
        let entries = oci["manifests"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["mediaType"], MANIFEST_MEDIA_TYPE);
        assert_eq!(entries[0]["digest"], "sha256:aaa");
        assert_eq!(entries[0]["size"], 100);
        assert_eq!(entries[0]["annotations"]["vnd.tessera.role"], "raw");
        assert_eq!(entries[1]["digest"], "sha256:bbb");
        assert_eq!(entries[1]["annotations"]["vnd.tessera.role"], "derived");
    }

    #[test]
    fn prefix_layout_trims_trailing_slash_and_closes_with_catalog_object() {
        let (_, _, coll) = build_collection();
        // trailing slash is normalised so a caller can pass "s3://b/p" or "s3://b/p/" the same way.
        let with_slash = prefix_layout(&coll, "s3://bucket/study/");
        let without = prefix_layout(&coll, "s3://bucket/study");
        assert_eq!(with_slash, without);
        // every member key has the .tsra extension under the prefix.
        for (key, _) in with_slash.iter().take(coll.members.len()) {
            assert!(key.starts_with("s3://bucket/study/"));
            assert!(key.ends_with(".tsra"));
        }
    }

    #[test]
    fn retention_mode_maps_raw_to_compliance_and_derived_to_governance() {
        // raw acquisition is irreplaceable → Compliance (no bypass); derived is regenerable →
        // Governance (audited bypass). This is the ADR-0033 raw/derived boundary.
        assert_eq!(retention_mode(Role::Raw), RetentionMode::Compliance);
        assert_eq!(retention_mode(Role::Derived), RetentionMode::Governance);
    }
}
