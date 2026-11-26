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
/// `events_NNNN`, and by vortex 0.75. It is frozen: after PR D the same function writes `events/NNNN`
/// with the same rows in the same split (ADR-0064 §6), which
/// [`the_writer_reproduces_the_legacy_fixture_content`] checks, and it keeps a product written by
/// vortex 0.75 under test, which [`the_legacy_fixture_reads_identically`] opens, verifies and reads.
const LEGACY_FIXTURE: &str = "tests/fixtures/legacy_flat_events.tsra";
const LEGACY_CONTENT_HASH: &str =
    "blake3:c1115417dcf7aba23ee3e9d50c6d193011b9d0201ffad7a528ff03ab572dd39a";
/// The fixture's seal. `Reader::open` re-verifies it over the stored manifest bytes, so this pins
/// that the frozen pre-edition fixture itself is unchanged (the byte identity of a pre-edition
/// spec's re-serialisation is `tessera-core`'s `pre_edition_spec_…` test).
const LEGACY_MANIFEST_HASH: &str =
    "blake3:498da59e89047908a03c79a49f155a90cfed3a7cacd9281f3f4472063d49aa31";
/// What [`write_legacy_events`] seals today. It differs from [`LEGACY_CONTENT_HASH`] only because the
/// table codec moved (vortex 0.75 → 0.93, #635), never because of the names.
const WRITER_CONTENT_HASH: &str =
    "blake3:37435470f4526600ea6dedc1fbbbc0a9fdd8c45f4994e56531cf110d8f85a532";
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

