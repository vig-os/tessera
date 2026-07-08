//! `InspectFacets` — the tabbed metadata inspectors (the Inspect wireframe). Where
//! [`ArtifactVerdict`](crate::verify) is the *deep* verify (re-hashes every block), this is the
//! **cheap, manifest-only** companion that backs the Inspect mode's seven tabs: it decodes **no block
//! payloads** — only the manifest, the aux directory, and (for Trust) the small embedded signature
//! member. So opening Inspect on a multi-gigabyte product is instant.
//!
//! Every facet is a plain, `Serialize`-able projection so the same data backs the TUI tabs, an MCP
//! tool, and a future `serve`. It reuses the shared verify view-models ([`SchemaVerdict`],
//! [`SignatureInfo`], [`LineageSummary`]) rather than recomputing them, and it is honest by
//! construction: Trust reports the signature's *declared* identity (never a trust-anchor match), and
//! FAIR/Governance assert only what the manifest actually carries.

use std::io::{Read, Seek};

use serde::Serialize;
use tessera_core::block::array::{ArraySpec, WorldFrame};
use tessera_core::schema::Sensitivity;
use tessera_core::{Manifest, ProductSchema, SchemaRegistry};
use tessera_io::Reader;

use crate::verify::{
    lineage_summary, read_signature, schema_verdict, LineageSummary, SchemaVerdict, SignatureInfo,
};

/// The full set of Inspect-tab facets, computed once from an opened container (no block decode).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct InspectFacets {
    /// Integrity tab — identity, seal, block digests, product/schema.
    pub integrity: IntegrityFacet,
    /// Provenance tab — the `sources` DAG edges + a lineage summary.
    pub provenance: ProvenanceFacet,
    /// Trust tab — the embedded signature envelope (attribution), if any.
    pub trust: TrustFacet,
    /// Schema tab — conformance verdict + the declared field roster.
    pub schema: SchemaFacet,
    /// Referencing tab — per-array spatial frames (convention · unit · space · spacing).
    pub referencing: ReferencingFacet,
    /// Governance tab — PHI/sensitivity posture.
    pub governance: GovernanceFacet,
    /// FAIR tab — a findable/accessible/interoperable/reusable checklist.
    pub fair: FairFacet,
}

/// Integrity tab — the artifact's identity and seal, manifest-only (no re-hash; that is Verify).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct IntegrityFacet {
    /// The lineage handle (`id`).
    pub id: String,
    /// The version seal (`manifest_hash`), or `None` if unsealed.
    pub manifest_hash: Option<String>,
    /// Whether the manifest carries a seal.
    pub sealed: bool,
    /// The product name.
    pub product: String,
    /// Schema conformance verdict.
    pub schema: SchemaVerdict,
    /// One row per storage block (name · kind · shortened digest).
    pub blocks: Vec<BlockLine>,
}

/// One block row in the Integrity tab.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BlockLine {
    /// Block name.
    pub name: String,
    /// Block kind (`array` / `table` / `blob` / `chunk_index`).
    pub kind: String,
    /// Recorded digest, shortened to a glanceable prefix (`—` if absent).
    pub digest: String,
}

/// Provenance tab — the provenance DAG edges and their integrity-chain coverage.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProvenanceFacet {
    /// One row per `sources` edge.
    pub edges: Vec<SourceRow>,
    /// The edge-count summary (total · how many pin an upstream `content_hash`).
    pub summary: LineageSummary,
}

/// One provenance edge in the Provenance tab.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceRow {
    /// Typed role of the edge (`ingested_from`, `calibration`, …).
    pub role: String,
    /// Upstream artifact reference (path / id).
    pub reference: String,
    /// Upstream `content_hash` (shortened), when the edge closes the integrity chain.
    pub content_hash: Option<String>,
}

/// Trust tab — the embedded signature envelope (attribution, not a trust proof).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TrustFacet {
    /// The signature envelope, or `None` when the product is unsigned.
    pub signature: Option<SignatureInfo>,
}

/// Schema tab — conformance verdict + the declared field roster.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SchemaFacet {
    /// Conformance verdict (conformant / non-conformant / open-world).
    pub verdict: SchemaVerdict,
    /// The resolved schema's `product v<version>` heading, or `None` when open-world.
    pub heading: Option<String>,
    /// One row per declared field (present in the manifest metadata or not).
    pub fields: Vec<FieldRow>,
}

