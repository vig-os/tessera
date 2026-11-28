//! #905 commit-cost probe: what one small durable commit costs through the write path tessera has
//! today (`table_block` encode + `WriteSession::append_block`: fragment, directory and journal
//! fsync), for 1, 100 and 10,000 rows per commit, and how that cost moves with the number of commits
//! already in the session. Beside it, as a lower bound and not as a design, a raw row log: the
//! same rows' little-endian column bytes appended to one file with one `sync_data` per commit.
//!
//! Every arm is checked: the tessera arm by `WriteSession::recover` (watermark == commits) and a
//! byte comparison of every committed fragment with a re-encode of the rows it should hold; the row
//! log by replaying the file. `crash` kills a writer process with SIGKILL and checks that every
//! acknowledged commit is recovered.
//!
//! `verdict` reads the records of a run and judges each arm against the budget for small commits
//! (`budget` below), which is proposed as the acceptance line of #905.
//!
//! Output: one JSON object per line on stdout. Timings are context (shared host); the counts of
//! sync calls come from `strace -c` in the job script, not from this program.
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use tessera_core::block::table::TableSpec;
use tessera_io::table::{table_block, ColumnData, TableData};
use tessera_io::WriteSession;

const TS: &str = "2026-11-18T00:00:00Z";

/// The budget for a durable commit of up to `MAX_ROWS` rows from one writer (owner, 2026-11-19;
/// nerdmachines/kit `docs/architecture/data-and-stores.md`, #905). The owner's words: p50 at most
/// 2 ms and p99 at most 10 ms; at least 500 commits per second; cost flat up to 100,000 commits in
/// a session; at most twice the raw row bytes on disk before sealing.
///
/// How it is judged is this probe's reading, not the owner's words: one commit at a time, without
/// group commit; p50 and rate as the median of the trials and p99 as the worst trial; `FLAT_RATIO`
/// for "flat", so the last tenth of the commits may cost `perf-budgets.toml`'s default tolerance
/// more than the first tenth; staged bytes (fragment plus journal) over the rows' raw column bytes.
mod budget {
    pub const MAX_ROWS: u64 = 100;
    pub const P50_NS: u64 = 2_000_000;
    pub const P99_NS: u64 = 10_000_000;
    pub const MIN_COMMITS_PER_S: f64 = 500.0;
    pub const FLAT_COMMITS: u64 = 100_000;
    pub const FLAT_RATIO: f64 = 1.2;
    pub const SIZE_RATIO: f64 = 2.0;
}

/// One record of the report, on stdout for the job to collect.
fn emit(record: &serde_json::Value) {
    writeln!(std::io::stdout().lock(), "{record}").expect("write report");
}

/// xorshift64*, seeded per global row so any row can be regenerated on its own.
fn mix(mut x: u64) -> u64 {
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    x.wrapping_mul(0x2545_f491_4f6c_dd1d)
}

fn text(seed: u64, len: usize) -> String {
    const ALPHA: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 ";
    let mut s = String::with_capacity(len);
    let mut x = seed | 1;
    for _ in 0..len {
        x = mix(x);
        s.push(ALPHA[(x >> 33) as usize % ALPHA.len()] as char);
    }
    s
}

/// The same row shape as the kit harness (nerdmachines/kit#98): about 350 bytes per row.
fn spec(rows: usize) -> TableSpec {
    serde_json::from_value(serde_json::json!({
        "columns": [
            {"name": "key", "dtype": "u8"},
            {"name": "kind", "dtype": "u1"},
            {"name": "state", "dtype": "u1"},
            {"name": "updated", "dtype": "i8"},
            {"name": "title", "dtype": "str"},
            {"name": "body", "dtype": "str"},
        ],
        "rows": rows,
    }))
    .expect("table spec")
}

/// The rows of commit `commit`: global row index `commit * rows + k`.
fn rows_of(commit: usize, rows: usize) -> TableData {
    let base = (commit * rows) as u64;
    let g = |k: usize| base + k as u64;
    vec![
        ("key".into(), ColumnData::U64((0..rows).map(|k| g(k) * 7 + 1).collect())),
        ("kind".into(), ColumnData::U8((0..rows).map(|k| (mix(g(k) + 11) % 5) as u8).collect())),
        ("state".into(), ColumnData::U8((0..rows).map(|k| (mix(g(k) + 23) % 3) as u8).collect())),
        (
            "updated".into(),
            ColumnData::I64((0..rows).map(|k| 1_700_000_000 + g(k) as i64).collect()),
        ),
        ("title".into(), ColumnData::Utf8((0..rows).map(|k| text(g(k) * 2 + 1, 40)).collect())),
        ("body".into(), ColumnData::Utf8((0..rows).map(|k| text(g(k) * 2 + 2, 300)).collect())),
    ]
}