/// A product written by vortex 0.75 before the 0.93 table-byte event (ADR-0024's 2026-11-09
/// amendment) still verifies against its sealed digests and reads its rows back. It predates
/// `TableSpec::edition` (the 2026-11-15 amendment) and `TableSpec::codec_profile` (2026-11-16):
/// its specs carry neither key, parse as "none recorded", and `Reader::open` re-verifies `id`,
/// `content_hash` and `manifest_hash` over its manifest unchanged.
#[test]
fn the_legacy_fixture_reads_identically() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(LEGACY_FIXTURE);
    let mut r = Reader::open(&path).unwrap();
    r.verify_payloads("legacy").unwrap();
    for b in &r.manifest().blocks {
        assert!(b.spec.get("edition").is_none(), "{}: {}", b.name, b.spec);
        assert!(
            b.spec.get("codec_profile").is_none(),
            "{}: {}",
            b.name,
            b.spec
        );
        let spec: TableSpec = serde_json::from_value(b.spec.clone()).unwrap();
        assert_eq!(spec.edition, None, "{}", b.name);
        assert_eq!(spec.codec_profile, None, "{}", b.name);
    }
    assert_eq!(
        r.manifest().manifest_hash.as_deref(),
        Some(LEGACY_MANIFEST_HASH)
    );
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
/// the marked leaves `events/NNNN`, writes the legacy fixture's rows in the same split; only the
/// names, the markers and so `manifest_hash` were to move. The fixture was written by vortex 0.75,
/// and vortex 0.93 re-encodes every table block (ADR-0024's 2026-11-09 amendment), so the leaves no
/// longer carry its block digests: their rows and split are compared here, and their bytes are pinned
/// by [`WRITER_CONTENT_HASH`].
#[test]
fn the_writer_reproduces_the_legacy_fixture_content() {
    let dir = tempfile::tempdir().unwrap();
    let (out, sealed) = write_legacy_events(dir.path());
    assert_eq!(sealed.content_hash.as_deref(), Some(WRITER_CONTENT_HASH));

    let legacy_path = Path::new(env!("CARGO_MANIFEST_DIR")).join(LEGACY_FIXTURE);
    let mut legacy_reader = Reader::open(&legacy_path).unwrap();
    let legacy = legacy_reader.manifest().clone();
    let split = |m: &Manifest| {
        m.blocks
            .iter()
            .map(|b| {
                let spec: TableSpec = serde_json::from_value(b.spec.clone()).unwrap();
                (b.kind, spec.rows, spec.row_index)
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(split(&sealed), split(&legacy));
    let mut written = Reader::open(&out).unwrap();
    let view = written.logical_table("events").unwrap();
    let legacy_view = legacy_reader.logical_table("events").unwrap();
    for (name, _) in legacy_rows() {
        assert_eq!(
            view.column(&mut written, &name).unwrap(),
            legacy_view.column(&mut legacy_reader, &name).unwrap(),
            "column {name}"
        );
    }
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
                edition: None,
                codec_profile: None,
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
    let (json, seal) = tessera_io::stored::manifest(&m).unwrap();
    tessera_io::stored::write_pair(&exploded.join("manifest.json"), &json, &seal).unwrap();
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

// #978: use the legacy form for dtype too: its seal previously skipped schema validation.
fn contract_leaves(dtype_mix: bool, legacy: bool) -> Vec<(BlockRef, BlockPayload)> {
    (0..2)
        .map(|index| {
            let dtype = if dtype_mix && index == 1 { "i8" } else { "i4" };
            let nullable = !dtype_mix && index == 1;
            let mut column = col("value", dtype);
            column.nullable = nullable;
            let spec = TableSpec {
                columns: vec![column],
                rows: 1,
                row_index: None,
                partition: (!legacy).then_some(Partition {
                    index,
                    last: index == 1,
                }),
                edition: None,
                codec_profile: None,
            };
            let values = if dtype == "i8" {
                ColumnData::I64(vec![7])
            } else {
                ColumnData::I32(vec![7])
            };
            let values = if nullable {
                ColumnData::Nullable {
                    values: Box::new(values),
                    validity: vec![true],
                }
            } else {
                values
            };
            let name = if legacy {
                format!("events_{index:04}")
            } else {
                format!("events/{index:04}")
            };
            table_block(&name, &spec, &vec![("value".into(), values)]).unwrap()
        })
        .collect()
}

fn assert_contract_error(error: Error) {
    match error {
        Error::LogicalTable {
            name,
            reason: LogicalTableError::SchemaMismatch { leaf, detail },
        } => {
            assert_eq!(name, "events");
            assert!(leaf.ends_with("0001"), "{leaf}");
            assert!(detail.contains("value"), "{detail}");
            assert!(detail.contains("i4"), "{detail}");
        }
        error => panic!("expected column contract refusal, got {error:?}"),
    }
}

fn builder_contract_refusal(dtype_mix: bool) {
    for legacy in [true, false] {
        let mut builder = tessera_core::ProductBuilder::new("test", "contracts", "d", TS);
        for (block, _) in contract_leaves(dtype_mix, legacy) {
            builder.add_block_ref(block);
        }
        assert_contract_error(
            builder
                .seal()
                .expect_err("a mismatched contract was sealed"),
        );
    }
}

fn streaming_contract_refusal(dtype_mix: bool) {
    for legacy in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let ws =
            WriteSession::create(&dir.path().join("ws"), "test", "contracts", "d", TS).unwrap();
        let mut writer = StreamWriter::new(ws, 2, 2);
        for (block, payload) in contract_leaves(dtype_mix, legacy) {
            writer
                .push(Box::new(move || Ok(((block, payload), Vec::new()))))
                .unwrap();
        }
        let out = dir.path().join("invalid.tsra");
        assert_contract_error(
            writer
                .finish(&out)
                .expect_err("a mismatched contract was sealed"),
        );
        assert!(!out.exists());
    }
}

#[test]
fn builder_refuses_nullable_contract_mix() {
    builder_contract_refusal(false);
}
#[test]
fn builder_refuses_dtype_contract_mix() {
    builder_contract_refusal(true);
}
#[test]
fn streaming_refuses_nullable_contract_mix() {
    streaming_contract_refusal(false);
}
#[test]
fn streaming_refuses_dtype_contract_mix() {
    streaming_contract_refusal(true);
}

fn sealed_contract_manifest(leaves: &[(BlockRef, BlockPayload)]) -> Manifest {
    // Model a product sealed before #978, bypassing the builder's new refusal.
    let mut m = Manifest::new("test", "contracts", "d", TS);
    m.blocks = leaves.iter().map(|(b, _)| b.clone()).collect();
    m.content_hash = Some(m.recompute_content_hash().unwrap());
    m.manifest_hash = Some(m.compute_manifest_hash().unwrap());
    m
}

#[test]
fn packers_refuse_previously_sealed_contract_mixes_before_writing() {
    for dtype_mix in [false, true] {
        for legacy in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let leaves = contract_leaves(dtype_mix, legacy);
            let manifest = sealed_contract_manifest(&leaves);
            let payloads = leaves.into_iter().map(|(_, p)| p).collect::<Vec<_>>();
            let out = dir.path().join("invalid.tsra");
            assert_contract_error(pack(&manifest, &payloads, &out).unwrap_err());
            assert!(!out.exists());
            // No fragment is opened: the contract is refused before any output is created.
            let sources = payloads
                .iter()
                .map(|p| (p.name.clone(), Path::new("absent")))
                .collect::<Vec<_>>();
            assert_contract_error(
                tessera_io::pack_streaming(&manifest, &sources, &out).unwrap_err(),
            );
            assert!(!out.exists());
        }
    }
}

#[test]
fn readers_still_refuse_previously_sealed_contract_mixes() {
    use std::io::Write;
    for dtype_mix in [false, true] {
        for legacy in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let leaves = contract_leaves(dtype_mix, legacy);
            let m = sealed_contract_manifest(&leaves);
            let (json, seal) = tessera_io::stored::manifest(&m).unwrap();
            let path = dir.path().join("old.tsra");
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored)
                .last_modified_time(zip::DateTime::default())
                .large_file(true)
                .system(zip::System::Unix);
            let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
            for (name, bytes) in [
                ("mimetype", tessera_io::MIMETYPE.as_bytes()),
                ("manifest.seal", seal.as_bytes()),
                ("manifest.json", json.as_slice()),
            ] {
                zip.start_file(name, options).unwrap();
                zip.write_all(bytes).unwrap();
            }
            for (_, p) in leaves {
                zip.start_file(format!("blocks/{}", p.name), options)
                    .unwrap();
                zip.write_all(&p.bytes).unwrap();
            }
            zip.finish().unwrap();
            let mut reader = Reader::open(&path).unwrap();
            assert_contract_error(reader.logical_table("events").err().unwrap());
        }
    }
}