/// One declared schema field in the Schema tab.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FieldRow {
    /// Field id.
    pub id: String,
    /// Tier — `required` / `recommended` / `optional`.
    pub tier: String,
    /// PHI sensitivity tier (`public` / `coded` / `sensitive` / `identifying`).
    pub sensitivity: String,
    /// Whether this field is carried in the manifest metadata.
    pub present: bool,
}

/// Referencing tab — the spatial frames of the product's array blocks.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReferencingFacet {
    /// One row per array block that carries a world frame (empty ⇒ index-space only).
    pub frames: Vec<FrameRow>,
}

/// One array block's spatial frame in the Referencing tab.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FrameRow {
    /// Owning block name.
    pub block: String,
    /// World handedness convention (`LPS` canonical).
    pub convention: String,
    /// World-coordinate unit (UCUM, e.g. `mm`).
    pub unit: String,
    /// Named target frame (`patient` / `scanner` / `aligned` / `atlas:<id>`).
    pub space: String,
    /// Per-axis voxel spacing derived from the affine.
    pub spacing: [f64; 3],
}

/// Governance tab — the product's PHI / sensitivity posture (schema-declared).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GovernanceFacet {
    /// The schema declares one or more directly-identifying (PHI) fields.
    pub has_phi: bool,
    /// Count of declared fields at each sensitivity tier (public · coded · sensitive · identifying).
    pub sensitivity_counts: SensitivityCounts,
    /// Identifying fields that are actually **present in the metadata in the clear** — the ingest-warn
    /// signal (the hook the future redact / field-encryption phases replace). Empty is the clean state.
    pub identifying_in_clear: Vec<String>,
    /// Whether the product is sealed (immutable, content-addressed) — the tamper-evidence posture.
    pub sealed: bool,
}

/// Per-tier counts of declared schema fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct SensitivityCounts {
    /// Safe-in-clear fields.
    pub public: usize,
    /// Controlled-vocabulary code fields.
    pub coded: usize,
    /// Clinical-but-not-identifying fields.
    pub sensitive: usize,
    /// Directly-identifying (PHI) fields.
    pub identifying: usize,
}

/// FAIR tab — a findable/accessible/interoperable/reusable checklist, each with a short reason.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FairFacet {
    /// Findable — a stable, content-addressed identity.
    pub findable: FairCheck,
    /// Accessible — sealed + retrievable as one self-contained container.
    pub accessible: FairCheck,
    /// Interoperable — a known schema and/or standard spatial referencing.
    pub interoperable: FairCheck,
    /// Reusable — provenance + schema conformance.
    pub reusable: FairCheck,
}

impl FairFacet {
    /// How many of the four FAIR dimensions are met.
    pub fn met(&self) -> usize {
        [
            &self.findable,
            &self.accessible,
            &self.interoperable,
            &self.reusable,
        ]
        .iter()
        .filter(|c| c.met)
        .count()
    }
}

/// One FAIR dimension: met-or-not + a short justification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FairCheck {
    /// Whether the dimension is satisfied.
    pub met: bool,
    /// A short reason (`"content-addressed id + version seal"`).
    pub reason: String,
}

/// Compute all Inspect-tab facets for an opened container. Cheap — manifest + aux + the embedded
/// signature member only (no block payload decode), so it is safe to run on entering Inspect and cache.
pub fn inspect_facets<R: Read + Seek>(reader: &mut Reader<R>) -> InspectFacets {
    let manifest = reader.manifest().clone();
    let signature = read_signature(reader);
    facets_from(&manifest, signature)
}

/// The pure core of [`inspect_facets`] — build the facets from an already-read manifest + signature.
/// Split out so it is testable without a live [`Reader`].
pub fn facets_from(m: &Manifest, signature: Option<SignatureInfo>) -> InspectFacets {
    let schema = resolve_schema(m);
    InspectFacets {
        integrity: integrity_facet(m),
        provenance: provenance_facet(m),
        trust: TrustFacet { signature },
        schema: schema_facet(m, schema.as_ref()),
        referencing: referencing_facet(m),
        governance: governance_facet(m, schema.as_ref()),
        fair: fair_facet(m, schema.as_ref()),
    }
}

/// Resolve the product's schema: the embedded one if present, else the builtin registry's.
fn resolve_schema(m: &Manifest) -> Option<ProductSchema> {
    if let Some(s) = m
        .schema
        .as_ref()
        .and_then(|v| ProductSchema::from_value(v).ok())
    {
        return Some(s);
    }
    SchemaRegistry::builtin().get(&m.product).cloned()
}

