//! #635 size probe: the sealed table-block size of representative columns, one column per table so
//! each number is attributable. Uses only `tessera_io::table::encode` and the core table types, so the
//! same file builds against the vortex 0.75 base and the vortex 0.93 head. Sizes are a pure function
//! of the data and the encoder, so they are machine-independent (unlike the bench timings).
use std::io::Write;

use tessera_core::block::table::{Column, TableSpec};
use tessera_io::table::{encode, ColumnData, TableData};

fn col(name: &str, dtype: &str) -> Column {
    Column {
        name: name.into(),
        dtype: dtype.into(),
        codec: None,
        ..Default::default()
    }
}

/// xorshift64, so the inputs are identical in both builds.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

fn main() {
    let n = 200_000usize;
    let mut r = Rng(0x9e37_79b9_7f4a_7c15);
    let words = ["alpha", "beta", "gamma", "delta", "pet", "ct", "lor", "coinc", "single", "prompt"];
    let mut cases: Vec<(&str, &str, ColumnData, usize)> = Vec::new();

    // #628: string-heavy. JSON text per row, and a low-cardinality categorical.
    let json: Vec<String> = (0..n)
        .map(|k| {
            format!(
                r#"{{"event":{k},"kind":"{}","det":{},"e_kev":{:.3},"tags":["{}","{}"]}}"#,
                words[(r.next() % 10) as usize],
                r.next() % 4096,
                r.unit() * 1000.0,
                words[(r.next() % 10) as usize],
                words[(r.next() % 10) as usize]
            )
        })
        .collect();
    let json_raw = json.iter().map(String::len).sum();
    cases.push(("str_json_text (#628)", "str", ColumnData::Utf8(json), json_raw));
    let cat: Vec<String> = (0..n).map(|_| words[(r.next() % 10) as usize].to_string()).collect();
    let cat_raw = cat.iter().map(String::len).sum();
    cases.push(("str_categorical", "str", ColumnData::Utf8(cat), cat_raw));

    // #380: floats (Pco).
    cases.push(("f64_smooth_sin (#380)", "f8", ColumnData::F64((0..n).map(|k| (k as f64 * 1e-3).sin()).collect()), n * 8));
    let mut x = 0.0f64;
    let walk = (0..n).map(|_| { x += r.unit() - 0.5; x }).collect();
    cases.push(("f64_random_walk (#380)", "f8", ColumnData::F64(walk), n * 8));
    cases.push(("f64_uniform_noise", "f8", ColumnData::F64((0..n).map(|_| r.unit()).collect()), n * 8));
    cases.push(("f32_energy_kev", "f4", ColumnData::F32((0..n).map(|_| (511.0 + (r.unit() - 0.5) * 80.0) as f32).collect()), n * 4));

    // listmode integers.
    let mut t = 0u64;
    let ts = (0..n).map(|_| { t += 1 + r.next() % 50; t }).collect();
    cases.push(("u64_timestamps_jitter", "u8", ColumnData::U64(ts), n * 8));
    cases.push(("u16_detector_id", "u2", ColumnData::U16((0..n).map(|_| (r.next() % 4096) as u16).collect()), n * 2));
    let mut outl: Vec<i64> = (0..n as i64).map(|k| k % 7).collect();
    for k in (0..n).step_by(9973) {
        outl[k] = 1 << 40;
    }
    cases.push(("i64_small_with_outliers (patches)", "i8", ColumnData::I64(outl), n * 8));

    // The report is this program's output, written to stdout for the job to collect.
    let mut out = std::io::stdout().lock();
    writeln!(out, "case\traw_bytes\tsealed_bytes\tratio").expect("write report");
    for (name, dtype, data, raw) in cases {
        let spec = TableSpec { columns: vec![col("c", dtype)], rows: n as u64, row_index: None, partition: None };
        let table: TableData = vec![("c".into(), data)];
        let bytes = encode(&spec, &table).expect("encode");
        writeln!(out, "{name}\t{raw}\t{}\t{:.4}", bytes.len(), bytes.len() as f64 / raw as f64)
            .expect("write report");
    }
}
