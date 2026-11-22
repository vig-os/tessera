//! Measure a table codec profile on real list-mode columns (#828, design note #915 §5 and §8).
//!
//! The input is a directory of one `.npy` file per column plus the `events.json` that names them
//! in order (the `tessera-bench-events/1` layout of the #822 fixtures). Every subcommand prints one
//! JSON object per line, so a run's output is its record.
//!
//! ```text
//! cargo run --release -p tessera-io --example table_profile -- write  <dir> <out.tsra> [workers]
//! cargo run --release -p tessera-io --example table_profile -- verify <dir> <in.tsra>
//! cargo run --release -p tessera-io --example table_profile -- decode <in.tsra> [reps]
//! cargo run --release -p tessera-io --example table_profile -- take   <in.tsra> [reps]
//! cargo run --release -p tessera-io --example table_profile -- layout <in.tsra> [block]
//! ```
//!
//! - `write` (A1, A3, M4): the real streaming writer (`StreamWriter::push_table`, partitioned at
//!   `BLOCK_ROWS`), fed one row-group at a time, with `workers` encode workers (default 1). Prints
//!   the summed table-block bytes per event and the whole-writer rate. Pin it to one core
//!   (`taskset -c N`) for the per-core number.
//! - `verify` (A2): decodes every table block and compares each column bit-exactly with its
//!   source `.npy`, and the blake3 of the decoded column bytes with that of the source.
//! - `decode` (A5, M3): full-scan decode of every block on the calling thread's pool, in MiB/s of
//!   logical column bytes; the median of `reps` (default 3).
//! - `take` (A4, M2): 1, 500 and 5,000 uniform random rows and a 500-row sorted cluster of the
//!   first full block, all columns; the median of `reps` (default 5) in ms.
//! - `layout` (M1): per-column data bytes, zone-map bytes and stored encodings of one block.
//!
//! The subcommands use only the public `tessera_io` API plus Vortex's, so the same file builds
//! against an older writer for a paired run (A3: "same binary except the profile"). The 8-bit
//! codec comparison (M6) is `examples/table_8bit_codecs.rs`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use tessera_core::block::table::{Column, TableSpec};
use tessera_core::block::BlockKind;
use tessera_io::table::{self, BLOCK_ROWS, ROWS_PER_GROUP};
use tessera_io::{ColumnData, Reader, StreamWriter, TableData, WriteSession};

const TS: &str = "2024-01-01T00:00:00Z";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |i: usize| -> &str {
        args.get(i)
            .map(String::as_str)
            .unwrap_or_else(|| panic!("missing argument {i}; see the module docs"))
    };
    let num = |i: usize, default: usize| -> usize {
        args.get(i)
            .map_or(default, |s| s.parse().expect("a number"))
    };
    match arg(0) {
        "write" => write(Path::new(arg(1)), Path::new(arg(2)), num(3, 1)),
        "verify" => verify(Path::new(arg(1)), Path::new(arg(2))),
        "decode" => decode(Path::new(arg(1)), num(2, 3)),
        "take" => take(Path::new(arg(1)), num(2, 5)),
        "layout" => layout(Path::new(arg(1)), args.get(2).map(String::as_str)),
        other => panic!("unknown subcommand {other}; see the module docs"),
    }
}

// ── input ───────────────────────────────────────────────────────────────────────────────────────

/// The columns of `dir`: (name, numpy-style tessera dtype, path), in `events.json` order.
pub fn columns(dir: &Path) -> Vec<(String, String, PathBuf)> {
    let meta: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("events.json")).expect("events.json"))
            .expect("events.json is JSON");
    meta["columns"]
        .as_array()
        .expect("columns")
        .iter()
        .map(|c| {
            let dtype = c["dtype"].as_str().expect("dtype");
            // `<i2` / `|u1` → `i2` / `u1`; the fixtures are little-endian.
            assert!(!dtype.starts_with('>'), "big-endian column {dtype}");
            (
                c["name"].as_str().expect("name").to_owned(),
                dtype.trim_start_matches(['<', '|', '=']).to_owned(),
                dir.join(c["file"].as_str().expect("file")),
            )
        })
        .collect()
}

/// The payload bytes of a little-endian, C-order, 1-D `.npy` (format 1.0 or 2.0).
pub fn npy_payload(path: &Path) -> Vec<u8> {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    assert_eq!(&bytes[..6], b"\x93NUMPY", "{} is not .npy", path.display());
    let (header_len, start) = match bytes[6] {
        1 => (u16::from_le_bytes([bytes[8], bytes[9]]) as usize, 10),
        2 | 3 => (
            u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize,
            12,
        ),
        v => panic!("unsupported .npy version {v}"),
    };
    let header = std::str::from_utf8(&bytes[start..start + header_len]).expect("ascii header");
    assert!(header.contains("'fortran_order': False"), "{header}");
    bytes[start + header_len..].to_vec()
}

