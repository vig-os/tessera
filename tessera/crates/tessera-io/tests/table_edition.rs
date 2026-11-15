//! ADR-0024's 2026-11-15 amendment (#635): every table block the writer seals records the Vortex
//! edition it was encoded under, in the sealed `TableSpec::edition`, and a reader gets it back out
//! of the product. A product sealed before the field existed is covered by
//! `logical_tables::the_legacy_fixture_reads_identically`.

use std::path::Path;

use tessera_core::block::table::{Column, TableSpec};
use tessera_core::manifest::Manifest;
use tessera_core::product::ProductBuilder;
use tessera_core::Error;
use tessera_io::stream::StreamWriter;
use tessera_io::table::{table_block, ROWS_PER_GROUP, TABLE_EDITION};
use tessera_io::{pack, ColumnData, Reader, TableData, TableMultiBlockSink, WriteSession};

const TS: &str = "2024-01-01T00:00:00Z";

/// The edition the pinned writer records. Spelled out, not derived from [`TABLE_EDITION`], so a
/// change of the constant shows up here as the format event it is.
const EDITION: &str = "core2026.08.3";

fn spec(rows: u64) -> TableSpec {
    TableSpec {
        columns: vec![Column::new("t", "u8"), Column::new("e", "f4")],
        rows,
        row_index: Some("t".into()),
        partition: None,
        edition: None,
    }
}

fn rows(n: u64) -> TableData {
    vec![
        ("t".into(), ColumnData::U64((0..n).collect())),
        (
            "e".into(),
            ColumnData::F32((0..n).map(|k| 511.0 + (k % 7) as f32).collect()),
        ),
    ]
}

/// Every table block of the product at `path`, as a reader sees it: the raw `edition` key of each
/// stored spec, and the spec parsed back.
fn read_editions(path: &Path) -> Vec<(serde_json::Value, Option<String>)> {
    let r = Reader::open(path).unwrap();
    r.manifest()
        .blocks
        .iter()
        .map(|b| {
            let parsed: TableSpec = serde_json::from_value(b.spec.clone()).unwrap();
            (b.spec["edition"].clone(), parsed.edition)
        })
        .collect()
}

#[test]
fn table_edition_constant_is_the_recorded_name() {
    assert_eq!(TABLE_EDITION.to_string(), EDITION);
}

/// The in-memory path ([`table_block`]): the sealed spec names the edition, the caller's spec is
/// left as it was, and the edition survives seal, pack, open (which re-verifies every hash) and
/// parse.
#[test]
fn a_single_table_block_records_the_writer_edition() {
    let dir = tempfile::tempdir().unwrap();
    let input = spec(5);
    let (block_ref, payload) = table_block("events", &input, &rows(5)).unwrap();
    assert_eq!(input.edition, None, "the caller's spec is not mutated");
    let mut b = ProductBuilder::new("listmode", "edition", "d", TS);
    b.add_block_ref(block_ref);
    let sealed: Manifest = b.seal().unwrap();
    let path = dir.path().join("single.tsra");
    pack(&sealed, &[payload], &path).unwrap();

    assert_eq!(
        read_editions(&path),
        vec![(serde_json::json!(EDITION), Some(EDITION.into()))]
    );
}

/// The staged path (a partitioned table through `TableMultiBlockSink`, as GE streaming ingest
/// drives it): every leaf records the edition.
#[test]
fn every_partition_leaf_records_the_writer_edition() {
    let dir = tempfile::tempdir().unwrap();
    let ws = WriteSession::create(&dir.path().join("ws"), "listmode", "edition", "d", TS).unwrap();
    let mut sw = StreamWriter::new(ws, 2, 4);
    {
        let mut sink = TableMultiBlockSink::with_block_rows(
            spec(0).columns,
            "events",
            &dir.path().join("sink"),
            &mut sw,
            ROWS_PER_GROUP as u64,
        )
        .unwrap()
        .with_row_index("t");
        sink.push(rows(2 * ROWS_PER_GROUP as u64 + 5)).unwrap();
        assert_eq!(sink.finish().unwrap(), 3);
    }
    let path = dir.path().join("leaves.tsra");
    sw.finish(&path).unwrap();

    let editions = read_editions(&path);
    assert_eq!(editions.len(), 3, "three leaves");
    for e in editions {
        assert_eq!(e, (serde_json::json!(EDITION), Some(EDITION.into())));
    }
}

/// A spec that names another edition is refused rather than sealed over bytes this writer did not
/// encode under it. One that names the writer's own edition is accepted.
#[test]
fn a_spec_naming_another_edition_is_refused() {
    let foreign = TableSpec {
        edition: Some("core2025.06.0".into()),
        ..spec(3)
    };
    match table_block("events", &foreign, &rows(3)) {
        Err(Error::Codec(msg)) => assert!(msg.contains("core2025.06.0"), "{msg}"),
        other => panic!("expected a codec error, got {:?}", other.map(|(r, _)| r)),
    }
    let own = TableSpec {
        edition: Some(EDITION.into()),
        ..spec(3)
    };
    let (block_ref, _) = table_block("events", &own, &rows(3)).unwrap();
    assert_eq!(block_ref.spec["edition"], EDITION);
}

/// The streamed paths refuse a foreign edition when the table is opened, before any row is staged
/// or any group is pulled from the input.
#[test]
fn push_table_and_open_table_refuse_another_edition_up_front() {
    let dir = tempfile::tempdir().unwrap();
    let ws = WriteSession::create(&dir.path().join("ws"), "listmode", "edition", "d", TS).unwrap();
    let mut sw = StreamWriter::new(ws, 2, 4);
    let foreign = |rows| TableSpec {
        edition: Some("core2025.06.0".into()),
        ..spec(rows)
    };
    let groups = std::iter::from_fn(|| -> Option<TableData> {
        panic!("a refused table must not pull its input")
    });
    match sw.push_table("events", foreign(3), groups) {
        Err(Error::Codec(msg)) => assert!(msg.contains("core2025.06.0"), "{msg}"),
        other => panic!("push_table: expected a codec error, got {other:?}"),
    }
    match sw.open_table("other", foreign(0)) {
        Err(Error::Codec(msg)) => assert!(msg.contains("core2025.06.0"), "{msg}"),
        Err(e) => panic!("open_table: expected a codec error, got {e:?}"),
        Ok(_) => panic!("open_table: expected a codec error, got a table"),
    }
}
