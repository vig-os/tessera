//! The conformance corpus — deterministic reference products whose golden hashes are pinned in
//! `tessera/corpus/corpus.json`.
//!
//! Two guarantees ride on this: (1) rebuilding any fixture reproduces its
//! `id`/`content_hash`/`manifest_hash` exactly — the writer-determinism + cross-version-drift gate;
//! and (2) the recorded hashes are the contract a second, independent implementation (the v1.0
//! pure-Python reader) must reproduce from `SPEC.md` alone. Every input here is a fixed constant.

use serde::{Deserialize, Serialize};
use tessera_core::block::array::{ArrayBlock, ArraySpec};
use tessera_core::block::table::{Column, TableBlock, TableSpec};
use tessera_core::manifest::Manifest;
use tessera_core::provenance::Source;
use tessera_core::ProductBuilder;

use crate::BlockPayload;

/// One corpus fixture: a deterministic product + the payloads to pack into its `.tsra`.
pub struct Fixture {
    pub name: &'static str,
    pub manifest: Manifest,
    pub payloads: Vec<BlockPayload>,
}

/// The golden record for a fixture (what `corpus/corpus.json` stores and what an independent
/// implementation must reproduce).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Golden {
    pub name: String,
    pub product: String,
    pub id: String,
    pub content_hash: String,
    pub manifest_hash: String,
}

impl Fixture {
    pub fn golden(&self) -> Golden {
        Golden {
            name: self.name.into(),
            product: self.manifest.product.clone(),
            id: self.manifest.id.clone(),
            content_hash: self.manifest.content_hash.clone().unwrap_or_default(),
            manifest_hash: self.manifest.manifest_hash.clone().unwrap_or_default(),
        }
    }
}

fn array_payload(b: &ArrayBlock) -> BlockPayload {
    BlockPayload::new(b.name.clone(), serde_json::to_vec(&b.spec).unwrap())
}
fn table_payload(b: &TableBlock) -> BlockPayload {
    BlockPayload::new(b.name.clone(), serde_json::to_vec(&b.spec).unwrap())
}

fn col(name: &str, dtype: &str, codec: Option<&str>) -> Column {
    Column {
        name: name.into(),
        dtype: dtype.into(),
        codec: codec.map(Into::into),
    }
}

/// Build the deterministic conformance corpus (fixed inputs → reproducible hashes).
pub fn fixtures() -> Vec<Fixture> {
    const TS: &str = "2024-01-01T00:00:00Z";
    let mut out = Vec::new();

    // 1 — recon, native int16 volume with modality + rescale + unit (the canonical CT product).
    {
        let vol = ArrayBlock::new(
            "volume",
            ArraySpec::new(vec![64, 64, 64], "int16")
                .with_rescale(1.0, -1024.0)
                .with_unit("HU"),
        );
        let pl = array_payload(&vol);
        let mut b = ProductBuilder::new("recon", "recon-int16", "int16 CT volume", TS);
        b.add_block(&vol).unwrap();
        b.with_field(
            "modality",
            serde_json::json!({"_vocabulary": "DICOM", "_code": "CT"}),
        );
        out.push(Fixture {
            name: "recon_int16",
            manifest: b.seal().unwrap(),
            payloads: vec![pl],
        });
    }

    // 2 — recon, float32 attenuation (μ-) map.
    {
        let mu = ArrayBlock::new(
            "volume",
            ArraySpec::new(vec![32, 32, 32], "float32").with_unit("1/cm"),
        );
        let pl = array_payload(&mu);
        let mut b = ProductBuilder::new("recon", "recon-mumap", "float32 μ-map", TS);
        b.add_block(&mu).unwrap();
        b.with_field(
            "modality",
            serde_json::json!({"_vocabulary": "DICOM", "_code": "OT"}),
        );
        out.push(Fixture {
            name: "recon_float32_mumap",
            manifest: b.seal().unwrap(),
            payloads: vec![pl],
        });
    }

    // 3 — listmode, columnar event table.
    {
        let events = TableBlock::new(
            "events",
            TableSpec {
                columns: vec![
                    col("t", "u8", Some("zstd")),
                    col("e0", "f4", Some("zstd")),
                    col("e1", "f4", Some("zstd")),
                ],
                rows: 1_000_000,
                row_index: Some("t".into()),
            },
        );
        let pl = table_payload(&events);
        let mut b = ProductBuilder::new("listmode", "lm-3p", "extended-coincidence events", TS);
        b.add_block(&events).unwrap();
        b.with_field(
            "coincidence_mode",
            serde_json::json!("extended-coincidence"),
        );
        out.push(Fixture {
            name: "listmode_events",
            manifest: b.seal().unwrap(),
            payloads: vec![pl],
        });
    }

    // 4 — spectrum, a 1-D positronium-lifetime histogram (named axis).
    {
        let spec = ArrayBlock::new(
            "spectrum",
            ArraySpec::new(vec![512], "float32")
                .with_axes(vec!["lifetime".into()])
                .with_unit("counts"),
        );
        let pl = array_payload(&spec);
        let mut b = ProductBuilder::new("spectrum", "ps-lifetime", "o-Ps lifetime histogram", TS);
        b.add_block(&spec).unwrap();
        b.with_field("domain", serde_json::json!("lifetime"));
        out.push(Fixture {
            name: "spectrum_lifetime",
            manifest: b.seal().unwrap(),
            payloads: vec![pl],
        });
    }

    // 5 — multi-block product with study + provenance edge + extension field.
    {
        let vol = ArrayBlock::new("volume", ArraySpec::new(vec![16, 16, 16], "int16"));
        let roi = TableBlock::new(
            "roi",
            TableSpec {
                columns: vec![col("label", "u4", None), col("volume_ml", "f4", None)],
                rows: 8,
                row_index: None,
            },
        );
        let payloads = vec![array_payload(&vol), table_payload(&roi)];
        let mut b = ProductBuilder::new("recon", "recon-with-roi", "volume + ROIs", TS);
        b.add_block(&vol).unwrap();
        b.add_block(&roi).unwrap();
        b.with_field("modality", serde_json::json!("CT"));
        b.with_study("DUPLET-DP06-exam");
        b.add_source(Source::new("derived_from", "recon-int16"));
        b.with_extra("vendor_note", serde_json::json!("synthetic fixture"));
        out.push(Fixture {
            name: "multiblock_study",
            manifest: b.seal().unwrap(),
            payloads,
        });
    }

    // 6 — edge: a metadata-only product with no blocks (open-world product type).
    {
        let m = ProductBuilder::new("marker", "empty", "no-block marker product", TS)
            .seal()
            .unwrap();
        out.push(Fixture {
            name: "empty_no_blocks",
            manifest: m,
            payloads: vec![],
        });
    }

    out
}

/// The golden records for the whole corpus.
pub fn goldens() -> Vec<Golden> {
    fixtures().iter().map(Fixture::golden).collect()
}