fn block_name(commit: usize) -> String {
    format!("d{commit:07}")
}

struct Stats {
    mean: f64,
    p50: u64,
    p99: u64,
    first_decile_mean: f64,
    last_decile_mean: f64,
}

fn stats(ns: &[u64]) -> Stats {
    let n = ns.len().max(1);
    let mean = ns.iter().sum::<u64>() as f64 / n as f64;
    let dec = (n / 10).max(1);
    let first = ns.iter().take(dec).sum::<u64>() as f64 / dec as f64;
    let last = ns.iter().rev().take(dec).sum::<u64>() as f64 / dec as f64;
    let mut sorted = ns.to_vec();
    sorted.sort_unstable();
    Stats {
        mean,
        p50: sorted[(n - 1) / 2],
        p99: sorted[((n - 1) * 99) / 100],
        first_decile_mean: first,
        last_decile_mean: last,
    }
}

fn stats_json(s: &Stats) -> serde_json::Value {
    serde_json::json!({
        "mean_ns": s.mean.round(), "p50_ns": s.p50, "p99_ns": s.p99,
        "first_decile_mean_ns": s.first_decile_mean.round(),
        "last_decile_mean_ns": s.last_decile_mean.round(),
    })
}

/// The rows' values alone: fixed-width values at their width, strings at their UTF-8 length, no
/// offsets, lengths or framing. The denominator of the budget's size line.
fn raw_row_bytes(data: &TableData) -> u64 {
    data.iter()
        .map(|(_, col)| match col {
            ColumnData::Utf8(v) => v.iter().map(|s| s.len() as u64).sum::<u64>(),
            other => other.to_le_bytes().len() as u64,
        })
        .sum()
}

fn dir_bytes(dir: &Path) -> u64 {
    let mut total = 0;
    if let Ok(entries) = fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                total += dir_bytes(&p);
            } else if let Ok(m) = e.metadata() {
                total += m.len();
            }
        }
    }
    total
}

/// Recover the session in `stage` and compare every committed fragment, byte for byte, with a
/// re-encode of the rows it should hold. Returns (watermark, mismatches, recover seconds).
fn recover_and_check(stage: &Path, rows: usize) -> (usize, usize, f64, WriteSession) {
    let t = Instant::now();
    let ws = WriteSession::recover(stage).expect("recover");
    let recover_s = t.elapsed().as_secs_f64();
    let watermark = ws.watermark();
    let sp = spec(rows);
    let mut mismatches = 0;
    for commit in 0..watermark {
        let name = block_name(commit);
        let (_, want) = table_block(&name, &sp, &rows_of(commit, rows)).expect("re-encode");
        match fs::read(stage.join("blocks").join(&name)) {
            Ok(got) if got == want.bytes => {}
            _ => mismatches += 1,
        }
    }
    (watermark, mismatches, recover_s, ws)
}

