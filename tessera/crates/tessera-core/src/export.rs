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

/// A **PHI-safe, opaque** identifier for one provenance edge — a `urn:tessera:source:<hash>` anchored
/// on the edge's `content_hash` (the source-bytes merkle root) when present, else a blake3 of the
/// reference. This is what goes into a *publishable* FAIR record: it proves + de-duplicates the source
/// without leaking the absolute filesystem path (which, for clinical DICOM, is PHI). #267
fn edge_urn(s: &crate::provenance::Source) -> String {
    let h = match &s.content_hash {
        Some(ch) => ch.trim_start_matches("blake3:").to_string(),
        None => crate::hash::digest(s.reference.as_bytes())
            .trim_start_matches("blake3:")
            .to_string(),
    };
    format!("urn:tessera:source:{h}")
}

/// Number of underlying source files an edge references (a DICOM series joins N slice paths with
/// commas). Used only for the human `description` — the paths themselves never leave the manifest.
fn source_count(s: &crate::provenance::Source) -> usize {
    s.reference
        .split(',')
        .filter(|x| !x.trim().is_empty())
        .count()
        .max(1)
}

/// One RO-Crate/JSON-LD entity per provenance edge — opaque `@id`, role in the description, and the
/// merkle `content_hash` as `sha256` when present. These are the `@graph` nodes `isBasedOn` points at.
fn source_entities(m: &Manifest) -> Vec<Value> {
    m.sources
        .iter()
        .map(|s| {
            let n = source_count(s);
            let ty = if n > 1 { "Dataset" } else { "File" };
            let mut e = json!({
                "@type": ty,
                "@id": edge_urn(s),
                "description": format!("{}: {n} source file(s)", s.role),
            });
            if let Some(h) = &s.content_hash {
                e["sha256"] = json!(h);
            }
            e
        })
        .collect()
}

/// Render a single product manifest as an RO-Crate `Dataset` entity at the given `@id`. Provenance
/// `sources` become `isBasedOn` references to opaque source entities (reusable for the single-product
/// `ro_crate` document with `id = "./"` and for member entities inside a collection's RO-Crate).
pub fn dataset_entity(m: &Manifest, id: &str) -> Value {
    // `isBasedOn` references opaque source entities (see `source_entities`) — never the raw path.
    let based_on: Vec<Value> = m
        .sources
        .iter()
        .map(|s| json!({ "@id": edge_urn(s) }))
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
    // The referenced source entities live in `@graph` (valid JSON-LD) — one opaque node per edge,
    // not a single mega-`@id` of comma-joined paths. #267
    let mut graph = vec![ro_crate_descriptor(), dataset_entity(m, "./")];
    graph.extend(source_entities(m));
    json!({
        "@context": "https://w3id.org/ro/crate/1.1/context",
        "@graph": graph
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
            // Opaque URN, not the raw path — a DataCite record is publishable metadata. #267
            json!({
                "relatedIdentifier": edge_urn(s),
                "relatedIdentifierType": "URN",
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
        // isBasedOn points at an opaque source URN in @graph — NOT the raw reference. #267
        let based = ds["isBasedOn"][0]["@id"].as_str().unwrap();
        assert!(based.starts_with("urn:tessera:source:"), "{based}");
        assert_ne!(based, "1.2.840.sop");
        assert!(c["@graph"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["@id"] == based && e["@type"] == "File"));
    }

    #[test]
    fn export_never_leaks_phi_paths_from_a_dicom_series() {
        // A DICOM-series `ingested_from` joins slice paths with commas; the paths carry PHI. #267
        let phi = "/data/DUPLET/PATIENT_SMITH_MRN123/CT.0001.IMA,\
                   /data/DUPLET/PATIENT_SMITH_MRN123/CT.0002.IMA";
        let mut b = ProductBuilder::new("recon", "s", "d", "2024-01-01T00:00:00Z");
        b.add_source(Source::new("ingested_from", phi).with_content_hash("blake3:abcd1234"));
        let m = b.seal().unwrap();
        for record in [ro_crate(&m), datacite(&m)] {
            let s = serde_json::to_string(&record).unwrap();
            assert!(!s.contains("PATIENT_SMITH"), "PHI leaked: {s}");
            assert!(!s.contains("/data/DUPLET"), "path leaked: {s}");
        }
        // The opaque URN is anchored on the edge's content_hash (integrity preserved).
        let ro = ro_crate(&m);
        let based = ro["@graph"][1]["isBasedOn"][0]["@id"].as_str().unwrap();
        assert_eq!(based, "urn:tessera:source:abcd1234");
        // The multi-file edge is a Dataset entity carrying the merkle sha256.
        let ent = ro["@graph"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["@id"] == based)
            .unwrap();
        assert_eq!(ent["@type"], "Dataset");
        assert_eq!(ent["sha256"], "blake3:abcd1234");
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
