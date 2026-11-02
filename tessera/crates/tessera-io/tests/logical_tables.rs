//! ADR-0064 PR B (#710): every `tessera-io` entry point that resolves a table name goes through
//! `tessera-core`'s one resolver and refuses an invalid partitioned table with its typed variant.

use tessera_core::block::partition::Partition;
use tessera_core::block::table::{Column, TableSpec};
use tessera_core::block::{BlockRef, LogicalTableError, TableGroup};
use tessera_core::manifest::Manifest;
use tessera_core::Error;
use tessera_io::table::table_block;
use tessera_io::{pack, BlockPayload, ColumnData, Reader, TableData};

const TS: &str = "2024-01-01T00:00:00Z";

fn col(name: &str, dtype: &str) -> Column {
    Column::new(name, dtype)
}

/// Leaves `pairs/0000` and `pairs/0002`: what an old binary's `commit --remove-block pairs/0001`
/// seals (ADR-0064 §6, "Known limit"), a `pairs` node with a gap at index 1.
fn gap_leaves() -> Vec<(BlockRef, BlockPayload)> {
    [(0, false), (2, true)]
        .into_iter()
        .map(|(index, last)| {
            let spec = TableSpec {
                columns: vec![col("t", "u8")],
                rows: 3,
                row_index: None,
                partition: Some(Partition { index, last }),
            };
            let data: TableData = vec![("t".into(), ColumnData::U64(vec![1, 2, 3]))];
            table_block(&format!("pairs/{index:04}"), &spec, &data).unwrap()
        })
        .collect()
}

#[track_caller]
fn assert_gap(e: Error) {
    match e {
        Error::LogicalTable { name, reason } => {
            assert_eq!(
                (name.as_str(), reason),
                ("pairs", LogicalTableError::Gap { missing: 1 })
            )
        }
        other => panic!("expected the typed gap of 'pairs', got {other:?}"),
    }
}

/// The invalid node is refused with its variant by every reader entry point that resolves `pairs`
/// (`Reader::table_group`, `Reader::logical_table`); each leaf still reads by its full name.
#[test]
fn an_invalid_node_is_refused_with_its_variant_by_every_entry_point() {
    let root = tempfile::tempdir().unwrap();
    let leaves = gap_leaves();
    let mut m = Manifest::new("listmode", "gap", "d", TS);
    m.blocks = leaves.iter().map(|(r, _)| r.clone()).collect();
    m.content_hash = Some(m.recompute_content_hash().unwrap());
    m.manifest_hash = Some(m.compute_manifest_hash().unwrap());
    let tsra = root.path().join("gap.tsra");
    let payloads: Vec<BlockPayload> = leaves
        .iter()
        .map(|(_, p)| BlockPayload::new(p.name.clone(), p.bytes.clone()))
        .collect();
    pack(&m, &payloads, &tsra).unwrap();

    let mut r = Reader::open(&tsra).unwrap();
    assert_gap(r.table_group("pairs").unwrap_err());
    assert_gap(r.logical_table("pairs").err().unwrap());
    for leaf in ["pairs/0000", "pairs/0002"] {
        r.read_block(leaf).unwrap();
        assert!(matches!(r.table_group(leaf), Ok(TableGroup::Single(b)) if b == leaf));
    }
}
