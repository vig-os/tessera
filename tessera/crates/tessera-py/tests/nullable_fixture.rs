//! The nullable-column fixture the Python tests read (`tests/fixtures/nullable_obs.tsra`, #804),
//! and what the Rust reader decodes from it (`tests/fixtures/nullable_obs.expected.json`).
//!
//! The Python `Builder` cannot write a nullable column, so the fixture is built here through the
//! public builder API and committed, as `partitioned_fixture.rs` does for its table. Its table `obs`
//! is partitioned into two leaves of 5 and 6 rows: 11 rows in all, and no leaf a multiple of 8, so a
//! reader that leaves the `ceil(rows/8)`-byte validity trailer on the values cannot pass by luck.
//!
//! The expected file is the **Rust reader's** answer: every column of `obs` read through
//! `LogicalTableView::column` (the read `Reader.column` uses), each as its values' little-endian bytes
//! in hex plus its validity mask. `tests/nullable_columns.py` requires every Python read path to
//! reproduce it bit for bit. Both files are rebuilt here and must equal the committed bytes;
//! regenerate after an intended change with
//! `TESSERA_PY_FIXTURE_REGEN=1 cargo test -p tessera-py --test nullable_fixture`, then update the
//! pinned `content_hash` in `tests/nullable_columns.py`.

mod common;

use std::path::Path;

use common::{assert_committed, col};
use serde_json::{json, Value};
use tessera_core::block::partition::{leaf_name, Partition};
use tessera_core::block::table::TableSpec;
use tessera_core::ProductBuilder;
use tessera_io::{ColumnData, TableData};

/// Rows per leaf of `obs`: 5 + 6 = 11.
const LEAF_ROWS: [u64; 2] = [5, 6];
const ROWS: u64 = 11;
const FIXTURE: &str = "nullable_obs.tsra";
const EXPECTED: &str = "nullable_obs.expected.json";

/// What sits under a NULL in the producer's buffer. Decoding returns the dtype default there
/// (`ColumnData::Nullable`, #659), so the expected file shows `0`, never this.
const UNDER_NULL: f64 = 999.0;

/// Whether global row `g` of each nullable column is present, and its value there.
fn a(g: u64) -> Option<i32> {
    (g % 3 != 1).then(|| 3 * g as i32 - 7)
}
fn e(g: u64) -> Option<f64> {
    match g {
        0 | 9 | 10 => None,
        2 => Some(f64::NAN), // a present NaN, which is not a NULL
        _ => Some(g as f64 * 0.5),
    }
}
fn t(g: u64) -> Option<u16> {
    Some(1000 + g as u16) // declared nullable, holds no NULL
}
fn z(g: u64) -> Option<f32> {
    (g >= LEAF_ROWS[0]).then_some(g as f32 * 1.5) // every row of leaf 0 is NULL
}

fn nullable<T: Copy>(
    rows: std::ops::Range<u64>,
    f: impl Fn(u64) -> Option<T>,
    under: T,
    wrap: impl Fn(Vec<T>) -> ColumnData,
) -> ColumnData {
    let (values, validity) = rows
        .map(|g| f(g).map_or((under, false), |v| (v, true)))
        .unzip();
    ColumnData::Nullable {
        values: Box::new(wrap(values)),
        validity,
    }
}