/// The existing path: one encoded table block and one `append_block` per commit.
fn run_tessera(dir: &Path, rows: usize, commits: usize, label: &str, seal: bool) {
    let stage = dir.join(format!("stage-{rows}-{commits}"));
    let _ = fs::remove_dir_all(&stage);
    let sp = spec(rows);
    // Rows are generated before the clock starts; the timed part is encode and append.
    let mut ws = WriteSession::create(&stage, "recon", "commit-cost", "probe", TS).expect("create");
    let mut encode_ns = Vec::with_capacity(commits);
    let mut append_ns = Vec::with_capacity(commits);
    let mut payload_bytes = 0u64;
    let mut raw_bytes = 0u64;
    let wall = Instant::now();
    for commit in 0..commits {
        let data = rows_of(commit, rows);
        raw_bytes += raw_row_bytes(&data);
        let t0 = Instant::now();
        let (block_ref, payload) = table_block(&block_name(commit), &sp, &data).expect("encode");
        let t1 = Instant::now();
        ws.append_block(block_ref, &payload.bytes).expect("append");
        let t2 = Instant::now();
        encode_ns.push((t1 - t0).as_nanos() as u64);
        append_ns.push((t2 - t1).as_nanos() as u64);
        payload_bytes += payload.bytes.len() as u64;
    }
    let wall_s = wall.elapsed().as_secs_f64();
    drop(ws); // no seal: the stage dir is what a crash would leave after the last commit

    let journal_bytes = fs::metadata(stage.join("journal.jsonl")).map(|m| m.len()).unwrap_or(0);
    let stage_bytes = dir_bytes(&stage);
    let (watermark, mismatches, recover_s, ws) = recover_and_check(&stage, rows);
    let (seal_s, sealed_bytes) = if seal {
        let out = dir.join(format!("sealed-{rows}-{commits}.tsra"));
        let _ = fs::remove_file(&out);
        let t = Instant::now();
        ws.seal(&out).expect("seal");
        let s = t.elapsed().as_secs_f64();
        let b = fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
        let _ = fs::remove_file(&out);
        (Some(s), Some(b))
    } else {
        drop(ws);
        (None, None)
    };
    let _ = fs::remove_dir_all(&stage);

    let total: Vec<u64> = encode_ns.iter().zip(&append_ns).map(|(a, b)| a + b).collect();
    emit(&serde_json::json!({
        "arm": "tessera_append_block", "medium": label, "rows_per_commit": rows,
        "commits": commits, "wall_s": wall_s,
        "commits_per_s": commits as f64 / total.iter().sum::<u64>() as f64 * 1e9,
        "encode": stats_json(&stats(&encode_ns)),
        "append": stats_json(&stats(&append_ns)),
        "commit_total": stats_json(&stats(&total)),
        "raw_row_bytes_per_commit": raw_bytes / commits as u64,
        "staged_bytes_per_commit": stage_bytes / commits as u64,
        "payload_bytes_per_commit": payload_bytes / commits as u64,
        "journal_bytes_per_commit": journal_bytes / commits as u64,
        "stage_bytes": stage_bytes,
        "recover_s": recover_s, "recovered_commits": watermark,
        "mismatches": mismatches + usize::from(watermark != commits),
        "seal_s": seal_s, "sealed_bytes": sealed_bytes,
    }));
}

fn record_bytes(data: &TableData) -> Vec<u8> {
    let mut payload = Vec::new();
    for (_, col) in data {
        let b = col.to_le_bytes();
        payload.extend_from_slice(&(b.len() as u64).to_le_bytes());
        payload.extend_from_slice(&b);
    }
    payload
}

/// Lower bound, not a design: the rows' column bytes appended to one file, one `sync_data` per
/// commit. Record = u64 length, the payload's digest as text (`blake3:<hex>`), payload.
fn run_rowlog(dir: &Path, rows: usize, commits: usize, label: &str) {
    let path = dir.join(format!("rowlog-{rows}-{commits}.log"));
    let _ = fs::remove_file(&path);
    let mut f = OpenOptions::new().create_new(true).append(true).open(&path).expect("open log");
    f.sync_all().expect("sync new log");
    File::open(dir).and_then(|d| d.sync_all()).expect("sync dir");
    let mut ns = Vec::with_capacity(commits);
    let mut bytes = 0u64;
    let mut raw_bytes = 0u64;
    for commit in 0..commits {
        let data = rows_of(commit, rows);
        raw_bytes += raw_row_bytes(&data);
        let t0 = Instant::now();
        let payload = record_bytes(&data);
        let mut rec = Vec::with_capacity(payload.len() + 40);
        rec.extend_from_slice(&(payload.len() as u64).to_le_bytes());
        rec.extend_from_slice(blake3_of(&payload).as_bytes());
        rec.extend_from_slice(&payload);
        f.write_all(&rec).expect("append");
        f.sync_data().expect("sync");
        ns.push(t0.elapsed().as_nanos() as u64);
        bytes += rec.len() as u64;
    }
    drop(f);

    // Replay: every record present, intact and equal to the rows it should hold.
    let t = Instant::now();
    let mut r = BufReader::new(File::open(&path).expect("reopen"));
    let mut replayed = 0usize;
    let mut mismatches = 0usize;
    loop {
        let mut len = [0u8; 8];
        if r.read_exact(&mut len).is_err() {
            break;
        }
        let mut digest = vec![0u8; blake3_of(b"").len()];
        let mut payload = vec![0u8; u64::from_le_bytes(len) as usize];
        if r.read_exact(&mut digest).is_err() || r.read_exact(&mut payload).is_err() {
            break; // torn tail
        }
        if digest != blake3_of(&payload).as_bytes()
            || payload != record_bytes(&rows_of(replayed, rows))
        {
            mismatches += 1;
        }
        replayed += 1;
    }
    let replay_s = t.elapsed().as_secs_f64();
    let _ = fs::remove_file(&path);
    emit(&serde_json::json!({
        "arm": "raw_row_log_lower_bound", "medium": label, "rows_per_commit": rows,
        "commits": commits,
        "commits_per_s": commits as f64 / ns.iter().sum::<u64>() as f64 * 1e9,
        "commit_total": stats_json(&stats(&ns)),
        "raw_row_bytes_per_commit": raw_bytes / commits as u64,
        "staged_bytes_per_commit": bytes / commits as u64,
        "replay_s": replay_s, "recovered_commits": replayed,
        "mismatches": mismatches + usize::from(replayed != commits),
    }));
}

