//! "v1 tables still read" (owner, #828, 2026-11-19; design note #915 §2.3): `tessera-table-v2`
//! changes which encodings the writer emits, never how a reader decodes. Decoding is by Vortex wire
//! ID, so a payload written by `tessera-table-v1` must still decode bit-exactly through
//! [`tessera_io::table::decode`], and v2 must leave the `b1`/`utf8` codec set of v1 untouched.
//!
//! The two payloads under `tests/fixtures/` were written ONCE by the `tessera-table-v1` writer
//! (`tessera_io::table::encode` at dev `4863a788`, the last v1 commit) from [`mixed`] and
//! [`strings`] below. They are frozen: never regenerate them. A v2 writer cannot reproduce them, and
//! that is the point.

use tessera_core::block::table::{Column, TableSpec};
use tessera_io::table::{decode, encode};
use tessera_io::{ColumnData, TableData};

/// Two zones (8,192 rows each) and a partial third: enough to cross a zone boundary while keeping
/// the frozen fixtures small. Row-group crossing is covered by the v2 tests, which need no fixture.
const ROWS: usize = 2 * 8_192 + 3;

fn spec(columns: Vec<Column>) -> TableSpec {
    TableSpec {
        columns,
        rows: ROWS as u64,
        row_index: None,
        partition: None,
        edition: None,
        codec_profile: None,
    }
}

/// Every numeric dtype class v2 routes differently (≥16-bit Pco, 8-bit Zstd), a nullable
/// column, float specials, and the `b1`/`utf8` columns v2 leaves on BtrBlocks.
fn mixed() -> (TableSpec, TableData) {
    let specials = [
        f64::from_bits(0x7ff8_0000_0000_beef), // NaN with a payload
        f64::INFINITY,
        f64::NEG_INFINITY,
        -0.0,
        f64::from_bits(1), // smallest denormal
        i64::MAX as f64,
    ];
    let data: TableData = vec![
        ("t".into(), ColumnData::U64((0..ROWS as u64).collect())),
        (
            "d".into(),
            ColumnData::I16((0..ROWS).map(|k| ((k * 37) % 2000) as i16 - 1000).collect()),
        ),
        (
            "c".into(),
            ColumnData::U8((0..ROWS).map(|k| (k % 40) as u8).collect()),
        ),
        (
            "s".into(),
            ColumnData::I8((0..ROWS).map(|k| ((k * 7) % 256) as u8 as i8).collect()),
        ),
        (
            "e".into(),
            ColumnData::F32((0..ROWS).map(|k| (k % 100) as f32 * 0.25).collect()),
        ),
        (
            "x".into(),
            ColumnData::F64(
                (0..ROWS)
                    .map(|k| match k % 101 {
                        r @ 0..=5 => specials[r],
                        r => r as f64 * 0.5,
                    })
                    .collect(),
            ),
        ),
        (
            "n".into(),
            ColumnData::Nullable {
                values: Box::new(ColumnData::I32(
                    (0..ROWS)
                        .map(|k| if k % 3 == 0 { 0 } else { (k % 1000) as i32 - 5 })
                        .collect(),
                )),
                validity: (0..ROWS).map(|k| k % 3 != 0).collect(),
            },
        ),
        (
            "b".into(),
            ColumnData::Bool((0..ROWS).map(|k| k % 3 == 0).collect()),
        ),
        (
            "name".into(),
            ColumnData::Utf8((0..ROWS).map(|k| format!("det{}", k % 50)).collect()),
        ),
    ];
    let columns = vec![
        Column::new("t", "u8"),
        Column::new("d", "i2"),
        Column::new("c", "u1"),
        Column::new("s", "i1"),
        Column::new("e", "f4"),
        Column::new("x", "f8"),
        Column::new("n", "i4").nullable(),
        Column::new("b", "b1"),
        Column::new("name", "str"),
    ];
    (spec(columns), data)
}

/// Only the dtypes v2 keeps on v1's codec set: a low-cardinality and a high-cardinality string
/// column (dictionary and FSST) and a bool column.
fn strings() -> (TableSpec, TableData) {
    let data: TableData = vec![
        (
            "b".into(),
            ColumnData::Bool((0..ROWS).map(|k| k % 5 < 2).collect()),
        ),
        (
            "tag".into(),
            ColumnData::Utf8((0..ROWS).map(|k| format!("tag-{}", k % 7)).collect()),
        ),
        (
            "uid".into(),
            ColumnData::Utf8((0..ROWS).map(|k| format!("1.2.840.{}", k % 997)).collect()),
        ),
    ];
    let columns = vec![
        Column::new("b", "b1"),
        Column::new("tag", "str"),
        Column::new("uid", "str"),
    ];
    (spec(columns), data)
}

