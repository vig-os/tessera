//! Measurement M6 of #828 (design note #915 §8): the 8-bit integer codec of `tessera-table-v2`.
//!
//! ```text
//! cargo run --release -p tessera-io --example table_8bit_codecs -- <dir>
//! ```
//!
//! `<dir>` is a `tessera-bench-events/1` column directory (one `.npy` per column plus
//! `events.json`, the #822 fixture layout). For every `i1`/`u1` column it prints the summed size of
//! `vortex.zstd` at levels 1, 3 and 9 with one frame per 65,536-row group and per 2^22-row block,
//! beside Pco level 8 on the column widened to `u16` and native 8-bit Pco (the standalone `pco`
//! crate with `enable_8_bit`), both per row-group. One JSON object on stdout.

use std::collections::BTreeMap;
use std::path::Path;

use tessera_io::table::{self, BLOCK_ROWS, ROWS_PER_GROUP};

#[path = "table_profile.rs"]
#[allow(dead_code)]
mod table_profile;

fn main() {
    let dir = std::env::args()
        .nth(1)
        .expect("usage: table_8bit_codecs <dir>");
    zstd(Path::new(&dir));
}

fn zstd(dir: &Path) {
    use vortex_array::arrays::PrimitiveArray;
    use vortex_array::validity::Validity;
    use vortex_array::VortexSessionExecute;
    use vortex_buffer::Buffer;

    let s = vortex_session::VortexSession::empty().with::<vortex_array::session::ArraySession>();
    let mut ctx = s.create_execution_ctx();
    let mut rows_total = 0;
    let mut sums: BTreeMap<String, usize> = BTreeMap::new();
    let mut per_col = Vec::new();
    for (name, dtype, path) in table_profile::columns(dir) {
        if dtype != "u1" && dtype != "i1" {
            continue;
        }
        let b = table_profile::npy_payload(&path);
        rows_total = b.len();
        let mut row: BTreeMap<String, usize> = BTreeMap::new();
        for (unit, rows) in [("rg", ROWS_PER_GROUP), ("block", BLOCK_ROWS)] {
            for level in [1, 3, 9] {
                let mut total = 0;
                for chunk in b.chunks(rows) {
                    let p = PrimitiveArray::new(Buffer::copy_from(chunk), Validity::NonNullable);
                    let z = vortex_zstd::ZstdData::from_primitive_without_dict(
                        &p,
                        level,
                        chunk.len(),
                        &mut ctx,
                    )
                    .expect("zstd");
                    total += vortex_zstd::Zstd::try_new(p.dtype().clone(), z, Validity::NonNullable)
                        .expect("zstd array")
                        .nbytes() as usize;
                }
                row.insert(format!("zstd{level}_{unit}"), total);
            }
        }
        let (mut widened, mut native) = (0, 0);
        for chunk in b.chunks(ROWS_PER_GROUP) {
            let wide: Vec<u16> = chunk.iter().map(|&v| u16::from(v)).collect();
            let p = PrimitiveArray::new(Buffer::copy_from(wide.as_slice()), Validity::NonNullable);
            widened += vortex_pco::Pco::from_primitive(
                p.as_view(),
                table::TABLE_V2_PCO_LEVEL,
                table::TABLE_V2_PCO_VALUES_PER_PAGE,
                &mut ctx,
            )
            .expect("pco")
            .nbytes() as usize;
            let cfg = pco::ChunkConfig::default()
                .with_enable_8_bit(true)
                .with_compression_level(table::TABLE_V2_PCO_LEVEL)
                .with_paging_spec(pco::PagingSpec::EqualPagesUpTo(
                    table::TABLE_V2_PCO_VALUES_PER_PAGE,
                ));
            native += pco::standalone::simple_compress(chunk, &cfg)
                .expect("8-bit pco")
                .len();
        }
        row.insert("pco8_widened_u16_rg".into(), widened);
        row.insert("pco8_native_u8_rg".into(), native);
        for (k, v) in &row {
            *sums.entry(k.clone()).or_default() += v;
        }
        per_col.push(serde_json::json!({"column": name, "bytes": row}));
    }
    let per_event: BTreeMap<String, f64> = sums
        .iter()
        .map(|(k, v)| (k.clone(), *v as f64 / rows_total as f64))
        .collect();
    println!(
        "{}",
        serde_json::json!({"cmd": "zstd", "events": rows_total, "columns": per_col, "bytes_per_event_8bit": per_event})
    );
}