fn integrity_facet(m: &Manifest) -> IntegrityFacet {
    IntegrityFacet {
        id: m.id.clone(),
        manifest_hash: m.manifest_hash.clone(),
        sealed: m.manifest_hash.is_some(),
        product: m.product.clone(),
        schema: schema_verdict(m),
        blocks: m
            .blocks
            .iter()
            .map(|b| BlockLine {
                name: b.name.clone(),
                kind: format!("{:?}", b.kind).to_lowercase(),
                digest: b
                    .digest
                    .as_deref()
                    .map(short_hash)
                    .unwrap_or_else(|| "—".into()),
            })
            .collect(),
    }
}

fn provenance_facet(m: &Manifest) -> ProvenanceFacet {
    ProvenanceFacet {
        edges: m
            .sources
            .iter()
            .map(|s| SourceRow {
                role: s.role.clone(),
                reference: s.reference.clone(),
                content_hash: s.content_hash.as_deref().map(short_hash),
            })
            .collect(),
        summary: lineage_summary(m),
    }
}

fn schema_facet(m: &Manifest, schema: Option<&ProductSchema>) -> SchemaFacet {
    let (heading, fields) = match schema {
        Some(s) => (
            Some(format!("{} v{}", s.product, s.version)),
            s.fields
                .iter()
                .map(|f| FieldRow {
                    id: f.id.clone(),
                    tier: field_tier(f.required, f.recommended).into(),
                    sensitivity: format!("{:?}", f.sensitivity).to_lowercase(),
                    present: m.metadata.contains_key(&f.id),
                })
                .collect(),
        ),
        None => (None, Vec::new()),
    };
    SchemaFacet {
        verdict: schema_verdict(m),
        heading,
        fields,
    }
}

fn referencing_facet(m: &Manifest) -> ReferencingFacet {
    let frames = m
        .blocks
        .iter()
        .filter_map(|b| {
            let spec: ArraySpec = serde_json::from_value(b.spec.clone()).ok()?;
            let wf: WorldFrame = spec.world_frame?;
            Some(FrameRow {
                block: b.name.clone(),
                convention: wf.convention.clone(),
                unit: wf.unit.clone(),
                space: wf.space.clone(),
                spacing: wf.spacing(),
            })
        })
        .collect();
    ReferencingFacet { frames }
}

fn governance_facet(m: &Manifest, schema: Option<&ProductSchema>) -> GovernanceFacet {
    let mut counts = SensitivityCounts::default();
    let mut identifying_in_clear = Vec::new();
    if let Some(s) = schema {
        for f in &s.fields {
            match f.sensitivity {
                Sensitivity::Public => counts.public += 1,
                Sensitivity::Coded => counts.coded += 1,
                Sensitivity::Sensitive => counts.sensitive += 1,
                Sensitivity::Identifying => {
                    counts.identifying += 1;
                    if m.metadata.contains_key(&f.id) {
                        identifying_in_clear.push(f.id.clone());
                    }
                }
            }
        }
    }
    GovernanceFacet {
        has_phi: counts.identifying > 0,
        sensitivity_counts: counts,
        identifying_in_clear,
        sealed: m.manifest_hash.is_some(),
    }
}

fn fair_facet(m: &Manifest, schema: Option<&ProductSchema>) -> FairFacet {
    let sealed = m.manifest_hash.is_some();
    let has_id = !m.id.is_empty();
    let known_schema = schema.is_some();
    let conformant = matches!(schema_verdict(m), SchemaVerdict::Conformant);
    let has_referencing = m.blocks.iter().any(|b| {
        serde_json::from_value::<ArraySpec>(b.spec.clone())
            .ok()
            .and_then(|s| s.world_frame)
            .is_some()
    });
    let has_provenance = !m.sources.is_empty();

    FairFacet {
        findable: FairCheck {
            met: has_id && sealed,
            reason: if has_id && sealed {
                "content-addressed id + version seal".into()
            } else {
                "missing stable id / seal".into()
            },
        },
        accessible: FairCheck {
            met: sealed,
            reason: if sealed {
                "sealed, self-contained container".into()
            } else {
                "unsealed — not a retrievable version".into()
            },
        },
        interoperable: FairCheck {
            met: known_schema || has_referencing,
            reason: match (known_schema, has_referencing) {
                (true, true) => "known schema + world-referenced".into(),
                (true, false) => "known schema".into(),
                (false, true) => "world-referenced (index+affine)".into(),
                (false, false) => "open-world, index-space only".into(),
            },
        },
        reusable: FairCheck {
            met: conformant && has_provenance,
            reason: match (conformant, has_provenance) {
                (true, true) => "schema-conformant + provenance recorded".into(),
                (true, false) => "conformant but no provenance edges".into(),
                (false, true) => "provenance present but not schema-conformant".into(),
                (false, false) => "no schema conformance or provenance".into(),
            },
        },
    }
}