/// A `table` product with one partitioned table `obs` (leaves `obs/0000`, `obs/0001`): `i` (u4,
/// not nullable) is the global row index; `a` (i4), `e` (f8), `t` (u2) and `z` (f4) are nullable.
fn build(path: &Path) {
    let mut builder = ProductBuilder::new(
        "table",
        "nullable-fixture",
        "#804 nullable columns",
        "2026-11-22T00:00:00Z",
    );
    let mut payloads = Vec::new();
    let mut start = 0u64;
    for (index, &rows) in LEAF_ROWS.iter().enumerate() {
        let r = start..start + rows;
        let data: TableData = vec![
            (
                "i".into(),
                ColumnData::U32(r.clone().map(|g| g as u32).collect()),
            ),
            (
                "a".into(),
                nullable(r.clone(), a, UNDER_NULL as i32, ColumnData::I32),
            ),
            (
                "e".into(),
                nullable(r.clone(), e, UNDER_NULL, ColumnData::F64),
            ),
            (
                "t".into(),
                nullable(r.clone(), t, UNDER_NULL as u16, ColumnData::U16),
            ),
            (
                "z".into(),
                nullable(r, z, UNDER_NULL as f32, ColumnData::F32),
            ),
        ];
        let spec = TableSpec {
            columns: vec![
                col("i", "u4", false),
                col("a", "i4", true),
                col("e", "f8", true),
                col("t", "u2", true),
                col("z", "f4", true),
            ],
            rows,
            row_index: None,
            partition: Some(Partition {
                index: index as u64,
                last: index + 1 == LEAF_ROWS.len(),
            }),
            edition: None,
            codec_profile: None,
        };
        let name = leaf_name("obs", index as u64).unwrap();
        let (block_ref, payload) =
            tessera_io::table::table_block(name.as_str(), &spec, &data).unwrap();
        builder.add_block_ref(block_ref);
        payloads.push(payload);
        start += rows;
    }
    let manifest = builder.seal().unwrap();
    tessera_io::pack(&manifest, &payloads, path).unwrap();
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The Rust reader's view of the fixture at `fixture`: every column of `obs`, in order, through
/// `LogicalTableView::column`, as `{name, dtype, values_le_hex, validity}` (`validity` is `null`
/// for a column that is not nullable).
fn rust_reader_view(fixture: &Path) -> Value {
    let mut reader = tessera_io::Reader::open(fixture).unwrap();
    let view = reader.logical_table("obs").unwrap();
    let names: Vec<String> = view.columns().iter().map(|c| c.name.clone()).collect();
    let columns: Vec<Value> = names
        .iter()
        .map(|name| {
            let column = view.column(&mut reader, name).unwrap();
            assert_eq!(column.len() as u64, ROWS, "column {name}");
            let (values, validity) = column.values_and_validity();
            json!({
                "name": name,
                "dtype": values.numpy_code(),
                "values_le_hex": hex(&values.to_le_bytes()),
                "validity": validity,
            })
        })
        .collect();
    json!({ "table": "obs", "rows": ROWS, "columns": columns })
}

#[test]
fn committed_fixture_matches_its_recipe() {
    assert_committed(
        FIXTURE,
        build,
        "update the pinned content_hash in tests/nullable_columns.py",
    );
}

/// The expected file is what the Rust reader decodes, and that decode is the recipe: each present
/// slot holds the recipe's value (bit for bit, so the present NaN stays a NaN) and each NULL the
/// dtype default.
#[test]
fn committed_expectation_is_the_rust_readers_view() {
    assert_committed(
        EXPECTED,
        |out| {
            let fixture = Path::new(env!("CARGO_TARGET_TMPDIR")).join("nullable_obs.view.tsra");
            build(&fixture);
            let mut text = serde_json::to_string_pretty(&rust_reader_view(&fixture)).unwrap();
            text.push('\n');
            std::fs::write(out, text).unwrap();
        },
        "rerun tests/nullable_columns.py",
    );

    let fixture = Path::new(env!("CARGO_TARGET_TMPDIR")).join("nullable_obs.recipe.tsra");
    build(&fixture);
    let view = rust_reader_view(&fixture);
    let col = |name: &str| {
        view["columns"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == name)
            .unwrap()
            .clone()
    };
    fn want<T: Copy + Default>(
        f: impl Fn(u64) -> Option<T>,
        le: impl Fn(T) -> Vec<u8>,
    ) -> (String, Value) {
        let (bytes, validity): (Vec<Vec<u8>>, Vec<bool>) = (0..ROWS)
            .map(|g| f(g).map_or((le(T::default()), false), |v| (le(v), true)))
            .unzip();
        (hex(&bytes.concat()), json!(validity))
    }
    let check = |name: &str, (values, validity): (String, Value)| {
        let c = col(name);
        assert_eq!(c["values_le_hex"], values.as_str(), "column {name} values");
        assert_eq!(c["validity"], validity, "column {name} validity");
    };
    check("a", want(a, |v: i32| v.to_le_bytes().to_vec()));
    check("e", want(e, |v: f64| v.to_le_bytes().to_vec()));
    check("t", want(t, |v: u16| v.to_le_bytes().to_vec()));
    check("z", want(z, |v: f32| v.to_le_bytes().to_vec()));
    let i = col("i");
    assert_eq!(i["validity"], Value::Null, "`i` is not nullable");
    let index: Vec<u8> = (0..ROWS as u32).flat_map(u32::to_le_bytes).collect();
    assert_eq!(i["values_le_hex"], hex(&index).as_str());
}