#[test]
fn push_table_and_open_table_refuse_incompatible_group_contracts() {
    for dtype_mix in [false, true] {
        for open in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let ws =
                WriteSession::create(&dir.path().join("ws"), "test", "contracts", "d", TS).unwrap();
            let mut writer = StreamWriter::new(ws, 2, 2);
            let spec = TableSpec {
                columns: vec![col("value", "i4")],
                rows: 0,
                row_index: None,
                partition: None,
                edition: None,
                codec_profile: None,
            };
            let values = if dtype_mix {
                ColumnData::I64(vec![7; ROWS_PER_GROUP])
            } else {
                ColumnData::Nullable {
                    values: Box::new(ColumnData::I32(vec![7; ROWS_PER_GROUP])),
                    validity: vec![true; ROWS_PER_GROUP],
                }
            };
            let good = vec![("value".into(), ColumnData::I32(vec![7; ROWS_PER_GROUP]))];
            let bad = vec![("value".into(), values)];
            let error = if open {
                let mut table = writer
                    .open_table_with_block_rows("events", spec, ROWS_PER_GROUP as u64)
                    .unwrap();
                table.push(&mut writer, good).unwrap();
                table.push(&mut writer, bad).unwrap_err()
            } else {
                writer
                    .push_table_with_block_rows(
                        "events",
                        TableSpec {
                            rows: (2 * ROWS_PER_GROUP) as u64,
                            ..spec
                        },
                        vec![good, bad],
                        ROWS_PER_GROUP as u64,
                    )
                    .unwrap_err()
            };
            assert_contract_error(error);
        }
    }
}

#[test]
fn single_block_push_table_keeps_maximum_length_bare_names() {
    let dir = tempfile::tempdir().unwrap();
    let ws = WriteSession::create(&dir.path().join("ws"), "test", "long-name", "d", TS).unwrap();
    let mut writer = StreamWriter::new(ws, 1, 1);
    let name = "v".repeat(255);
    let spec = TableSpec {
        columns: vec![col("value", "i4")],
        rows: 1,
        row_index: None,
        partition: None,
        edition: None,
        codec_profile: None,
    };
    writer
        .push_table(
            &name,
            spec,
            vec![vec![("value".into(), ColumnData::I32(vec![7]))]],
        )
        .unwrap();
    let out = dir.path().join("long.tsra");
    writer.finish(&out).unwrap();
    Reader::open(&out).unwrap().logical_table(&name).unwrap();
}

#[test]
fn ingest_multi_block_sink_refuses_incompatible_group_contracts() {
    for dtype_mix in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let ws =
            WriteSession::create(&dir.path().join("ws"), "test", "contracts", "d", TS).unwrap();
        let mut writer = StreamWriter::new(ws, 2, 2);
        let mut sink = TableMultiBlockSink::with_block_rows(
            vec![col("value", "i4")],
            "events",
            &dir.path().join("sink"),
            &mut writer,
            ROWS_PER_GROUP as u64,
        )
        .unwrap();
        sink.push(vec![(
            "value".into(),
            ColumnData::I32(vec![7; ROWS_PER_GROUP]),
        )])
        .unwrap();
        let values = if dtype_mix {
            ColumnData::I64(vec![7])
        } else {
            ColumnData::Nullable {
                values: Box::new(ColumnData::I32(vec![7])),
                validity: vec![true],
            }
        };
        assert_contract_error(sink.push(vec![("value".into(), values)]).unwrap_err());
    }
}

#[test]
fn array_bare_name_next_to_numbered_table_seals_and_reads() {
    use tessera_core::block::array::ArraySpec;
    use tessera_io::array::{array_block, ArrayData};
    let (array, array_payload) = array_block(
        "img",
        &ArraySpec::new(vec![1], "int16"),
        &ArrayData::I16(vec![3]),
    )
    .unwrap();
    let (mut table, table_payload) = contract_leaves(true, true).remove(0);
    table.name = "img_0001".parse().unwrap();
    let table_payload = BlockPayload::new(table.name.clone(), table_payload.bytes);
    let mut builder = tessera_core::ProductBuilder::new("test", "mixed-kinds", "d", TS);
    builder.add_block_ref(array).add_block_ref(table);
    let manifest = builder.seal().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("mixed.tsra");
    pack(&manifest, &[array_payload, table_payload], &out).unwrap();
    let mut reader = Reader::open(&out).unwrap();
    assert_eq!(
        reader.table_group("img_0001").unwrap(),
        TableGroup::Single("img_0001".parse().unwrap())
    );
    reader.logical_table("img_0001").unwrap();
    reader.read_block("img").unwrap();
}
