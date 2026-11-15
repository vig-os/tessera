//! ADR-0064 PR B (#710): every `tessera-io` entry point that resolves a table name, or finalises a
//! set of names, goes through `tessera-core`'s one resolver and refuses an invalid partitioned
//! table with its typed variant; and the legacy flat form `events_NNNN` keeps a real product under
//! test after the writers switch (PR D).

use std::path::{Path, PathBuf};

use tessera_core::block::partition::{marker, Partition};
use tessera_core::block::table::{Column, TableSpec};
use tessera_core::block::{BlockRef, LogicalTableError, TableGroup};
use tessera_core::manifest::Manifest;
use tessera_core::Error;
use tessera_io::stream::StreamWriter;
use tessera_io::table::{table_block, ROWS_PER_GROUP};
use tessera_io::{
    pack, pack_dir, unpack, BlockPayload, ColumnData, Reader, TableData, TableMultiBlockSink,
    WriteSession,
};

const TS: &str = "2024-01-01T00:00:00Z";

/// Written by [`write_legacy_events`] before ADR-0064 PR D, so by the writer that names blocks
/// `events_NNNN`. It is frozen: after PR D the same function writes `events/NNNN` with the same
/// `content_hash` (ADR-0064 §6), which [`the_writer_reproduces_the_legacy_fixture_content`] checks.
const LEGACY_FIXTURE: &str = "tests/fixtures/legacy_flat_events.tsra";
const LEGACY_CONTENT_HASH: &str =
    "blake3:c1115417dcf7aba23ee3e9d50c6d193011b9d0201ffad7a528ff03ab572dd39a";
/// Three blocks at `ROWS_PER_GROUP` rows each, the last one partial.
const LEGACY_ROWS: usize = 2 * ROWS_PER_GROUP + 4242;

fn col(name: &str, dtype: &str) -> Column {
    Column::new(name, dtype)
}

/// `t` is the global row index, `e` a deterministic pattern.
fn legacy_rows() -> TableData {
    vec![
        (
            "t".into(),
            ColumnData::U64((0..LEGACY_ROWS as u64).collect()),
        ),
        (
            "e".into(),
            ColumnData::F32((0..LEGACY_ROWS).map(|k| 511.0 + (k % 13) as f32).collect()),
        ),
    ]
}

/// The multi-block writer (`TableMultiBlockSink`, as GE streaming ingest and `bench --input` drive
/// it) over [`legacy_rows`], partitioned through its `with_block_rows` seam.
fn write_legacy_events(dir: &Path) -> (PathBuf, Manifest) {
    let ws = WriteSession::create(&dir.join("ws"), "listmode", "legacy", "d", TS).unwrap();
    let mut sw = StreamWriter::new(ws, 2, 4);
    {
        let mut sink = TableMultiBlockSink::with_block_rows(
            vec![col("t", "u8"), col("e", "f4")],
            "events",
            &dir.join("sink"),
            &mut sw,
            ROWS_PER_GROUP as u64,
        )
        .unwrap()
        .with_row_index("t");
        sink.push(legacy_rows()).unwrap();
        assert_eq!(sink.finish().unwrap(), 3);
    }
    let out = dir.join("legacy.tsra");
    let sealed = sw.finish(&out).unwrap();
    (out, sealed)
}

#[test]
fn the_legacy_fixture_reads_identically() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(LEGACY_FIXTURE);
    let mut r = Reader::open(&path).unwrap();
    r.verify_payloads("legacy").unwrap();
    assert_eq!(
        r.manifest().content_hash.as_deref(),
        Some(LEGACY_CONTENT_HASH)
    );
    let blocks = ["events_0000", "events_0001", "events_0002"].map(|n| n.parse().unwrap());
    assert_eq!(
        r.table_group("events").unwrap(),
        TableGroup::LegacyFlat {
            blocks: blocks.to_vec()
        }
    );
    let view = r.logical_table("events").unwrap();
    assert_eq!(view.block_names(), blocks.map(|b| b.to_string()));
    for (name, want) in legacy_rows() {
        assert_eq!(view.column(&mut r, &name).unwrap(), want, "column {name}");
    }
}

/// ADR-0064 §6: a block's name and marker are in no payload, so the multi-block writer, now writing
/// the marked leaves `events/NNNN`, seals the pre-switch writer's `content_hash` and block digests
/// for the same rows; only the names, the markers and so `manifest_hash` move.
#[test]
fn the_writer_reproduces_the_legacy_fixture_content() {
    let dir = tempfile::tempdir().unwrap();
    let (_, sealed) = write_legacy_events(dir.path());
    assert_eq!(sealed.content_hash.as_deref(), Some(LEGACY_CONTENT_HASH));

    let legacy = Reader::open(&Path::new(env!("CARGO_MANIFEST_DIR")).join(LEGACY_FIXTURE))
        .unwrap()
        .manifest()
        .clone();
    let digests = |m: &Manifest| {
        m.blocks
            .iter()
            .map(|b| b.digest.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(digests(&sealed), digests(&legacy));
    let leaves: Vec<_> = sealed
        .blocks
        .iter()
        .map(|b| (b.name.as_str(), marker(b).unwrap()))
        .collect();
    let leaf = |index, last| Some(Partition { index, last });
    assert_eq!(
        leaves,
        [
            ("events/0000", leaf(0, false)),
            ("events/0001", leaf(1, false)),
            ("events/0002", leaf(2, true)),
        ]
    );
    assert_ne!(sealed.manifest_hash, legacy.manifest_hash);
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
/// (`Reader::table_group`, `Reader::logical_table`) and by every door that finalises the names
/// (`WriteSession::seal`, `unpack`, `pack_dir`), before anything is written; each leaf still reads
/// by its full name.
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

    let out = root.path().join("out");
    assert_gap(unpack(&tsra, &out).unwrap_err());
    assert!(!out.exists(), "unpack wrote before refusing");

    let exploded = root.path().join("exploded");
    std::fs::create_dir_all(exploded.join("blocks/pairs")).unwrap();
    std::fs::write(exploded.join("manifest.json"), m.to_json().unwrap()).unwrap();
    for p in &payloads {
        std::fs::write(exploded.join("blocks").join(p.name.as_str()), &p.bytes).unwrap();
    }
    let repacked = root.path().join("re.tsra");
    assert_gap(pack_dir(&exploded, &repacked).unwrap_err());
    assert!(!repacked.exists(), "pack_dir wrote before refusing");

    let mut ws = WriteSession::create(&root.path().join("ws"), "listmode", "gap", "d", TS).unwrap();
    for (block, payload) in &leaves {
        ws.append_block(block.clone(), &payload.bytes).unwrap();
    }
    let sealed = root.path().join("ws.tsra");
    assert_gap(ws.seal(&sealed).unwrap_err());
    assert!(!sealed.exists(), "WriteSession::seal wrote before refusing");
}
