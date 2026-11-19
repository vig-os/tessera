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
/// edge `content_hash` as a **labelled** `identifier` when present. These are the `@graph` nodes
/// `isBasedOn` points at.
///
/// # Why not `sha256` (#525)
///
/// This used to emit the value under schema.org's `sha256`, which is defined as "The SHA-2 SHA256 hash
/// of the content of the item" and has domain `MediaObject`. Three things were wrong at once: the
/// algorithm is BLAKE3, not SHA-256; the value kept its `blake3:` prefix so it was not even a bare hex
/// digest; and a multi-file edge is typed `Dataset`, which is not a `MediaObject` at all. A consumer
/// validating `sha256` reports corruption on an intact crate.
///
/// Relabelling it `blake3` would NOT have been the fix either: the value's *construction* varies by edge
/// role (see [`edge_hash_label`]), and for most roles it is not a digest any checksum tool reproduces. The
/// label therefore names the construction rather than just a hash function.
///
/// RO-Crate 1.1/1.2 prescribe no digest property for data entities (their only checksum text is the
/// BagIt profile's SHA-512 manifest, a separate mechanism), so the shape follows schema.org:
/// `identifier` has domain `Thing` — so it is legal on both `File` and `Dataset` — and accepts a
/// `PropertyValue`, which is the documented way to carry a typed identifier. `additionalProperty` was
/// the other candidate and is wrong here: its domain is `Offer`/`Place`/`Product`/…, not `Dataset`.
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
            if s.content_hash.is_some() {
                // A REFERENCE only. RO-Crate JSON-LD is flattened: "all described entities must be
                // JSON objects as direct children of the `@graph` element rather than being nested
                // under another object or array", and "properties referencing entities must use a
                // JSON object with `@id` as the only key" (1.1 JSON-LD appendix). The PropertyValue
                // itself is appended to `@graph` below.
                e["identifier"] = json!({ "@id": edge_hash_id(s) });
            }
            e
        })
        .collect()
}

/// `@id` of the PropertyValue entity describing one edge's pinned hash.
///
/// Keyed on **role and hash together**, not the hash alone: two edges can pin the same hash under
/// different roles, and the label differs per role, so a hash-only id would give one `@id` two
/// different descriptions.
fn edge_hash_id(s: &crate::provenance::Source) -> String {
    // Derived from a digest of the pair, NOT from the strings themselves. `role` is free-form, so
    // interpolating it would let a space, `#` or `%` produce an invalid IRI fragment — and a `-`
    // separator is ambiguous: role `a-b` + hash `c` and role `a` + hash `b-c` would build the same id,
    // and since entities are de-duplicated by id one of the two labels would silently vanish. A
    // NUL-separated digest cannot collide and is always IRI-safe.
    let mut key = s.role.clone();
    key.push('\0');
    key.push_str(s.content_hash.as_deref().unwrap_or_default());
    let d = crate::hash::digest(key.as_bytes());
    format!("#edge-hash-{}", &d.trim_start_matches("blake3:")[..16])
}