/// Bit equality: floats by their bits (NaN payloads, −0.0), the rest by value.
fn bit_eq(a: &ColumnData, b: &ColumnData) -> bool {
    match (a, b) {
        (ColumnData::F32(x), ColumnData::F32(y)) => x
            .iter()
            .map(|v| v.to_bits())
            .eq(y.iter().map(|v| v.to_bits())),
        (ColumnData::F64(x), ColumnData::F64(y)) => x
            .iter()
            .map(|v| v.to_bits())
            .eq(y.iter().map(|v| v.to_bits())),
        (
            ColumnData::Nullable {
                values: xv,
                validity: xm,
            },
            ColumnData::Nullable {
                values: yv,
                validity: ym,
            },
        ) => xm == ym && bit_eq(xv, yv),
        _ => a == b,
    }
}

fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

/// A frozen `tessera-table-v1` payload decodes bit-exactly under this reader: every column,
/// including the NaN payload, ±inf, −0.0, the denormal and the NULL slots.
#[test]
fn a_frozen_v1_payload_decodes_bit_exactly() {
    let (spec, want) = mixed();
    let got = decode(&spec, &fixture("table_v1_mixed.vortex")).unwrap();
    assert_eq!(got.len(), want.len());
    for ((gn, g), (wn, w)) in got.iter().zip(&want) {
        assert_eq!(gn, wn);
        assert!(
            bit_eq(g, w),
            "column {gn} differs after decoding the v1 payload"
        );
    }
}

/// Every stored chunk of every column of a Vortex table file, each as a fingerprint of its whole
/// encoding tree: `(encoding id, its buffers' bytes, its children's fingerprints)`, recursively.
fn stored_fingerprints(bytes: &[u8]) -> Vec<String> {
    use futures::StreamExt;
    use vortex_array::ArrayRef;
    use vortex_file::{register_default_encodings, OpenOptionsSessionExt};
    use vortex_io::runtime::current::CurrentThreadRuntime;
    use vortex_io::runtime::BlockingRuntime;
    use vortex_io::session::{RuntimeSession, RuntimeSessionExt};

    fn fingerprint(a: &ArrayRef) -> String {
        let buffers: Vec<String> = a
            .buffers()
            .iter()
            .map(|b| blake3::hash(b.as_slice()).to_hex().to_string())
            .collect();
        let children: Vec<String> = a
            .named_children()
            .into_iter()
            .map(|(n, c)| format!("{n}:{}", fingerprint(&c)))
            .collect();
        format!(
            "{}[{}]({})",
            a.encoding_id(),
            buffers.join(","),
            children.join(";")
        )
    }
    let rt = CurrentThreadRuntime::new();
    let s = vortex_session::VortexSession::empty()
        .with::<vortex_array::session::ArraySession>()
        .with::<vortex_layout::session::LayoutSession>()
        .with::<vortex_array::scalar_fn::session::ScalarFnSession>()
        .with::<RuntimeSession>()
        .with_handle(rt.handle());
    register_default_encodings(&s);
    let file = s
        .open_options()
        .open_buffer(vortex_buffer::ByteBuffer::copy_from(bytes))
        .unwrap();
    rt.block_on(
        file.scan()
            .unwrap()
            .into_array_stream()
            .unwrap()
            .collect::<Vec<_>>(),
    )
    .into_iter()
    .map(|c| fingerprint(&c.unwrap()))
    .collect()
}

/// `b1` and `utf8` columns keep v1's codec set under v2 (note §2.1): a table of only these columns
/// stores the same encoding trees with byte-identical buffers as the frozen `tessera-table-v1`
/// payload. The FILE bytes do differ, and only in the footer: a Vortex file serialises the sorted
/// list of every encoding ID its session registers and its edition allows, and each array names its
/// encoding by an index into it, so registering `vortex.zstd` (O1) changes that table and the
/// indices for every table, whichever encodings it uses. The array bytes the codec set produces do
/// not move.
#[test]
fn bool_and_string_columns_keep_v1s_encodings_and_buffers() {
    let (spec, data) = strings();
    let frozen = fixture("table_v1_strings.vortex");
    let now = encode(&spec, &data).unwrap();
    assert_eq!(
        stored_fingerprints(&now),
        stored_fingerprints(&frozen),
        "a b1/utf8 column no longer stores tessera-table-v1's encodings and buffers"
    );
    assert_ne!(
        now, frozen,
        "the file bytes are expected to differ in the footer's encoding-ID table; if they are equal \
         again, vortex.zstd is no longer registered and this test's explanation is stale"
    );
    let got = decode(&spec, &frozen).unwrap();
    for ((gn, g), (_, w)) in got.iter().zip(&data) {
        assert!(
            bit_eq(g, w),
            "column {gn} differs after decoding the v1 payload"
        );
    }
}