fn column_from_le(dtype: &str, b: &[u8]) -> ColumnData {
    macro_rules! le {
        ($t:ty, $n:literal, $v:ident) => {
            ColumnData::$v(
                b.chunks_exact($n)
                    .map(|c| <$t>::from_le_bytes(c.try_into().unwrap()))
                    .collect(),
            )
        };
    }
    match dtype {
        "i1" => ColumnData::I8(b.iter().map(|&v| v as i8).collect()),
        "u1" => ColumnData::U8(b.to_vec()),
        "i2" => le!(i16, 2, I16),
        "u2" => le!(u16, 2, U16),
        "i4" => le!(i32, 4, I32),
        "u4" => le!(u32, 4, U32),
        "i8" => le!(i64, 8, I64),
        "u8" => le!(u64, 8, U64),
        "f4" => le!(f32, 4, F32),
        "f8" => le!(f64, 8, F64),
        d => panic!("unsupported dtype {d}"),
    }
}

/// The little-endian bytes of a decoded numeric column (the `.npy` payload form).
fn column_to_le(c: &ColumnData) -> Vec<u8> {
    match c {
        ColumnData::I8(v) => v.iter().map(|&x| x as u8).collect(),
        ColumnData::U8(v) => v.clone(),
        ColumnData::I16(v) => v.iter().flat_map(|x| x.to_le_bytes()).collect(),
        ColumnData::U16(v) => v.iter().flat_map(|x| x.to_le_bytes()).collect(),
        ColumnData::I32(v) => v.iter().flat_map(|x| x.to_le_bytes()).collect(),
        ColumnData::U32(v) => v.iter().flat_map(|x| x.to_le_bytes()).collect(),
        ColumnData::I64(v) => v.iter().flat_map(|x| x.to_le_bytes()).collect(),
        ColumnData::U64(v) => v.iter().flat_map(|x| x.to_le_bytes()).collect(),
        ColumnData::F32(v) => v.iter().flat_map(|x| x.to_le_bytes()).collect(),
        ColumnData::F64(v) => v.iter().flat_map(|x| x.to_le_bytes()).collect(),
        other => panic!("not a numeric column: {}", other.numpy_code()),
    }
}

