//! `tessera-table-v2` determinism across worker counts (#828, design note #915 §2.5 item 4): a
//! multi-block table with every codec route of the profile (Pco, Zstd, the BtrBlocks set, a
//! nullable column) sealed with 1 and with 4 encode workers has the same `content_hash`. Each chunk
//! is encoded on one thread, so the bytes cannot depend on how many run.

use std::path::Path;

use tessera_core::block::table::Column;
use tessera_io::table::ROWS_PER_GROUP;
use tessera_io::{ColumnData, StreamWriter, TableData, TableMultiBlockSink, WriteSession};

const TS: &str = "2024-01-01T00:00:00Z";

fn columns() -> Vec<Column> {
    vec![
        Column::new("t", "u8"),
        Column::new("c", "u1"),
        Column::new("d", "i2"),
        Column::new("e", "f4"),
        Column::new("n", "i1").nullable(),
        Column::new("tag", "str"),
    ]
}

fn rows(lo: usize, hi: usize) -> TableData {
    vec![
        (
            "t".into(),
            ColumnData::U64((lo as u64..hi as u64).collect()),
        ),
        (
            "c".into(),
            ColumnData::U8((lo..hi).map(|k| (k * 7 % 40) as u8).collect()),
        ),
        (
            "d".into(),
            ColumnData::I16((lo..hi).map(|k| (k * 37 % 2000) as i16 - 1000).collect()),
        ),
        (
            "e".into(),
            ColumnData::F32((lo..hi).map(|k| (k % 97) as f32 * 0.5).collect()),
        ),
        (
            "n".into(),
            ColumnData::Nullable {
                values: Box::new(ColumnData::I8(
                    (lo..hi)
                        .map(|k| if k % 3 == 0 { 0 } else { (k % 100) as i8 })
                        .collect(),
                )),
                validity: (lo..hi).map(|k| k % 3 != 0).collect(),
            },
        ),
        (
            "tag".into(),
            ColumnData::Utf8((lo..hi).map(|k| format!("tag-{}", k % 5)).collect()),
        ),
    ]
}

fn seal(dir: &Path, total: usize, workers: usize) -> String {
    let ws =
        WriteSession::create(&dir.join(format!("ws{workers}")), "listmode", "p", "d", TS).unwrap();
    let mut sw = StreamWriter::new(ws, workers, 8);
    {
        let mut sink = TableMultiBlockSink::with_block_rows(
            columns(),
            "events",
            &dir.join(format!("sink{workers}")),
            &mut sw,
            2 * ROWS_PER_GROUP as u64,
        )
        .unwrap();
        // Misaligned batches, so row-group and block boundaries fall inside a push.
        let mut at = 0;
        while at < total {
            let n = 13_337.min(total - at);
            sink.push(rows(at, at + n)).unwrap();
            at += n;
        }
        sink.finish().unwrap();
    }
    let m = sw.finish(&dir.join(format!("w{workers}.tsra"))).unwrap();
    m.content_hash.unwrap()
}

#[test]
fn a_multi_block_v2_table_seals_the_same_with_1_and_4_workers() {
    let dir = tempfile::tempdir().unwrap();
    let total = 5 * ROWS_PER_GROUP + 1_234; // 3 blocks at 2 row-groups per block
    let one = seal(dir.path(), total, 1);
    assert!(one.starts_with("blake3:"));
    assert_eq!(one, seal(dir.path(), total, 4), "workers 1 vs 4");
}
