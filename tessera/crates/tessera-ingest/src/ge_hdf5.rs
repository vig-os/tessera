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

/// The GE listmode 2-photon coincidence record (compound, 30 B; verified via `h5dump -H` on real
/// DUPLET data — a *different* member set from the 3-photon record). This is the bulk dataset
/// (~10⁸ events): `ms` u32 · `en[2]` f32 (energies) · `ax[2]` u8 (axial blocks) · `tx[2]` u16
/// (transaxial crystals) · `vtx[3]` f32 (LOR vertex).
#[repr(C)]
#[derive(H5Type, Clone, Copy)]
struct Rec2p {
    ms: u32,
    en: [f32; 2],
    ax: [u8; 2],
    tx: [u16; 2],
    vtx: [f32; 3],
}

fn he(e: impl std::fmt::Display) -> Error {
    Error::Invalid(format!("ge-hdf5: {e}"))
}

/// Append a fixed-size compound vector member as `prefix_0..N-1` scalar columns (the row-major →
/// columnar transpose), wrapping each column's `Vec<T>` into its [`ColumnData`] variant.
fn flatten<const N: usize, T>(
    cols: &mut TableData,
    prefix: &str,
    arrs: [Vec<T>; N],
    wrap: fn(Vec<T>) -> ColumnData,
) {
    for (k, v) in arrs.into_iter().enumerate() {
        cols.push((format!("{prefix}_{k}"), wrap(v)));
    }
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
    flatten(&mut cols, "id", id, ColumnData::U16);
    flatten(&mut cols, "en", en, ColumnData::F32);
    flatten(&mut cols, "vtx", vtx, ColumnData::F32);
    cols.push(("lt".into(), ColumnData::F32(lt)));
    Ok(cols)
}

/// Read a GE listmode 2-photon dataset (default name `events_2p`) and **transpose** it to flat
/// Tessera columns: `ms` (u4) · `en_0..1` (f4) · `ax_0..1` (u1) · `tx_0..1` (u2) · `vtx_0..2` (f4).
pub fn read_events_2p(path: &std::path::Path, dataset: &str) -> Result<TableData> {
    let file = hdf5::File::open(path).map_err(he)?;
    let recs: Vec<Rec2p> = file
        .dataset(dataset)
        .map_err(he)?
        .read_raw::<Rec2p>()
        .map_err(he)?;
    let n = recs.len();
    let mut ms = Vec::with_capacity(n);
    let mut en: [Vec<f32>; 2] = Default::default();
    let mut ax: [Vec<u8>; 2] = Default::default();
    let mut tx: [Vec<u16>; 2] = Default::default();
    let mut vtx: [Vec<f32>; 3] = Default::default();
    for r in &recs {
        ms.push(r.ms);
        for k in 0..2 {
            en[k].push(r.en[k]);
            ax[k].push(r.ax[k]);
            tx[k].push(r.tx[k]);
        }
        for (k, &v) in r.vtx.iter().enumerate() {
            vtx[k].push(v);
        }
    }
    let mut cols: TableData = vec![("ms".into(), ColumnData::U32(ms))];
    flatten(&mut cols, "en", en, ColumnData::F32);
    flatten(&mut cols, "ax", ax, ColumnData::U8);
    flatten(&mut cols, "tx", tx, ColumnData::U16);
    flatten(&mut cols, "vtx", vtx, ColumnData::F32);
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
    let mut b = ProductBuilder::new(
        "listmode",
        name,
        "GE listmode coincidence events",
        timestamp,
    );
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

    fn write_synth_2p(path: &std::path::Path, n: usize) {
        let recs: Vec<Rec2p> = (0..n)
            .map(|k| Rec2p {
                ms: k as u32,
                en: [511.0, 510.0 + k as f32],
                ax: [(k % 64) as u8, (k % 32) as u8],
                tx: [k as u16, k as u16 + 7],
                vtx: [0.1 * k as f32, 0.2, 0.3],
            })
            .collect();
        let f = hdf5::File::create(path).unwrap();
        f.new_dataset::<Rec2p>()
            .shape(n)
            .create("events_2p")
            .unwrap()
            .write(&recs)
            .unwrap();
    }

    #[test]
    fn ingest_2p_transposes_distinct_member_set_to_columns() {
        let dir = tempfile::tempdir().unwrap();
        let h5 = dir.path().join("LIST_2p.h5");
        write_synth_2p(&h5, 64);

        let cols = read_events_2p(&h5, "events_2p").unwrap();
        // 1 ms + 2 en + 2 ax + 2 tx + 3 vtx = 10 columns, each 64 rows
        assert_eq!(cols.len(), 10);
        assert!(cols.iter().all(|(_, c)| c.len() == 64));
        let by: std::collections::HashMap<&str, &ColumnData> =
            cols.iter().map(|(n, c)| (n.as_str(), c)).collect();
        // the 2p member set differs from 3p: ax (u1) + tx (u2), no id/lt
        assert_eq!(by["ax_0"].numpy_code(), "u1");
        assert_eq!(by["tx_1"].numpy_code(), "u2");
        assert_eq!(by["vtx_2"].numpy_code(), "f4");
        assert!(!by.contains_key("lt") && !by.contains_key("id_0"));
        let ColumnData::U8(ax0) = by["ax_0"] else {
            panic!()
        };
        assert_eq!(ax0[63], 63);

        // round-trips through the Vortex table backend (u8 column included)
        let (sealed, payloads) = to_listmode_product(
            &cols,
            "DP06-2p",
            "2024-01-01T00:00:00Z",
            "LIST0000_events.h5",
        )
        .unwrap();
        let out = dir.path().join("lm2p.tsra");
        tessera_io::pack(&sealed, &payloads, &out).unwrap();
        let mut r = tessera_io::Reader::open(&out).unwrap();
        let bytes = r.read_block("events").unwrap();
        let spec = TableSpec {
            columns: cols
                .iter()
                .map(|(n, c)| Column {
                    name: n.clone(),
                    dtype: c.numpy_code().into(),
                    codec: None,
                })
                .collect(),
            rows: 64,
            row_index: Some("ms".into()),
        };
        assert_eq!(table::decode(&spec, &bytes).unwrap(), cols);
    }
}
