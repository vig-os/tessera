//! GE-HDF5 vendor-raw ingest (#208) — read GE Discovery MI Gen2 listmode coincidence events
//! (an HDF5 **compound** dataset, row-major) into a Tessera `listmode` product, **transposing to
//! columnar** on the way in (the fd5 #193 fix: a single-column projection on a row-major compound
//! costs a full-table read; Vortex columns don't). Lossless: native dtypes preserved.
//!
//! The 3-photon record layout (verified via `h5ls -v` on real DUPLET data): `ms` u32 (timestamp),
//! `id[3]` u16 (crystal ids), `en[3]` f32 (energies), `vtx[3]` f32 (vertex), `lt` f32 (o-Ps
//! lifetime) — 38 B/record. libhdf5 is found via pkg-config (see flake.nix; HDF5_DIR unset).

use hdf5::H5Type;
use hdf5_metno as hdf5;
use tessera_core::block::table::{Column, TableSpec};
use tessera_core::manifest::Manifest;
use tessera_core::{Error, ProductBuilder, Result};
use tessera_io::table::{self, ColumnData, TableData};
use tessera_io::BlockPayload;

/// The GE listmode 3-photon coincidence record (compound, 38 B). `#[repr(C)]` + `H5Type` mirror the
/// on-disk member names/types; HDF5 maps by member name on read.
#[repr(C)]
#[derive(H5Type, Clone, Copy)]
struct Rec3p {
    ms: u32,
    id: [u16; 3],
    en: [f32; 3],
    vtx: [f32; 3],
    lt: f32,
}

fn he(e: impl std::fmt::Display) -> Error {
    Error::Invalid(format!("ge-hdf5: {e}"))
}

/// Read a GE listmode 3-photon dataset (default name `events_3p`) and **transpose** it to flat
/// Tessera columns: `ms` (u4) · `id_0..2` (u2) · `en_0..2` (f4) · `vtx_0..2` (f4) · `lt` (f4).
pub fn read_events_3p(path: &std::path::Path, dataset: &str) -> Result<TableData> {
    let file = hdf5::File::open(path).map_err(he)?;
    let recs: Vec<Rec3p> = file
        .dataset(dataset)
        .map_err(he)?
        .read_raw::<Rec3p>()
        .map_err(he)?;
    let n = recs.len();
    let mut ms = Vec::with_capacity(n);
    let mut id: [Vec<u16>; 3] = Default::default();
    let mut en: [Vec<f32>; 3] = Default::default();
    let mut vtx: [Vec<f32>; 3] = Default::default();
    let mut lt = Vec::with_capacity(n);
    for r in &recs {
        ms.push(r.ms);
        lt.push(r.lt);
        for k in 0..3 {
            id[k].push(r.id[k]);
            en[k].push(r.en[k]);
            vtx[k].push(r.vtx[k]);
        }
    }
    let mut cols: TableData = vec![("ms".into(), ColumnData::U32(ms))];
    for (k, v) in id.into_iter().enumerate() {
        cols.push((format!("id_{k}"), ColumnData::U16(v)));
    }
    for (k, v) in en.into_iter().enumerate() {
        cols.push((format!("en_{k}"), ColumnData::F32(v)));
    }
    for (k, v) in vtx.into_iter().enumerate() {
        cols.push((format!("vtx_{k}"), ColumnData::F32(v)));
    }
    cols.push(("lt".into(), ColumnData::F32(lt)));
    Ok(cols)
}

/// Build a sealed Tessera `listmode` product (Vortex table) from flattened GE columns, with an
/// `ingested_from` provenance edge to the source `.h5`. `row_index` is `ms`.
pub fn to_listmode_product(
    cols: &TableData,
    name: &str,
    timestamp: &str,
    source: &str,
) -> Result<(Manifest, Vec<BlockPayload>)> {
    let rows = cols.first().map(|(_, c)| c.len()).unwrap_or(0) as u64;
    let columns: Vec<Column> = cols
        .iter()
        .map(|(n, c)| Column {
            name: n.clone(),
            dtype: c.numpy_code().into(),
            codec: None,
        })
        .collect();
    let spec = TableSpec {
        columns,
        rows,
        row_index: Some("ms".into()),
    };
    let (block_ref, payload) = table::table_block("events", &spec, cols)?;
    let mut b = ProductBuilder::new("listmode", name, "GE listmode 3-photon events", timestamp);
    b.add_block_ref(block_ref);
    b.add_source(tessera_core::provenance::Source::new(
        "ingested_from",
        source,
    ));
    let sealed = b.seal()?;
    Ok((sealed, vec![payload]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_synth_3p(path: &std::path::Path, n: usize) {
        let recs: Vec<Rec3p> = (0..n)
            .map(|k| Rec3p {
                ms: k as u32,
                id: [k as u16, k as u16 + 1, k as u16 + 2],
                en: [511.0, 510.0 + k as f32, 512.0],
                vtx: [0.1 * k as f32, 0.2, 0.3],
                lt: 1.5 + k as f32 * 0.01,
            })
            .collect();
        let f = hdf5::File::create(path).unwrap();
        f.new_dataset::<Rec3p>()
            .shape(n)
            .create("events_3p")
            .unwrap()
            .write(&recs)
            .unwrap();
    }

    #[test]
    fn ingest_3p_transposes_to_columns_and_builds_listmode() {
        let dir = tempfile::tempdir().unwrap();
        let h5 = dir.path().join("LIST_events.h5");
        write_synth_3p(&h5, 100);

        let cols = read_events_3p(&h5, "events_3p").unwrap();
        // 1 ms + 3 id + 3 en + 3 vtx + 1 lt = 11 columns, each 100 rows
        assert_eq!(cols.len(), 11);
        assert!(cols.iter().all(|(_, c)| c.len() == 100));
        let names: Vec<&str> = cols.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names[0], "ms");
        assert!(
            names.contains(&"id_2")
                && names.contains(&"en_0")
                && names.contains(&"vtx_1")
                && names.contains(&"lt")
        );
        // ms is the monotonic event index
        let ColumnData::U32(ms) = &cols[0].1 else {
            panic!()
        };
        assert_eq!(ms[0], 0);
        assert_eq!(ms[99], 99);

        let (sealed, payloads) = to_listmode_product(
            &cols,
            "DP06-lm",
            "2024-01-01T00:00:00Z",
            "LIST0000_events.h5",
        )
        .unwrap();
        assert_eq!(sealed.product, "listmode");
        assert_eq!(sealed.metadata.get("modality"), None);
        assert_eq!(sealed.sources[0].reference, "LIST0000_events.h5");

        let out = dir.path().join("lm.tsra");
        tessera_io::pack(&sealed, &payloads, &out).unwrap();
        let mut r = tessera_io::Reader::open(&out).unwrap();
        // round-trips through Vortex: decode the events table back
        let bytes = r.read_block("events").unwrap();
        let back = table::decode(
            &tessera_core::block::table::TableSpec {
                columns: cols
                    .iter()
                    .map(|(n, c)| Column {
                        name: n.clone(),
                        dtype: c.numpy_code().into(),
                        codec: None,
                    })
                    .collect(),
                rows: 100,
                row_index: Some("ms".into()),
            },
            &bytes,
        )
        .unwrap();
        assert_eq!(back, cols);
    }
}
