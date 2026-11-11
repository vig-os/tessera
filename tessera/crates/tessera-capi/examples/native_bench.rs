//! The native-Rust counterpart of `cpp/bench.cpp`: the same rows and columns through
//! `StreamWriter::push_table`, with every row-group built before the clock starts.
//!
//!   cargo run --release -p tessera-capi --example native_bench -- <stage_dir> <out.tsra> <rows> <workers>
//!
//! Prints one JSON line in the shape `capi_bench` prints.

#[path = "../tests/common/mod.rs"]
mod common;

use std::path::Path;
use std::time::Instant;

use tessera_io::stream::StreamWriter;
use tessera_io::table::ROWS_PER_GROUP;
use tessera_io::WriteSession;

/// The process's peak resident set (`VmHWM`), in MiB.
fn max_rss_mb() -> f64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("VmHWM:"))
                .and_then(|v| v.trim().trim_end_matches("kB").trim().parse::<f64>().ok())
        })
        .map_or(f64::NAN, |kb| kb / 1024.0)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 5 {
        eprintln!("usage: native_bench <stage_dir> <out.tsra> <rows> <workers>");
        std::process::exit(2);
    }
    let rows: u64 = args[3].parse().expect("rows");
    let workers: usize = args[4].parse().expect("workers");
    let groups = common::native_groups(rows, ROWS_PER_GROUP as u64);

    let session = WriteSession::create(
        Path::new(&args[1]).join("session").as_path(),
        "listmode",
        "capi-test",
        "written through the C ABI",
        common::TS,
    )
    .expect("session");
    let t0 = Instant::now();
    let mut sw = StreamWriter::new(session, workers, 4);
    sw.push_table("events", common::native_spec(rows), groups)
        .expect("push_table");
    let manifest = sw.finish(Path::new(&args[2])).expect("finish");
    let secs = t0.elapsed().as_secs_f64();
    let raw_mb = rows as f64 * 24.125 / 1e6;
    println!(
        "{{\"impl\":\"native\",\"rows\":{rows},\"batch_rows\":{ROWS_PER_GROUP},\"producers\":1,\"workers\":{workers},\
         \"seconds\":{secs:.3},\"rows_per_s\":{:.0},\"raw_mb_per_s\":{:.1},\"max_rss_mb\":{:.1},\"content_hash\":\"{}\"}}",
        rows as f64 / secs,
        raw_mb / secs,
        max_rss_mb(),
        manifest.content_hash.unwrap_or_default()
    );
}