/// **What an edge's `content_hash` actually is — per role.** This is the whole lesson of #525 applied
/// one level down: a single hard-coded label was false for most roles.
///
/// `Source.content_hash` is not one construction. Verified against the producers:
///
/// | role | what the value is | where |
/// | --- | --- | --- |
/// | `derived_from`, `supersedes`, `snapshot_of` | the parent product's `manifest_hash` — BLAKE3 over its canonical-JSON manifest | `Manifest::compute_manifest_hash` |
/// | `ingested_from` | BLAKE3 **Merkle root over the source files'** digests, one leaf per file | `tessera_ingest::provenance::source_digest` |
/// | `ingested_via_spec` | BLAKE3 over the canonical JSON of the parsed ingest spec | `tessera_ingest::spec::spec_hash` |
/// | anything else | a hash the producer of that role chose | — |
///
/// So *none* of them is the product's own block Merkle root, and none is reproducible with a checksum
/// tool over a file.
///
/// `role` is a free-form `String` (domain edges like `emission_data` / `calibration` are expected), so
/// the compiler cannot force this match to be exhaustive — there is no enum to match on, and making one
/// would change a serialised format field. The fallback arm is therefore written to be **true of any
/// role**, and `every_known_role_gets_a_truthful_label` pins the known set so a new in-tree role shows
/// up as a test failure rather than inheriting someone else's description.
fn edge_hash_label(role: &str, value: &str) -> (&'static str, String) {
    match role {
        "derived_from" | "supersedes" | "snapshot_of" => (
            "Parent product seal (manifest_hash)",
            "BLAKE3 over the parent product's canonical-JSON manifest (its `manifest_hash`), which is \
             the seal this edge commits to. Not a digest of any file's bytes."
                .to_string(),
        ),
        // TWO constructions share this one role, so the label has to be true of both. The blob tier
        // reuses its block digest, which is plain `blake3(file bytes)` and IS reproducible with
        // `b3sum` (`tessera_io::blob` -> `ingested_from_digest`). Every other producer hands over a
        // Merkle root over the source files' digests. They are genuinely different values — a
        // one-element Merkle root is `leaf_hash(d)`, which carries a `0x00` domain prefix and so is
        // NOT `d` — and nothing stored on the edge separates them: `source_count` is 1 for a blob
        // *and* 1 for a single-file generic ingest, which still goes through the Merkle path. So one
        // label, true either way, rather than a guess that is wrong half the time.
        "ingested_from" => (
            "Source hash recorded at ingest",
            "A BLAKE3 hash of the source as Tessera recorded it: for a single file preserved by the \
             blob tier, `blake3` of the file's bytes, which `b3sum` reproduces; for every other \
             source, a Merkle root over the source files' digests, which it does not. Either way it \
             is not the product's own block Merkle root."
                .to_string(),
        ),
        "ingested_via_spec" => (
            "Ingest spec hash",
            "BLAKE3 over the canonical JSON of the parsed ingest spec that produced this product."
                .to_string(),
        ),
        // No assumption about the algorithm: `Source::with_content_hash` takes any string, so a domain
        // producer can pin `sha256:…` or anything else. The algorithm is read off the value's own
        // prefix by the caller and passed in, rather than asserted here.
        _ => (
            "Upstream hash pinned by this provenance edge",
            format!(
                "{} recorded for the artifact this `{role}` edge points at. How it was constructed is \
                 decided by whatever produced that role, so no checksum tool is expected to reproduce \
                 it.",
                algorithm_phrase(value)
            ),
        ),
    }
}

/// Name the algorithm from the value's own `<alg>:` prefix rather than assuming one.
///
/// `Source::with_content_hash` takes **any** string: every producer in this workspace writes `blake3:…`,
/// but a domain edge is free to pin `sha256:…` or a vendor checksum, and the whole point of #525 is not to
/// claim an algorithm the value does not have. An unprefixed value gets no claim at all.
fn algorithm_phrase(value: &str) -> &'static str {
    match value.split_once(':').map(|(alg, _)| alg) {
        Some("blake3") => "A BLAKE3 hash",
        Some("sha256") => "A SHA-256 hash",
        Some("sha512") => "A SHA-512 hash",
        Some(_) => "A hash, in the algorithm its own prefix names,",
        None => "A hash of unstated algorithm",
    }
}