/// tessera's digest spelling (`blake3:<hex>`), so the probe needs no crate of its own.
fn blake3_of(bytes: &[u8]) -> String {
    tessera_core::hash::digest(bytes)
}

/// The cost of one sync on this medium: 64 × (append 128 bytes, `sync_all`).
fn run_sync_cost(dir: &Path, label: &str) {
    let path = dir.join("sync-cost.bin");
    let mut f = File::create(&path).expect("create");
    let mut ns = Vec::new();
    for _ in 0..64 {
        f.write_all(&[7u8; 128]).expect("write");
        let t = Instant::now();
        f.sync_all().expect("sync");
        ns.push(t.elapsed().as_nanos() as u64);
    }
    let _ = fs::remove_file(&path);
    emit(&serde_json::json!({"arm": "sync_cost", "medium": label, "sync_all": stats_json(&stats(&ns))}));
}

/// Child of `crash`: commit forever, printing `ack <commit>` after each `append_block` returns.
fn run_child(stage: &Path, rows: usize) {
    let sp = spec(rows);
    let mut ws = WriteSession::create(stage, "recon", "commit-cost", "probe", TS).expect("create");
    let out = std::io::stdout();
    for commit in 0.. {
        let (block_ref, payload) =
            table_block(&block_name(commit), &sp, &rows_of(commit, rows)).expect("encode");
        ws.append_block(block_ref, &payload.bytes).expect("append");
        let mut o = out.lock();
        writeln!(o, "ack {commit}").expect("ack");
        o.flush().expect("flush");
    }
}

/// SIGKILL a writer after it acknowledged `kill_after + 1` commits, then recover: every
/// acknowledged commit must be there and every recovered fragment must hold the right rows.
fn run_crash(dir: &Path, rows: usize, kill_after: usize, label: &str) {
    let stage = dir.join(format!("crash-{rows}-{kill_after}"));
    let _ = fs::remove_dir_all(&stage);
    let exe = std::env::current_exe().expect("exe");
    let mut child = Command::new(exe)
        .args(["child", stage.to_str().expect("utf8"), &rows.to_string()])
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn");
    let mut acked = 0usize;
    for line in BufReader::new(child.stdout.take().expect("stdout")).lines() {
        let line = line.expect("line");
        if let Some(n) = line.strip_prefix("ack ") {
            acked = n.parse::<usize>().expect("ack index") + 1;
            if acked > kill_after {
                break;
            }
        }
    }
    child.kill().expect("kill");
    let _ = child.wait();
    let (watermark, mismatches, recover_s, ws) = recover_and_check(&stage, rows);
    drop(ws);
    let _ = fs::remove_dir_all(&stage);
    emit(&serde_json::json!({
        "arm": "tessera_crash_sigkill", "medium": label, "rows_per_commit": rows,
        "acknowledged_commits": acked, "recovered_commits": watermark,
        "lost_acknowledged": acked.saturating_sub(watermark),
        "mismatches": mismatches, "recover_s": recover_s,
    }));
}

fn median(mut v: Vec<f64>) -> Option<f64> {
    v.sort_by(f64::total_cmp);
    v.get(v.len().checked_sub(1)? / 2).copied()
}

