//! `ArtifactVerdict` — the deep verification view-model (the Verify wireframe). The **second and
//! last** Phase-1b view-model addition (the first is [`hierarchy::NodeTree`](crate::hierarchy)).
//!
//! Where [`HeaderStatus`](crate::hierarchy) is a *structural* glance (manifest-only: "sealed", "sig
//! present"), this is the *deep* check a Verify pane / `tsra verify` / an auditor's signed report
//! renders: the seal, **every block digest streamed and confirmed**, schema conformance, the embedded
//! signature's envelope, and a lineage summary. It runs over a generic [`Reader`] (local + `cloud`) —
//! no `&Path` — so the same verdict backs the TUI, an MCP tool, and a future `serve`.
//!
//! Honesty boundary: this confirms **integrity** (the bytes match their sealed digests) and reports the
//! signature's *declared* identity, but it does **not** perform trust-anchor resolution — proving a
//! signature was made by a *trusted* key needs a trust store (the CLI `tsra verify-sig`). The verdict
//! says so via [`SignatureInfo::trust_checked`] (always `false` here), never overclaiming.

use std::io::{Read, Seek};

use serde::Serialize;
use tessera_core::signing::Signature;
use tessera_core::{Manifest, SchemaRegistry};
use tessera_io::Reader;

/// The full verification verdict for one `.tsra`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ArtifactVerdict {
    /// The manifest carries a seal (`manifest_hash`) — checked when the container was opened.
    pub sealed: bool,
    /// Deep integrity: every block's payload re-hashed against its recorded digest.
    pub integrity: IntegrityCheck,
    /// Schema conformance against the embedded/registry schema.
    pub schema: SchemaVerdict,
    /// The embedded signature's envelope, when one is present (attribution — not a trust proof).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<SignatureInfo>,
    /// Provenance-edge summary.
    pub lineage: LineageSummary,
}

impl ArtifactVerdict {
    /// `true` when the artifact is sealed and every block re-hashed to its recorded digest — the
    /// "the bytes are intact and match the seal" summary. Deliberately **byte-integrity only**: schema
    /// conformance and signature *trust* are separate dimensions reported on their own fields, so a
    /// schema-imperfect-but-untampered product still reads as integrity-ok.
    pub fn integrity_ok(&self) -> bool {
        self.sealed && self.integrity.all_ok()
    }
}

/// The outcome of streaming every block and re-checking its digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IntegrityCheck {
    /// Blocks in the manifest.
    pub blocks_total: usize,
    /// Blocks whose payload re-hashed to the recorded digest.
    pub blocks_ok: usize,
    /// The first block that failed (name + reason), if any — `None` means all passed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
}

impl IntegrityCheck {
    /// Every block verified (and at least the count is internally consistent).
    pub fn all_ok(&self) -> bool {
        self.failure.is_none() && self.blocks_ok == self.blocks_total
    }
}

/// Schema conformance state (mirrors the manifest-only glance, recomputed here for a complete verdict).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SchemaVerdict {
    /// Schema known (embedded or registry) and the manifest validates.
    Conformant,
    /// Schema known but the manifest fails validation.
    NonConformant,
    /// No schema for this product — open-world.
    OpenWorld,
}

/// The embedded signature's envelope fields — **attribution**, surfaced from the signature the
/// producer embedded. Presence + declared identity, not a trust-anchor match.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SignatureInfo {
    /// Signature scheme (`ed25519`, `ssh-ed25519`, …).
    pub alg: String,
    /// Verifying-key identifier (hex public key / ssh fingerprint).
    pub key_id: String,
    /// Signer identity (ORCID / institutional URI), when declared.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signer: Option<String>,
    /// RFC-3339 time of signing, when declared.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signed_at: Option<String>,
    /// `key_id` encoding tag (`raw-hex` / `ssh-ed25519`), when declared.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_format: Option<String>,
    /// Always `false` here — trust-anchor resolution is a trust-store concern (`tsra verify-sig`).
    /// Surfaced so a renderer never mistakes "signed" for "trusted".
    pub trust_checked: bool,
}

/// A summary of the provenance DAG edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct LineageSummary {
    /// Number of provenance edges (`sources`).
    pub edges: usize,
    /// How many edges pin an upstream `content_hash` (close the integrity chain).
    pub with_content_hash: usize,
}

/// Compute the [`ArtifactVerdict`] for an opened container. Streams **every block** (bounded memory)
/// to confirm its digest, so this is the deep check — more than the manifest-only glance. Opening the
/// [`Reader`] already validated the magic + manifest seal; this adds per-block integrity + schema +
/// the embedded signature envelope + lineage.
pub fn artifact_verdict<R: Read + Seek>(reader: &mut Reader<R>) -> ArtifactVerdict {
    // Snapshot what we need from the manifest before the mutable block/aux reads borrow the reader.
    let manifest = reader.manifest().clone();
    let block_names: Vec<String> = manifest.blocks.iter().map(|b| b.name.clone()).collect();
    let sig_member = reader
        .aux_names()
        .into_iter()
        .find(|n| n.starts_with("signatures/"));

    // Deep integrity: re-read each block (payload bytes vs recorded digest); stop-report the first fail.
    let mut blocks_ok = 0usize;
    let mut failure = None;
    for name in &block_names {
        match reader.read_block(name) {
            Ok(_) => blocks_ok += 1,
            Err(e) => {
                failure = Some(format!("{name}: {e}"));
                break;
            }
        }
    }
    let integrity = IntegrityCheck {
        blocks_total: block_names.len(),
        blocks_ok,
        failure,
    };

    // The embedded signature envelope (attribution), read from the aux member — never a trust proof.
    let signature = sig_member.and_then(|name| read_signature_member(reader, &name));

    ArtifactVerdict {
        sealed: manifest.manifest_hash.is_some(),
        integrity,
        schema: schema_verdict(&manifest),
        signature,
        lineage: lineage_summary(&manifest),
    }
}