/// The PropertyValue entities for the edges that pin a hash — `@graph` children, de-duplicated by
/// `@id`, referenced from each edge entity's `identifier`.
///
/// No `url`: RO-Crate 1.2 says a PropertyValue identifier "SHOULD have a `url` if the identifier is
/// Web-resolvable", and a content hash is not. `value` is present, which 1.2 requires. `propertyID` is
/// a short code rather than the registry URI 1.2's example shows, because no registry defines this
/// characteristic and a URI we do not serve would be worse than a plain code.
fn edge_hash_entities(m: &Manifest) -> Vec<Value> {
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for s in &m.sources {
        let Some(h) = &s.content_hash else { continue };
        let id = edge_hash_id(s);
        if !seen.insert(id.clone()) {
            continue;
        }
        let (name, description) = edge_hash_label(&s.role, h);
        out.push(json!({
            "@type": "PropertyValue",
            "@id": id,
            "propertyID": "tessera-provenance-edge-hash",
            "name": name,
            "description": description,
            "value": h,
        }));
    }
    out
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
    // Flattened form: the edge-hash PropertyValues are `@graph` children, not nested values.
    graph.extend(edge_hash_entities(m));
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
        // The multi-file edge is a Dataset entity carrying the merkle root as a LABELLED identifier
        // (#525 — it used to be emitted as `sha256`, which it is not).
        let ent = ro["@graph"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["@id"] == based)
            .unwrap();
        assert_eq!(ent["@type"], "Dataset");
        // A flattened reference; the PropertyValue itself is its own `@graph` child (#525).
        let hid = ent["identifier"]["@id"].as_str().unwrap();
        let pv = ro["@graph"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["@id"] == hid)
            .expect("the edge-hash PropertyValue must be in @graph");
        assert_eq!(pv["@type"], "PropertyValue");
        assert_eq!(pv["value"], "blake3:abcd1234");
    }

    /// **An RO-Crate must not claim a digest it did not compute** (#525).
    ///
    /// `content_hash` is a BLAKE3 **Merkle root over the product's ordered block digests**, and it was
    /// emitted under schema.org's `sha256` — defined as "The SHA-2 SHA256 hash of the content of the
    /// item". Two false claims in one field: the wrong algorithm, and (because the value kept its
    /// `blake3:` prefix) not even a bare hex digest. A consumer validating `sha256` reports corruption
    /// on a perfectly intact crate.
    ///
    /// Nor is relabelling it `blake3` the fix: what the value IS varies by edge role, and for most roles
    /// no checksum tool reproduces it. The label has to name the construction, not just an algorithm.
    #[test]
    fn the_rocrate_never_claims_a_digest_it_did_not_compute() {
        let mut b = ProductBuilder::new("recon", "s", "d", "2024-01-01T00:00:00Z");
        b.add_source(Source::new("ingested_from", "/a,/b").with_content_hash("blake3:abcd1234"));
        let m = b.seal().unwrap();
        let ro = ro_crate(&m);
        let text = serde_json::to_string(&ro).unwrap();

        // We compute no SHA-256 anywhere, so the document must not mention one — in any casing, and
        // not as a bare key that a validator would pick up.
        assert!(
            !text.to_lowercase().contains("sha256"),
            "the crate claims a sha256 it never computed: {text}"
        );

        // The digest still has to be THERE — fixing a mislabel by dropping the data would be worse.
        let based = ro["@graph"][1]["isBasedOn"][0]["@id"].as_str().unwrap();
        let ent = ro["@graph"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["@id"] == based)
            .unwrap();
        let hid = ent["identifier"]["@id"]
            .as_str()
            .expect("identifier must be an @id reference, not a nested object");
        let pv = ro["@graph"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["@id"] == hid)
            .expect("the referenced PropertyValue must exist as a @graph child");
        assert_eq!(pv["@type"], "PropertyValue", "got {pv}");
        assert_eq!(pv["value"], "blake3:abcd1234", "got {pv}");
        assert!(pv["propertyID"].is_string(), "needs a propertyID: {pv}");
        // This edge is `ingested_from`, so the label must describe THAT construction — a Merkle root
        // over the source files — and must not claim the product's own block digests.
        let label = format!("{} {}", pv["name"], pv["description"]).to_lowercase();
        assert!(label.contains("merkle"), "{label}");
        assert!(label.contains("source files"), "{label}");
    }

    /// **Every role gets a label that is true of THAT role.**
    ///
    /// One label for all roles would be false for most of them: a `derived_from` edge pins the parent's
    /// `manifest_hash`, which is a digest of canonical JSON and not a Merkle root at all. `role` is a
    /// free-form `String`, so this cannot be a compiler-exhaustive match; the pin is here instead.
    ///
    /// This checks the label's SHAPE for each role. That the label matches what each real producer
    /// actually stores is checked against the producers themselves, in
    /// `tessera-ingest/tests/edge_hash_labels.rs` — a role list cannot catch a producer whose
    /// construction differs from what the role name suggests.
    #[test]
    fn every_known_role_gets_a_truthful_label() {
        // role, substrings the label MUST contain, substrings it must NOT contain.
        //
        // The exclusions are only words that would be a *claim* in these labels. Note what is NOT
        // excluded: "block". The `ingested_from` description legitimately says "not the product's own
        // block merkle root" — a disclaimer — and a substring ban cannot tell a claim from a
        // disclaimer, so the exclusions are kept to words whose mere presence is wrong for that role.
        let cases: &[(&str, &[&str], &[&str])] = &[
            ("derived_from", &["manifest"], &["merkle"]),
            ("supersedes", &["manifest"], &["merkle"]),
            ("snapshot_of", &["manifest"], &["merkle"]),
            // Two producers share this role with different constructions, so the label names both.
            (
                "ingested_from",
                &["merkle", "source files", "b3sum"],
                &["manifest"],
            ),
            ("ingested_via_spec", &["spec"], &["merkle"]),
            // A domain role the producer invents: the label must stay true without guessing.
            ("emission_data", &["emission_data"], &["merkle", "manifest"]),
        ];
        for (role, must, must_not) in cases {
            let (name, desc) = edge_hash_label(role, "blake3:abcd");
            let label = format!("{name} {desc}").to_lowercase();
            // Never name a command that does not check this value: `tessera verify` verifies the
            // product, not an edge's pinned hash.
            assert!(
                !label.contains("tessera verify"),
                "role {role}: `tessera verify` does not check an edge hash: {label}"
            );
            assert!(label.contains("blake3"), "role {role}: {label}");
            for w in *must {
                assert!(label.contains(w), "role {role} must mention {w}: {label}");
            }
            for w in *must_not {
                assert!(
                    !label.contains(w),
                    "role {role} must NOT claim {w}: {label}"
                );
            }
        }

        // **The algorithm is never assumed.** `with_content_hash` takes any string, so a domain edge can
        // pin a non-BLAKE3 value, and the label must not claim otherwise. Every producer in this
        // workspace writes `blake3:`, which is exactly why this would rot unnoticed without a test.
        let (_, sha) = edge_hash_label("vendor_checksum", "sha256:deadbeef");
        assert!(sha.contains("SHA-256"), "{sha}");
        assert!(!sha.to_lowercase().contains("blake3"), "{sha}");
        let (_, bare) = edge_hash_label("vendor_checksum", "deadbeef");
        assert!(
            bare.contains("unstated"),
            "an unprefixed value claims nothing: {bare}"
        );
        assert!(!bare.to_lowercase().contains("blake3"), "{bare}");
    }

    /// **RO-Crate JSON-LD is flattened** — 1.1's JSON-LD appendix: "all described entities must be JSON
    /// objects as direct children of the `@graph` element rather than being nested under another object
    /// or array", and "properties referencing entities must use a JSON object with `@id` as the only
    /// key". The first fix nested a PropertyValue inside an edge entity, which breaks both.
    ///
    /// Asserted structurally over the whole document rather than on the one property that was wrong, so
    /// a future nested entity anywhere fails here.
    #[test]
    fn the_rocrate_graph_is_flattened_and_references_are_id_only() {
        let mut b = ProductBuilder::new("recon", "s", "d", "2024-01-01T00:00:00Z");
        b.add_source(Source::new("ingested_from", "/a,/b").with_content_hash("blake3:abcd1234"));
        b.add_source(Source::new("derived_from", "parent").with_content_hash("blake3:beef"));
        let m = b.seal().unwrap();
        let ro = ro_crate(&m);

        fn check(v: &Value, path: &str) {
            match v {
                Value::Object(o) => {
                    // A nested object is legal ONLY as a pure {"@id": ...} reference.
                    let keys: Vec<&str> = o.keys().map(String::as_str).collect();
                    assert_eq!(
                        keys,
                        vec!["@id"],
                        "nested object at {path} must be an @id-only reference, got {keys:?}"
                    );
                }
                Value::Array(a) => {
                    for (i, x) in a.iter().enumerate() {
                        check(x, &format!("{path}[{i}]"));
                    }
                }
                _ => {}
            }
        }
        for (i, entity) in ro["@graph"].as_array().unwrap().iter().enumerate() {
            let o = entity.as_object().expect("every @graph child is an object");
            assert!(o.contains_key("@id"), "entity {i} has no @id: {entity}");
            for (k, v) in o {
                if k == "@id" || k == "@type" {
                    continue;
                }
                check(v, &format!("@graph[{i}].{k}"));
            }
        }

        // And each edge's identifier resolves to a PropertyValue that really is in @graph.
        let ids: Vec<&str> = ro["@graph"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|e| e["@id"].as_str())
            .collect();
        let mut pvs = 0;
        for e in ro["@graph"].as_array().unwrap() {
            if let Some(r) = e["identifier"]["@id"].as_str() {
                assert!(ids.contains(&r), "dangling identifier reference {r}");
                pvs += 1;
            }
        }
        assert_eq!(pvs, 2, "both edges should reference their own hash entity");
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