/// `required` / `recommended` / `optional` from a schema field's two flags.
fn field_tier(required: bool, recommended: bool) -> &'static str {
    if required {
        "required"
    } else if recommended {
        "recommended"
    } else {
        "optional"
    }
}

/// Shorten a `blake3:<hex>` digest to a glanceable prefix.
fn short_hash(d: &str) -> String {
    match d.split_once(':') {
        Some((alg, hex)) if hex.len() > 12 => format!("{alg}:{}…", &hex[..12]),
        _ => d.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tessera_core::block::array::ArraySpec;
    use tessera_core::block::{BlockKind, BlockRef};
    use tessera_core::provenance::Source;

    /// A minimal known-product manifest (listmode is a builtin schema) with one table block.
    fn manifest() -> Manifest {
        let mut m = Manifest::new("listmode", "study", "d", "2024-01-01T00:00:00Z");
        m.manifest_hash = Some("blake3:aaaa000011112222333344445555".into());
        m.blocks.push(BlockRef {
            name: "events".into(),
            kind: BlockKind::Table,
            digest: Some("blake3:1111000011112222333344445555".into()),
            spec: json!({}),
        });
        m
    }

    #[test]
    fn integrity_and_provenance_facets_project_the_manifest() {
        let mut m = manifest();
        m.sources.push(Source {
            role: "ingested_from".into(),
            reference: "raw.l64".into(),
            content_hash: Some("blake3:dddd000011112222333344445555".into()),
        });
        let f = facets_from(&m, None);
        assert!(f.integrity.sealed);
        assert_eq!(f.integrity.blocks.len(), 1);
        assert_eq!(f.integrity.blocks[0].kind, "table");
        assert!(f.integrity.blocks[0].digest.ends_with('…'));
        assert_eq!(f.provenance.edges.len(), 1);
        assert_eq!(f.provenance.summary.edges, 1);
        assert_eq!(f.provenance.summary.with_content_hash, 1);
        assert!(f.provenance.edges[0].content_hash.is_some());
        // No signature → Trust facet is empty and honest.
        assert!(f.trust.signature.is_none());
    }

    #[test]
    fn referencing_facet_surfaces_a_world_frame() {
        let mut m = manifest();
        let spec = ArraySpec {
            world_frame: Some(WorldFrame {
                affine: [
                    2.0, 0.0, 0.0, -100.0, 0.0, 3.0, 0.0, -80.0, 0.0, 0.0, 4.0, -60.0,
                ],
                convention: "LPS".into(),
                unit: "mm".into(),
                space: "patient".into(),
            }),
            ..ArraySpec::new(vec![4, 4, 4], "int16")
        };
        m.blocks.push(BlockRef {
            name: "volume".into(),
            kind: BlockKind::Array,
            digest: Some("blake3:2222000011112222333344445555".into()),
            spec: serde_json::to_value(&spec).unwrap(),
        });
        let f = facets_from(&m, None);
        assert_eq!(f.referencing.frames.len(), 1);
        let fr = &f.referencing.frames[0];
        assert_eq!(fr.block, "volume");
        assert_eq!(fr.convention, "LPS");
        assert_eq!(fr.spacing, [2.0, 3.0, 4.0]);
    }

    #[test]
    fn governance_and_fair_are_honest_projections() {
        let m = manifest();
        let f = facets_from(&m, None);
        // listmode is a known schema → interoperable met; sealed → findable + accessible met.
        assert!(f.fair.findable.met);
        assert!(f.fair.accessible.met);
        assert!(f.fair.interoperable.met);
        // No provenance edges on this minimal product → reusable not met (honest).
        assert!(!f.fair.reusable.met);
        assert!(f.fair.met() >= 3);
        // Governance counts come from the resolved schema; sealed posture is surfaced.
        assert!(f.governance.sealed);
        assert!(f.governance.identifying_in_clear.is_empty());
    }

    #[test]
    fn facets_serialize_for_agent_and_serve_surfaces() {
        let m = manifest();
        let j = serde_json::to_value(facets_from(&m, None)).unwrap();
        assert_eq!(j["integrity"]["sealed"], true);
        assert!(j["schema"]["verdict"].is_string());
        assert!(j["fair"]["findable"]["met"].is_boolean());
    }
}