fn load(dir: &Path) -> (TableSpec, TableData) {
    let cols = columns(dir);
    let data: TableData = cols
        .iter()
        .map(|(n, d, p)| (n.clone(), column_from_le(d, &npy_payload(p))))
        .collect();
    let rows = data[0].1.len() as u64;
    let spec = TableSpec {
        columns: cols.iter().map(|(n, d, _)| Column::new(n, d)).collect(),
        rows,
        row_index: None,
        partition: None,
        edition: None,
        codec_profile: None,
    };
    (spec, data)
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

// ── the product's table blocks ──────────────────────────────────────────────────────────────────

/// Every table block of `tsra` in manifest order: (name, sealed spec, payload bytes).
fn table_blocks(tsra: &Path) -> Vec<(String, TableSpec, Vec<u8>)> {
    let mut r = Reader::open(tsra).expect("open product");
    let blocks: Vec<_> = r
        .manifest()
        .blocks
        .iter()
        .filter(|b| b.kind == BlockKind::Table)
        .map(|b| (b.name.to_string(), b.spec.clone()))
        .collect();
    blocks
        .into_iter()
        .map(|(name, spec)| {
            let bytes = r.read_block(&name).expect("read block");
            (
                name,
                serde_json::from_value(spec).expect("table spec"),
                bytes,
            )
        })
        .collect()
}

// ── subcommands ─────────────────────────────────────────────────────────────────────────────────

fn write(dir: &Path, out: &Path, workers: usize) {
    let (spec, data) = load(dir);
    let rows = spec.rows as usize;
    let stage = out.with_extension("stage");
    let _ = std::fs::remove_dir_all(&stage);
    let ws = WriteSession::create(&stage, "listmode", "table-profile", "d", TS).expect("session");
    let mut sw = StreamWriter::new(ws, workers, workers * 2);
    // One row-group at a time, sliced from the loaded columns (as a DAQ producer would hand them).
    let groups = (0..rows.div_ceil(ROWS_PER_GROUP)).map(|g| {
        let (lo, hi) = (g * ROWS_PER_GROUP, ((g + 1) * ROWS_PER_GROUP).min(rows));
        data.iter()
            .map(|(n, c)| (n.clone(), c.slice(lo, hi)))
            .collect::<TableData>()
    });
    let t = Instant::now();
    sw.push_table("events", spec.clone(), groups)
        .expect("push_table");
    sw.finish(out).expect("seal");
    let wall = t.elapsed().as_secs_f64();
    let _ = std::fs::remove_dir_all(&stage);
    let table_bytes: usize = table_blocks(out).iter().map(|(_, _, b)| b.len()).sum();
    println!(
        "{}",
        serde_json::json!({
            "cmd": "write",
            "profile": table::TABLE_CODEC_PROFILE,
            "events": rows,
            "workers": workers,
            "blocks": rows.div_ceil(BLOCK_ROWS),
            "table_bytes": table_bytes,
            "file_bytes": std::fs::metadata(out).unwrap().len(),
            "bytes_per_event": table_bytes as f64 / rows as f64,
            "wall_s": wall,
            "mev_per_s": rows as f64 / wall / 1e6,
        })
    );
}

fn verify(dir: &Path, tsra: &Path) {
    let (_, source) = load(dir);
    let blocks = table_blocks(tsra);
    let mut decoded: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for (_, spec, bytes) in &blocks {
        for (name, col) in table::decode(spec, bytes).expect("decode") {
            decoded.entry(name).or_default().extend(column_to_le(&col));
        }
    }
    let mut mismatches = 0;
    let mut cols = Vec::new();
    for (name, col) in &source {
        let want = column_to_le(col);
        let got = decoded.get(name).map(Vec::as_slice).unwrap_or_default();
        let equal = got == want.as_slice();
        let (bw, bg) = (blake3::hash(&want), blake3::hash(got));
        mismatches += usize::from(!equal || bw != bg);
        cols.push(serde_json::json!({
            "column": name, "equal": equal, "blake3_source": bw.to_hex().to_string(),
            "blake3_decoded": bg.to_hex().to_string(),
        }));
    }
    println!(
        "{}",
        serde_json::json!({"cmd": "verify", "blocks": blocks.len(), "mismatches": mismatches, "columns": cols})
    );
    assert_eq!(mismatches, 0, "decoded columns differ from their source");
}

fn decode(tsra: &Path, reps: usize) {
    let blocks = table_blocks(tsra);
    let logical: usize = blocks
        .iter()
        .map(|(_, s, _)| {
            s.columns
                .iter()
                .map(|c| ColumnData::dtype_size(&c.dtype).unwrap_or(0))
                .sum::<usize>()
                * s.rows as usize
        })
        .sum();
    let rows: u64 = blocks.iter().map(|(_, s, _)| s.rows).sum();
    let times: Vec<f64> = (0..reps)
        .map(|_| {
            let t = Instant::now();
            for (_, spec, bytes) in &blocks {
                std::hint::black_box(table::decode(spec, bytes).expect("decode"));
            }
            t.elapsed().as_secs_f64()
        })
        .collect();
    let s = median(times.clone());
    println!(
        "{}",
        serde_json::json!({
            "cmd": "decode", "profile": profile_of(&blocks), "events": rows,
            "logical_bytes": logical, "times_s": times, "median_s": s,
            "mib_per_s": logical as f64 / s / (1 << 20) as f64, "mev_per_s": rows as f64 / s / 1e6,
        })
    );
}

fn take(tsra: &Path, reps: usize) {
    let blocks = table_blocks(tsra);
    let (name, spec, bytes) = blocks
        .iter()
        .find(|(_, s, _)| s.rows as usize == BLOCK_ROWS)
        .unwrap_or(&blocks[0]);
    let n = spec.rows;
    // A fixed linear-congruential sequence: the same rows on every run and every build.
    let mut x: u64 = 0x2545_f491_4f6c_dd1d;
    let mut uniform = |k: usize| -> Vec<u64> {
        let mut v: Vec<u64> = (0..k)
            .map(|_| {
                x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                (x >> 11) % n
            })
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    };
    let cases: Vec<(&str, Vec<u64>)> = vec![
        ("uniform_1", uniform(1)),
        ("uniform_500", uniform(500)),
        ("uniform_5000", uniform(5000)),
        ("cluster_500", (n / 2..n / 2 + 500).collect()),
    ];
    let mut out = serde_json::Map::new();
    for (case, rows) in &cases {
        let times: Vec<f64> = (0..reps)
            .map(|_| {
                let t = Instant::now();
                std::hint::black_box(table::decode_rows(spec, bytes, rows).expect("take"));
                t.elapsed().as_secs_f64() * 1e3
            })
            .collect();
        out.insert(
            (*case).into(),
            serde_json::json!({"rows": rows.len(), "times_ms": times, "median_ms": median(times.clone())}),
        );
    }
    println!(
        "{}",
        serde_json::json!({"cmd": "take", "profile": profile_of(&blocks), "block": name, "block_rows": n, "cases": out})
    );
}

fn profile_of(blocks: &[(String, TableSpec, Vec<u8>)]) -> Option<String> {
    blocks.first().and_then(|(_, s, _)| s.codec_profile.clone())
}

fn layout(tsra: &Path, block: Option<&str>) {
    use futures::StreamExt;
    use vortex_array::arrays::struct_::StructArrayExt;
    use vortex_array::arrays::Struct;
    use vortex_buffer::ByteBuffer;
    use vortex_file::{register_default_encodings, OpenOptionsSessionExt};
    use vortex_io::runtime::current::CurrentThreadRuntime;
    use vortex_io::runtime::BlockingRuntime;
    use vortex_io::session::{RuntimeSession, RuntimeSessionExt};
    use vortex_layout::session::LayoutSession;
    use vortex_session::VortexSession;

    let blocks = table_blocks(tsra);
    let (name, _, bytes) = match block {
        Some(b) => blocks
            .iter()
            .find(|(n, _, _)| n == b)
            .expect("no such block"),
        None => &blocks[0],
    };
    let rt = CurrentThreadRuntime::new();
    let s = VortexSession::empty()
        .with::<vortex_array::session::ArraySession>()
        .with::<LayoutSession>()
        .with::<vortex_array::scalar_fn::session::ScalarFnSession>()
        .with::<RuntimeSession>()
        .with_handle(rt.handle());
    register_default_encodings(&s);
    let file = s
        .open_options()
        .open_buffer(ByteBuffer::copy_from(bytes.as_slice()))
        .expect("open");
    let segments = file.footer().segment_map().clone();
    let root = file.footer().layout().clone();

    /// (data bytes, zone-map bytes, data leaves) under one column's layout.
    fn sizes(
        l: &vortex_layout::LayoutRef,
        in_zones: bool,
        segs: &[vortex_file::SegmentSpec],
        acc: &mut (u64, u64, u64),
    ) {
        for id in l.segment_ids() {
            let len = u64::from(segs[*id as usize].length);
            if in_zones {
                acc.1 += len
            } else {
                acc.0 += len
            }
        }
        if !in_zones && l.encoding_id().to_string() == "vortex.flat" {
            acc.2 += 1;
        }
        for (n, c) in l.child_names().zip(l.children().expect("children")) {
            sizes(&c, in_zones || &*n == "zones", segs, acc);
        }
    }

    // The stored encodings of each column: the root encoding ids of its chunks, counted.
    let chunks: Vec<_> = rt
        .block_on(
            file.scan()
                .unwrap()
                .into_array_stream()
                .unwrap()
                .collect::<Vec<_>>(),
        )
        .into_iter()
        .map(|c| c.expect("chunk"))
        .collect();
    let mut cols = Vec::new();
    for (cname, child) in root.child_names().zip(root.children().unwrap()) {
        let mut acc = (0, 0, 0);
        sizes(&child, false, &segments, &mut acc);
        let mut encodings: BTreeMap<String, usize> = BTreeMap::new();
        for c in &chunks {
            let f = c
                .as_opt::<Struct>()
                .expect("struct chunk")
                .unmasked_field_by_name(&*cname)
                .expect("field")
                .clone();
            *encodings.entry(encoding_tree(&f)).or_default() += 1;
        }
        cols.push(serde_json::json!({
            "column": &*cname, "data_bytes": acc.0, "zone_bytes": acc.1, "data_leaves": acc.2,
            "encodings": encodings,
        }));
    }
    println!(
        "{}",
        serde_json::json!({
            "cmd": "layout", "profile": profile_of(&blocks), "block": name, "block_bytes": bytes.len(),
            "rows": root.row_count(), "columns": cols,
        })
    );
}

/// `encoding(child, …)` of an array, two levels deep: enough to tell FoR+bit-packing from a
/// dictionary or Pco.
fn encoding_tree(a: &vortex_array::ArrayRef) -> String {
    fn go(a: &vortex_array::ArrayRef, depth: usize) -> String {
        let id = a.encoding_id().to_string();
        let kids: Vec<String> = if depth == 0 {
            Vec::new()
        } else {
            a.named_children()
                .into_iter()
                .filter(|(n, _)| n != "validity")
                .map(|(n, c)| format!("{n}={}", go(&c, depth - 1)))
                .collect()
        };
        if kids.is_empty() {
            id
        } else {
            format!("{id}({})", kids.join(", "))
        }
    }
    go(a, 2)
}