/// Judge every commit arm in the report at `path` against `budget`, per medium and rows per commit.
/// Latency, rate and size come from the trial runs (median over trials; p99 from the worst trial).
/// Flatness comes from the arm's run of `budget::FLAT_COMMITS` commits on the same medium, which the
/// job makes at one row per commit, where a cost that grows with the session shows most. A line that
/// was not measured is `null`, and the verdict is then `incomplete`, never `pass`.
fn run_verdict(path: &Path) {
    let records: Vec<serde_json::Value> = BufReader::new(File::open(path).expect("open report"))
        .lines()
        .map(|l| serde_json::from_str(&l.expect("line")).expect("json record"))
        .collect();
    let commit_arm = |r: &serde_json::Value| {
        matches!(r["arm"].as_str(), Some("tessera_append_block" | "raw_row_log_lower_bound"))
    };
    let num = |r: &serde_json::Value, path: &[&str]| {
        path.iter().fold(r, |v, k| &v[*k]).as_f64().expect("number in record")
    };

    // (arm, medium) -> (rows per commit, last tenth over first tenth), from the long run.
    let mut flat: BTreeMap<(String, String), (u64, f64)> = BTreeMap::new();
    // (arm, medium, rows per commit) -> the trial records.
    let mut trials: BTreeMap<(String, String, u64), Vec<&serde_json::Value>> = BTreeMap::new();
    for r in records.iter().filter(|r| commit_arm(r)) {
        let arm = r["arm"].as_str().expect("arm").to_string();
        let medium = r["medium"].as_str().expect("medium").to_string();
        let rows = r["rows_per_commit"].as_u64().expect("rows");
        if r["commits"].as_u64().expect("commits") >= budget::FLAT_COMMITS {
            let ratio = num(r, &["commit_total", "last_decile_mean_ns"])
                / num(r, &["commit_total", "first_decile_mean_ns"]);
            flat.insert((arm, medium), (rows, ratio));
        } else if rows <= budget::MAX_ROWS {
            trials.entry((arm, medium, rows)).or_default().push(r);
        }
    }

    for ((arm, medium, rows), rs) in &trials {
        let over = |path: &[&str]| rs.iter().map(|r| num(r, path)).collect::<Vec<f64>>();
        let p50 = median(over(&["commit_total", "p50_ns"])).expect("trial");
        let p99 = over(&["commit_total", "p99_ns"]).into_iter().fold(0.0, f64::max);
        let p99_median = median(over(&["commit_total", "p99_ns"])).expect("trial");
        let rate = median(over(&["commits_per_s"])).expect("trial");
        let size = median(over(&["staged_bytes_per_commit"])).expect("trial")
            / median(over(&["raw_row_bytes_per_commit"])).expect("trial");
        // The long run is at one row count; its ratio judges every row count of that arm and medium.
        let long_run = flat.get(&(arm.clone(), medium.clone())).copied();
        let flat_ratio = long_run.map(|(_, ratio)| ratio);
        let lines = [
            Some(p50 <= budget::P50_NS as f64),
            Some(p99 <= budget::P99_NS as f64),
            Some(rate >= budget::MIN_COMMITS_PER_S),
            flat_ratio.map(|f| f <= budget::FLAT_RATIO),
            Some(size <= budget::SIZE_RATIO),
        ];
        let verdict = if lines.contains(&Some(false)) {
            "fail"
        } else if lines.contains(&None) {
            "incomplete"
        } else {
            "pass"
        };
        emit(&serde_json::json!({
            "arm": "budget_verdict", "judged_arm": arm, "medium": medium, "rows_per_commit": rows,
            "trials": rs.len(), "verdict": verdict,
            "p50_ns": p50, "p50_ok": lines[0], "budget_p50_ns": budget::P50_NS,
            "worst_p99_ns": p99, "median_p99_ns": p99_median, "p99_ok": lines[1],
            "budget_p99_ns": budget::P99_NS,
            "commits_per_s": rate, "rate_ok": lines[2], "budget_commits_per_s": budget::MIN_COMMITS_PER_S,
            "last_over_first_tenth": flat_ratio, "flat_ok": lines[3], "budget_flat_ratio": budget::FLAT_RATIO,
            "flat_rows_per_commit": long_run.map(|(rows, _)| rows), "flat_commits": budget::FLAT_COMMITS,
            "staged_over_raw_bytes": size, "size_ok": lines[4], "budget_size_ratio": budget::SIZE_RATIO,
        }));
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arg = |i: usize| args.get(i).unwrap_or_else(|| panic!("missing argument {i}")).clone();
    let num = |i: usize| arg(i).parse::<usize>().expect("number");
    let dir = PathBuf::from(arg(2));
    match arg(1).as_str() {
        // child <stage> <rows>
        "child" => run_child(&dir, num(3)),
        // sync-cost <dir> <label>
        "sync-cost" => run_sync_cost(&dir, &arg(3)),
        // tessera <dir> <rows> <commits> <label> [seal]
        "tessera" => run_tessera(&dir, num(3), num(4), &arg(5), args.get(6).is_some()),
        // rowlog <dir> <rows> <commits> <label>
        "rowlog" => run_rowlog(&dir, num(3), num(4), &arg(5)),
        // crash <dir> <rows> <kill_after> <label>
        "crash" => run_crash(&dir, num(3), num(4), &arg(5)),
        // verdict <report.jsonl>
        "verdict" => run_verdict(&dir),
        other => panic!("unknown mode {other}"),
    }
}
