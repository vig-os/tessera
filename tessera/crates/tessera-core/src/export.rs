//! FAIR discovery exports (ROADMAP P6 / #209): render a product's manifest into standard
//! metadata so Tessera artifacts are **F**indable & **I**nteroperable in the wider ecosystem —
//! **RO-Crate** (JSON-LD, the research-data packaging standard) and **DataCite** (the DOI metadata
//! schema). Both are pure functions of the manifest (no I/O); they describe the product, they do not
//! alter it.

use serde_json::{json, Value};

use crate::manifest::Manifest;

/// The 4-digit year from an RFC-3339 timestamp (`2024-01-01T…` → 2024), or 0 if unparseable.
fn year_of(timestamp: &str) -> i64 {
    timestamp
        .get(0..4)
        .and_then(|y| y.parse::<i64>().ok())
        .unwrap_or(0)
}

/// The RO-Crate `ro-crate-metadata.json` descriptor entity — the standard header pointing at the
/// root dataset (`./`). Stable for any document this module produces.
pub fn ro_crate_descriptor() -> Value {
    json!({
        "@type": "CreativeWork",
        "@id": "ro-crate-metadata.json",
        "conformsTo": { "@id": "https://w3id.org/ro/crate/1.1" },
        "about": { "@id": "./" }
    })
}

/// Render a single product manifest as an RO-Crate `Dataset` entity at the given `@id`. Provenance
/// `sources` become `isBasedOn` references; the product schema is a keyword. Reusable for both the
/// single-product `ro_crate` document (where `id = "./"` marks the root) and for member entities
/// inside a collection's RO-Crate (where `id` is the member's reference — see
/// `tessera_io::collection::to_rocrate`).
pub fn dataset_entity(m: &Manifest, id: &str) -> Value {
    let based_on: Vec<Value> = m
        .sources
        .iter()
        .map(|s| json!({ "@id": s.reference }))
        .collect();
    json!({
        "@type": "Dataset",
        "@id": id,
        "identifier": m.id,
        "name": m.name,
        "description": m.description,
        "datePublished": m.timestamp,
        "keywords": [m.product],
        "isBasedOn": based_on
    })
}

/// Render an [RO-Crate 1.1](https://www.researchobject.org/ro-crate/) metadata document (the
/// `ro-crate-metadata.json` JSON-LD) describing this product as a `Dataset`. Provenance `sources`
/// become `isBasedOn` references; the product schema is a keyword.
pub fn ro_crate(m: &Manifest) -> Value {
    json!({
        "@context": "https://w3id.org/ro/crate/1.1/context",
        "@graph": [ro_crate_descriptor(), dataset_entity(m, "./")]
    })
}

/// Render a [DataCite](https://schema.datacite.org/) metadata record (the JSON:API `dois` shape) for
/// minting a DOI. `resourceType` carries the Tessera product schema; the content-addressed `id` is an
/// alternate identifier; provenance `sources` become `IsDerivedFrom` related identifiers.
pub fn datacite(m: &Manifest) -> Value {
    let related: Vec<Value> = m
        .sources
        .iter()
        .map(|s| {
            json!({
                "relatedIdentifier": s.reference,
                "relatedIdentifierType": "URL",
                "relationType": "IsDerivedFrom"
            })
        })
        .collect();
    // DataCite-mandatory `creators`: use the manifest's `creator` metadata field if recorded, else the
    // DataCite-sanctioned `(:unav)` ("value unavailable") placeholder — schema-valid + honest when the
    // product carries no author (a deposit overrides it with the depositing institution).
    let creators: Vec<Value> = match m.metadata.get("creator").and_then(|v| v.as_str()) {
        Some(name) => vec![json!({ "name": name })],
        None => vec![json!({ "name": "(:unav)", "nameType": "Organizational" })],
    };
    json!({
        "data": {
            "type": "dois",
            "attributes": {
                "creators": creators,
                "titles": [{ "title": m.name }],
                "descriptions": [{ "description": m.description, "descriptionType": "Abstract" }],
                "publisher": "Tessera",
                "publicationYear": year_of(&m.timestamp),
                "types": { "resourceTypeGeneral": "Dataset", "resourceType": m.product },
                "identifiers": [{ "identifier": m.id, "identifierType": "blake3" }],
                "relatedIdentifiers": related
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provenance::Source;
    use crate::ProductBuilder;

    fn product() -> Manifest {
        let mut b = ProductBuilder::new(
            "recon",
            "DP06-ct",
            "int16 CT volume",
            "2024-03-15T00:00:00Z",
        );
        b.add_source(Source::new("ingested_from", "1.2.840.sop"));
        b.seal().unwrap()
    }

    #[test]
    fn ro_crate_describes_the_dataset() {
        let m = product();
        let c = ro_crate(&m);
        assert_eq!(c["@context"], "https://w3id.org/ro/crate/1.1/context");
        let ds = &c["@graph"][1];
        assert_eq!(ds["@type"], "Dataset");
        assert_eq!(ds["identifier"], m.id);
        assert_eq!(ds["name"], "DP06-ct");
        assert_eq!(ds["datePublished"], "2024-03-15T00:00:00Z");
        assert_eq!(ds["keywords"][0], "recon");
        assert_eq!(ds["isBasedOn"][0]["@id"], "1.2.840.sop");
    }

    #[test]
    fn datacite_record_has_required_fields() {
        let m = product();
        let d = datacite(&m);
        let a = &d["data"]["attributes"];
        assert_eq!(a["titles"][0]["title"], "DP06-ct");
        assert_eq!(a["publicationYear"], 2024);
        assert_eq!(a["types"]["resourceTypeGeneral"], "Dataset");
        assert_eq!(a["types"]["resourceType"], "recon");
        assert_eq!(a["identifiers"][0]["identifier"], m.id);
        assert_eq!(a["relatedIdentifiers"][0]["relationType"], "IsDerivedFrom");
    }

    #[test]
    fn datacite_has_all_mandatory_fields_for_doi_minting() {
        // DataCite 4.x requires Identifier, Creator, Title, Publisher, PublicationYear, ResourceType —
        // all must be present + non-empty for InvenioRDM/DataCite to mint a DOI.
        let a = datacite(&product())["data"]["attributes"].clone();
        assert!(a["identifiers"].as_array().is_some_and(|v| !v.is_empty()));
        assert!(a["creators"].as_array().is_some_and(|v| !v.is_empty()));
        assert!(a["titles"].as_array().is_some_and(|v| !v.is_empty()));
        assert!(a["publisher"].as_str().is_some_and(|s| !s.is_empty()));
        assert!(a["publicationYear"].as_i64().is_some_and(|y| y > 0));
        assert!(a["types"]["resourceTypeGeneral"]
            .as_str()
            .is_some_and(|s| !s.is_empty()));
        // no recorded author → the DataCite `(:unav)` unavailable convention (schema-valid + honest).
        assert_eq!(a["creators"][0]["name"], "(:unav)");
    }

    #[test]
    fn datacite_creator_comes_from_manifest_metadata_when_present() {
        let mut b = ProductBuilder::new("recon", "n", "d", "2024-01-01T00:00:00Z");
        b.with_field("creator", serde_json::json!("Dr. A. Researcher"));
        let m = b.seal().unwrap();
        assert_eq!(
            datacite(&m)["data"]["attributes"]["creators"][0]["name"],
            "Dr. A. Researcher"
        );
    }

    #[test]
    fn year_parsing_is_robust() {
        assert_eq!(year_of("2024-03-15T00:00:00Z"), 2024);
        assert_eq!(year_of("garbage"), 0);
    }
}