/// Read + parse the embedded signature envelope (attribution) from an aux member, or `None` if it is
/// absent / unreadable / malformed. Cheap (one small aux read, no block decode) — shared by the deep
/// [`artifact_verdict`] and the Inspect Trust tab. `trust_checked` is always `false`: this surfaces the
/// signature's *declared* identity, never a trust-anchor match (that is `tsra verify-sig`).
pub fn read_signature<R: Read + Seek>(reader: &mut Reader<R>) -> Option<SignatureInfo> {
    let name = reader
        .aux_names()
        .into_iter()
        .find(|n| n.starts_with("signatures/"))?;
    read_signature_member(reader, &name)
}

/// Parse a specific signature aux member into a [`SignatureInfo`].
fn read_signature_member<R: Read + Seek>(
    reader: &mut Reader<R>,
    name: &str,
) -> Option<SignatureInfo> {
    let bytes = reader.read_aux(name).ok()?;
    let sig: Signature = serde_json::from_slice(&bytes).ok()?;
    Some(SignatureInfo {
        alg: sig.alg,
        key_id: sig.key_id,
        signer: sig.signer,
        signed_at: sig.signed_at,
        key_format: sig.key_format,
        trust_checked: false,
    })
}

/// Summarise the provenance DAG edges (`sources`) — total edges and how many pin an upstream
/// `content_hash`. Pure manifest projection, shared by [`artifact_verdict`] and the Inspect tabs.
pub fn lineage_summary(m: &Manifest) -> LineageSummary {
    LineageSummary {
        edges: m.sources.len(),
        with_content_hash: m
            .sources
            .iter()
            .filter(|s| s.content_hash.is_some())
            .count(),
    }
}

/// Schema conformance for a manifest — known+valid / known+invalid / open-world. Shared by
/// [`artifact_verdict`] and the Inspect Schema tab.
pub fn schema_verdict(m: &Manifest) -> SchemaVerdict {
    let known = m.schema.is_some() || SchemaRegistry::builtin().get(&m.product).is_some();
    if !known {
        SchemaVerdict::OpenWorld
    } else if tessera_core::validate_manifest(m).is_ok() {
        SchemaVerdict::Conformant
    } else {
        SchemaVerdict::NonConformant
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tessera_core::block::table::{Column, TableSpec};
    use tessera_core::ProductBuilder;
    use tessera_io::{pack, table::TableData, ColumnData};

    /// Seal a tiny one-table product to a `.tsra` and return its path (in `dir`).
    fn sample(dir: &std::path::Path) -> std::path::PathBuf {
        let spec = TableSpec {
            columns: vec![Column {
                name: "ms".into(),
                dtype: "u4".into(),
                codec: None,
            }],
            rows: 3,
            row_index: None,
        };
        let data: TableData = vec![("ms".into(), ColumnData::U32(vec![1, 2, 3]))];
        let (block_ref, payload) = tessera_io::table::table_block("events", &spec, &data).unwrap();
        let mut b = ProductBuilder::new("listmode", "DP", "d", "2024-01-01T00:00:00Z");
        b.add_block_ref(block_ref);
        let sealed = b.seal().unwrap();
        let path = dir.join("p.tsra");
        pack(&sealed, &[payload], &path).unwrap();
        path
    }

    #[test]
    fn verdict_of_a_clean_sealed_product_passes_integrity() {
        let dir = tempfile::tempdir().unwrap();
        let path = sample(dir.path());
        let mut r = Reader::open(&path).unwrap();
        let v = artifact_verdict(&mut r);
        assert!(v.sealed);
        assert_eq!(v.integrity.blocks_total, 1);
        assert_eq!(v.integrity.blocks_ok, 1);
        assert!(v.integrity.all_ok());
        assert!(v.integrity_ok());
        // No signature embedded, no provenance edges on this minimal product.
        assert!(v.signature.is_none());
        assert_eq!(v.lineage.edges, 0);
    }

    #[test]
    fn serializes_for_the_agent_and_serve_surfaces() {
        let dir = tempfile::tempdir().unwrap();
        let path = sample(dir.path());
        let mut r = Reader::open(&path).unwrap();
        let v = artifact_verdict(&mut r);
        let j = serde_json::to_value(&v).unwrap();
        assert_eq!(j["integrity"]["blocks_ok"], 1);
        // `schema` renders as a snake_case tag (listmode is a known product → conformant or not).
        assert!(j["schema"].is_string());
    }
}
