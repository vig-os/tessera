//! The conformance corpus — deterministic reference products whose golden hashes are pinned in
//! `tessera/corpus/corpus.json`.
//!
//! Two guarantees ride on this: (1) rebuilding any fixture reproduces its
//! `id`/`content_hash`/`manifest_hash` exactly — the writer-determinism + cross-version-drift gate;
//! and (2) the recorded hashes are the contract a second, independent implementation (the v1.0
//! pure-Python reader) must reproduce from `SPEC.md` alone. Every input here is a fixed constant.
//!
//! Array blocks use Zarr v3 + pcodec ([`crate::array`]); table blocks use Vortex ([`crate::table`]).
//! Both carry real, byte-deterministic payloads. Regenerate after an intended format change with
//! `cargo run -p tessera-io --example gen_corpus`.

use serde::{Deserialize, Serialize};
use tessera_core::block::array::ArraySpec;
use tessera_core::block::table::{Column, TableSpec};
use tessera_core::manifest::Manifest;
use tessera_core::provenance::Source;
use tessera_core::ProductBuilder;

use crate::array::{self, ArrayData};
use crate::table::{self, ColumnData, TableData};
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

/// Encode a real array block (Zarr v3 + pcodec), register its digested ref on the builder, and
/// return the payload to pack. The digest is over the encoded bytes (not the spec).
fn push_array(
    b: &mut ProductBuilder,
    name: &str,
    spec: &ArraySpec,
    data: ArrayData,
) -> BlockPayload {
    let (block_ref, payload) =
        array::array_block(name, spec, &data).expect("fixture array encodes");
    b.add_block_ref(block_ref);
    payload
}

/// Encode a real table block (Vortex), register its digested ref on the builder, and return the
/// payload to pack. The digest is over the encoded bytes (not the spec).
fn push_table(
    b: &mut ProductBuilder,
    name: &str,
    spec: &TableSpec,
    data: TableData,
) -> BlockPayload {
    let (block_ref, payload) =
        table::table_block(name, spec, &data).expect("fixture table encodes");
    b.add_block_ref(block_ref);
    payload
}

fn col(name: &str, dtype: &str, codec: Option<&str>) -> Column {
    Column {
        name: name.into(),
        dtype: dtype.into(),
        codec: codec.map(Into::into),
    }
}

// ── deterministic sample generators (fixed → reproducible hashes) ───────────────────────────────
fn ramp_i16(n: usize) -> ArrayData {
    ArrayData::I16((0..n).map(|k| (k % 4096) as i16 - 1024).collect())
}
fn ramp_f32(n: usize) -> ArrayData {
    ArrayData::F32((0..n).map(|k| k as f32 * 0.001).collect())
}

/// Build the deterministic conformance corpus (fixed inputs → reproducible hashes).
pub fn fixtures() -> Vec<Fixture> {
    const TS: &str = "2024-01-01T00:00:00Z";
    let mut out = Vec::new();

    // 1 — recon, native int16 volume with modality + rescale + unit (the canonical CT product).
    {
        let spec = ArraySpec::new(vec![64, 64, 64], "int16")
            .with_rescale(1.0, -1024.0)
            .with_unit("HU");
        let mut b = ProductBuilder::new("recon", "recon-int16", "int16 CT volume", TS);
        let pl = push_array(&mut b, "volume", &spec, ramp_i16(64 * 64 * 64));
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
        let spec = ArraySpec::new(vec![32, 32, 32], "float32").with_unit("1/cm");
        let mut b = ProductBuilder::new("recon", "recon-mumap", "float32 μ-map", TS);
        let pl = push_array(&mut b, "volume", &spec, ramp_f32(32 * 32 * 32));
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

    // 3 — listmode, columnar event table (real Vortex payload). Row count is kept small — a
    // conformance fixture tests determinism/correctness, not scale (scale is the bench's job, #206).
    {
        const N: usize = 4096;
        let spec = TableSpec {
            columns: vec![
                col("t", "u8", Some("zstd")),
                col("e0", "f4", Some("zstd")),
                col("e1", "f4", Some("zstd")),
            ],
            rows: N as u64,
            row_index: Some("t".into()),
        };
        let data: TableData = vec![
            ("t".into(), ColumnData::U64((0..N as u64).collect())),
            (
                "e0".into(),
                ColumnData::F32((0..N).map(|k| k as f32 * 0.01).collect()),
            ),
            (
                "e1".into(),
                ColumnData::F32((0..N).map(|k| k as f32 * 0.02 - 5.0).collect()),
            ),
        ];
        let mut b = ProductBuilder::new("listmode", "lm-3p", "extended-coincidence events", TS);
        let pl = push_table(&mut b, "events", &spec, data);
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
        let spec = ArraySpec::new(vec![512], "float32")
            .with_axes(vec!["lifetime".into()])
            .with_unit("counts");
        let data = ArrayData::F32(
            (0..512)
                .map(|k| 1000.0 * (-(k as f32) / 64.0).exp())
                .collect(),
        );
        let mut b = ProductBuilder::new("spectrum", "ps-lifetime", "o-Ps lifetime histogram", TS);
        let pl = push_array(&mut b, "spectrum", &spec, data);
        b.with_field("domain", serde_json::json!("lifetime"));
        out.push(Fixture {
            name: "spectrum_lifetime",
            manifest: b.seal().unwrap(),
            payloads: vec![pl],
        });
    }

    // 5 — multi-block product with study + provenance edge + extension field.
    {
        let vol_spec = ArraySpec::new(vec![16, 16, 16], "int16");
        let roi_spec = TableSpec {
            columns: vec![col("label", "u4", None), col("volume_ml", "f4", None)],
            rows: 8,
            row_index: None,
        };
        let roi_data: TableData = vec![
            ("label".into(), ColumnData::U32((0..8u32).collect())),
            (
                "volume_ml".into(),
                ColumnData::F32((0..8).map(|k| k as f32 * 1.25 + 0.5).collect()),
            ),
        ];
        let mut b = ProductBuilder::new("recon", "recon-with-roi", "volume + ROIs", TS);
        let vol_pl = push_array(&mut b, "volume", &vol_spec, ramp_i16(16 * 16 * 16));
        let roi_pl = push_table(&mut b, "roi", &roi_spec, roi_data);
        b.with_field("modality", serde_json::json!("CT"));
        b.with_study("DUPLET-DP06-exam");
        b.add_source(Source::new("derived_from", "recon-int16"));
        b.with_extra("vendor_note", serde_json::json!("synthetic fixture"));
        out.push(Fixture {
            name: "multiblock_study",
            manifest: b.seal().unwrap(),
            payloads: vec![vol_pl, roi_pl],
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
