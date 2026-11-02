//! The partitioned-table fixture the Python tests read (`tests/fixtures/partitioned_pairs.tsra`,
//! ADR-0064 PR C2, #717).
//!
//! No writer produces a partitioned table yet (that is PR D, #712), and the Python `Builder` writes
//! exactly the one block it is given, so the fixture is built here through the public builder API
//! and committed. This test rebuilds it and requires the committed bytes to match, so the file can't
//! drift from the recipe below. Regenerate after an intended change (a codec bump re-encodes the
//! leaves) with `TESSERA_PY_FIXTURE_REGEN=1 cargo test -p tessera-py --test partitioned_fixture`,
//! then update the pinned `content_hash` in `tests/logical_tables.py`.

use std::path::{Path, PathBuf};

use tessera_core::block::partition::{leaf_name, Partition};
use tessera_core::block::table::{Column, TableSpec};
use tessera_core::ProductBuilder;
use tessera_io::{ColumnData, TableData};

/// Rows per leaf of `pairs`: 3 + 3 + 2 = 8 rows, `i` = 0..8 across the leaves.
const LEAF_ROWS: [u64; 3] = [3, 3, 2];

fn fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/partitioned_pairs.tsra")
}

fn col(name: &str, dtype: &str) -> Column {
    Column {
        name: name.into(),
        dtype: dtype.into(),
        codec: None,
        ..Default::default()
    }
}

/// A `table` product with one partitioned table `pairs`: leaves `pairs/0000`, `pairs/0001`,
/// `pairs/0002`, each marked with its index and the last one `last: true`. Column `i` (u64) is the
/// global row index and `x` (f64) is `i / 4`, so a reader that drops or reorders a leaf shows it.
fn build(path: &Path) {
    let mut builder = ProductBuilder::new(
        "table",
        "pairs-fixture",
        "ADR-0064 C2",
        "2026-11-02T00:00:00Z",
    );
    let mut payloads = Vec::new();
    let mut start = 0u64;
    for (index, &rows) in LEAF_ROWS.iter().enumerate() {
        let i: Vec<u64> = (start..start + rows).collect();
        let x = i.iter().map(|&v| v as f64 / 4.0).collect();
        let data: TableData = vec![
            ("i".into(), ColumnData::U64(i)),
            ("x".into(), ColumnData::F64(x)),
        ];
        let spec = TableSpec {
            columns: vec![col("i", "u8"), col("x", "f8")],
            rows,
            row_index: None,
            partition: Some(Partition {
                index: index as u64,
                last: index + 1 == LEAF_ROWS.len(),
            }),
        };
        let name = leaf_name("pairs", index as u64).unwrap();
        let (block_ref, payload) =
            tessera_io::table::table_block(name.as_str(), &spec, &data).unwrap();
        builder.add_block_ref(block_ref);
        payloads.push(payload);
        start += rows;
    }
    let manifest = builder.seal().unwrap();
    tessera_io::pack(&manifest, &payloads, path).unwrap();
}

#[test]
fn committed_fixture_matches_its_recipe() {
    let committed = fixture_path();
    if std::env::var_os("TESSERA_PY_FIXTURE_REGEN").is_some() {
        build(&committed);
    }
    let fresh = Path::new(env!("CARGO_TARGET_TMPDIR")).join("partitioned_pairs.tsra");
    build(&fresh);
    let want = std::fs::read(&fresh).unwrap();
    let have = std::fs::read(&committed).unwrap_or_else(|e| {
        panic!(
            "{}: {e}; regenerate with TESSERA_PY_FIXTURE_REGEN=1",
            committed.display()
        )
    });
    assert!(
        have == want,
        "{} differs from its recipe; regenerate with TESSERA_PY_FIXTURE_REGEN=1 and update the \
         pinned content_hash in tests/logical_tables.py",
        committed.display()
    );
}
