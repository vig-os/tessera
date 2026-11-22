---
type: issue
state: open
created: 2026-11-18T20:04:30Z
updated: 2026-11-21T21:23:29Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/905
comments: 6
labels: feature, area:io, adr
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-22T09:19:45.834Z
---

# [Issue 905]: [feat(io): a commit log for small keyed commits — append as the commit point, replay on open, flush into a #781 delta](https://github.com/vig-os/tessera/issues/905)

Filed by an agent for the owner, 2026-11-18, from the kit storage benchmark (nerdmachines/kit#98 for nerdmachines/kit#97). Feature request. It needs an ADR, because it touches ADR-0026's "one encoder, one commit point".

## Motivation

kit keeps app state (settings, layouts, tasks, edges) in keyed tessera collections, with no row store beside them (owner direction 2026-11-18, nerdmachines/kit#97: typed collections at every scale, no SQLite exception). *Corrected 2026-11-19: this sentence earlier added "the log is Arrow IPC, batched into tessera blocks" as owner direction. The owner's words (spelling normalised) were: "logs are arrow ipc batched to tsra blocks/chunks as it's a continuous append/write. this is really about a configured layout, settings, state of the app, if sqlite is the choice here or not." The measurer reads them as being about logs as append data, not about the commit log of keyed collections; whether the owner meant more is an open owner question (nerdmachines/kit D61). Which commit point small keyed commits use is the open question of this issue.*. Its commits are small, 1 to about 1,000 rows, and arrive at human rate and at agent rate. Each commit must be durable, atomic and visible to the next read.

## What already exists

- `WriteSession`: a journal line, fsynced, is the commit point; fragments are Vortex files; `recover` drops a torn trailing line and quarantines a tail it cannot replay (ADR-0026, #686, #695).
- #798: group commit for the journal, design first.
- #203 (design comment of 2026-11-10) leaves the WAL format open: fragment-as-WAL with an optional Arrow IPC mirror in `aux/`, and "reject a second sealed log".
- #781: delta products over a base, with compaction.
- Filed the same day from the kit's acquisition design (found after this issue was opened): #907, a timer flush that spills a partial row group to a durable staging fragment, fsynced and journalled; #908, an ingress tap that shows pushed rows before they are durable; #909, writer states.
- The kit's own DAQ decision (ADR 0009 §2, proposed) takes the same position as #203: fragment plus journal is the one commit point, and an Arrow IPC log beside it is rejected for append streams.

## The gap

A Vortex fragment per commit is the wrong unit for small commits. Measured in the harness (Vortex 0.93.0 from crates.io, virtiofs scratch on SSD, one `fdatasync` there about 430 µs):

| Rows per commit | One Vortex delta file per commit |
|---|---:|
| 1 | about 10 ms (93–99 commits per second) |
| 10,000 | about 180 ms (5.5 commits per second) |

An appended Arrow IPC stream, flushed into a delta later, behaved as follows:

| Changed rows in the log | Replay on open | Flush into one Vortex delta |
|---|---:|---:|
| 10³ | 0.5 ms | 59 ms, 322 KiB |
| 10⁵ | 44 ms | 991 ms, 28.7 MiB |
| 10⁶ | 0.8 s | 11.3 s, 288 MiB |

- Replay reproduced the oracle state at all three sizes, from a clean log. A torn tail was not tested.
- **Not validly tested** (corrected 2026-11-18 after two reviews of the harness): read-your-write, reader behaviour during a commit or a flush, and two writers. The rerun is nerdmachines/kit#113.
- **The durable commit rate was not measured.** The harness log sat on tmpfs, so its sync was free. This request asks for that number.
- An earlier measurement from another codebase, for scale: flock recorded a durable event log at p50 about 4 ms and p99 under 12 ms, and rejected SlateDB on a measured 100 ms flush floor (flock ADR-0005/0031/0045 and ADR-0046, as cited in the kit's porting catalog; medium and sync policy not checked).

## Scope, after reading #907 and #908

The row log below is one of two ways to get a cheap commit point, and it is the one #203 and ADR-0026 argue against. The other is the path that exists: a staging fragment plus a journal line per commit, which #907 already opens up for partial row groups. The harness figure of about 10 ms is for a complete Vortex file per commit. It did not exercise tessera's staging fragments, so it says nothing about their cost.

So the order of work is:
1. Measure a commit of 1, 100 and 10,000 rows through staging plus journal with an explicit flush per commit (#907 with a deadline of zero).
2. Only if that cannot meet small commits, decide on a row log as a second kind of commit point.

Points 1, 3, 4 and 6 of the proposal hold for either path. #908 is the natural source for the not-yet-durable view of a commit, and the kit's ADR 0010 already defines how one query reads sealed, committed and not-yet-durable rows through one snapshot.

## Proposal

For keyed collections of the small-row profile (#904), an unsealed commit log in the stage directory (or the staging path above):

1. **A commit is one appended record batch.** It holds the changed rows in the table's schema plus the `op` column of #781, and one opaque metadata value supplied by the caller. kit puts its receipt there, so a state change and its receipt cannot exist without each other.
2. **The commit point is the log's sync.** Policy is declared: sync per commit, or group commit as in #798, with the loss window stated ("a crash loses at most the commits of one interval, never part of one").
3. **Recovery replays the log** in order, drops a torn trailing batch, and quarantines a tail it cannot replay. These are the journal's rules.
4. **A flush writes the log's rows as one #781 delta product,** advances the `current` collection, and only then truncates the log. A crash between those steps must not lose or double-apply a commit.
5. **The log never enters `content_hash`.** Sealed bytes are still written by the one Vortex encoder, at flush. The log is a second kind of commit point, not a second sealed log; that is the part ADR-0026 has to decide.
6. **One writer per collection.** Commit order is log order. Several writers are #288's question.

## Acceptance

- [ ] Crash injection at every step (inside a batch, after the sync, between the delta becoming durable and the log truncation): the recovered state equals an oracle of the acknowledged commits. A truncation at every byte offset of the last record is one of the cases.
- [ ] A multi-row commit is applied entirely or not at all.
- [ ] Durable commits per second on a local disk, with sync per commit and with group commit, beside the count of syncs, in the performance book. Batch sizes 1, 100 and 10,000 rows.
- [ ] **Budget for small commits** (proposed as an acceptance line; the budget itself was accepted by the owner on 2026-11-19, nerdmachines/kit#97): for commits up to 100 rows, p50 at most 2 ms and p99 at most 10 ms; at least 500 commits per second from one writer; cost flat up to 100,000 commits in a session; at most twice the raw row bytes on disk before sealing. Judged by the `verdict` mode of the #905 commit-cost probe. Reading "flat" as at most 1.2 times and p99 as the worst of three trials is the measurer's reading. Measured in three runs, 2026-11-18 and 2026-11-19: the existing path fails in each. A one-append-one-sync lower bound passes every line at one row per commit. At 100 rows its rate is near or below the line in the two later runs (509 and 494 commits per second; 781 in the first; the probe's verdict in the third run is `fail` on rate), and growth was not measured there ([run 3](https://github.com/vig-os/tessera/issues/905#issuecomment-6997624518), [run 5](https://github.com/vig-os/tessera/issues/905#issuecomment-6999085780)). How the rate line at 100 rows is to be judged on this mount is open for the owner.
- [ ] The delta written by a flush is byte-identical to sealing the same rows in one batch write.
- [ ] `content_hash` of every sealed product is the same with and without the log path in use.

## Open

- ADR-0026: is a row log as the commit point acceptable for this profile, or must the fragment stay the commit point with the cost above?
- Does #795's signed checkpoint cover commits still in the log?
- Who owns the in-memory view of unflushed commits that gives read-your-write: tessera's reader (#903) or the embedding application?
- Conditional commits: two kit consumers autosave with If-Match and surface a conflict. Is the row-version check done by the caller before the commit, or does the commit carry an expected version?
- Other storage media: the kit's local-web/WASM and mobile profiles hold local collections directly, so the commit path has to work on browser and mobile storage, not only on a POSIX file with fsync. Not considered in the harness.
- Secondary lookups (rows by a non-key column) were not tested. The harness kept only a key index in memory.

## Related

#203 (WAL format question), #907 (timer flush), #908 (ingress tap), #909 (writer states), #903 (keyed read), #904 (profile), #906 (retention), #798 (group commit), #781 (deltas), #780 (keys), #795 (checkpoints), #288 (several writers), ADR-0026. Harness: `bench/vortex-delta/` on nerdmachines/kit#98, stage 1f; its comparison with SQLite's write rate is void and is not used here.




---

# [Comment #1]() by [gerchowl]()

_Posted on November 18, 2026 at 09:58 PM_

**Measured: what a small durable commit costs through the path tessera has today** (agent, 2026-11-18; step 1 of "Scope" above, for nerdmachines/kit#97).

**Setup.** tessera `dev` at `81914f6` on its Vortex fork pin, built with the pattern of `bench/ksb/635-vortex-ab`. One commit = `table::table_block` (encode) + `WriteSession::append_block`, single thread. Rows as in the kit harness: about 350 bytes (`u64` key, two `u8`, one `i64`, a 40-byte and a 300-byte string; the strings are random). Batch host, 4 exclusive CPUs, job `tessera-905-commit-2`. Two media: the SSD scratch, which is a virtiofs mount (one `sync_all` there: p50 0.43 ms, p99 0.80 ms), and tmpfs, where a sync is free, so tmpfs shows the CPU cost alone. Three trials per cell, medians shown. 44 records, no mismatch.

Beside it a **lower bound, not a design**: the same rows' column bytes appended to one file with one `sync_data` per commit.

**Checks.** After every run the session is dropped unsealed and reopened with `WriteSession::recover`; the watermark equals the commits, and every fragment in `blocks/` is byte-equal to a re-encode of the rows it should hold. A writer killed with SIGKILL after 501 and after 201 acknowledged commits lost none. That is a process kill, not a power loss.

## Per commit, on the SSD scratch

| Rows per commit | Existing path, p50 | of which encode | of which append | Commits per second | Bytes per commit | Lower bound, p50 | Lower bound, commits per second |
|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | 3.7 ms | 1.3 ms | 2.4 ms | 256 | 6,756 + 389 journal | 0.56 ms | 1,718 |
| 100 | 15.6 ms | 10.4 ms | 4.5 ms | 57 | 43,137 | 1.06 ms | 781 |
| 10,000 | 83 ms | 53 ms | 30 ms | 11.5 | 3.32 MB | 32 ms | 30 |

Worst p99 over the trials: 12.8 ms, 31.5 ms and 214 ms for the existing path; 4.9 ms, 6.6 ms and 58 ms for the lower bound.

## CPU cost alone (tmpfs)

| Rows per commit | Existing path, p50 | of which encode | of which append | Lower bound, p50 |
|---:|---:|---:|---:|---:|
| 1 | 1.31 ms | 1.20 ms | 0.12 ms | 1.5 µs |
| 100 | 9.99 ms | 9.84 ms | 0.15 ms | 31 µs |
| 10,000 | 53.7 ms | 52.7 ms | 0.94 ms | 3.1 ms |

## What the numbers say

- **Three syncs per commit.** `strace -c` counts 612 `fsync` calls for 200 commits plus session setup (fragment, directory, journal), against one `fdatasync` per commit for the lower bound. On this disk that is about 1.3 ms of the 3.7 ms.
- **Encoding a one-row block costs 1.2 ms of CPU and 6.8 KB,** about 19 times the row. At 100 rows it is 10 ms. The encoder's fixed cost is the largest single part of a small commit, larger than the syncs on this disk.
- **The rest of the append,** about 1.1 ms per commit on the SSD scratch, is file creation and directory work. Part of that is this mount (virtiofs); it has not been separated out.
- **The cost of an append grows with the number of blocks already in the session.**

  | Commits in the session | Append, mean of first tenth | Append, mean of last tenth | `recover` |
  |---:|---:|---:|---:|
  | 2,000 (tmpfs) | 68 µs | 184 µs | 0.03 s |
  | 20,000 (tmpfs) | 120 µs | 2,299 µs | 0.46 s |
  | 20,000 (SSD) | 2.5 ms | 5.4 ms | 2.3 s |

  That is roughly 0.1 µs per committed block per append. The likely cause is the name checks in `admit`, which scan every committed name (read in `write.rs`, not profiled). `recover` grows faster than linearly too. One trial for the 20,000-commit runs.
- **Sealing many small blocks is expensive in bytes:** 2,000 one-row commits seal to 14.7 MiB for under 1 MiB of rows (3.8 s on the SSD scratch, 0.13 s on tmpfs). A flush would have to compact, as #781 intends.

## For the choice in this issue

- The path as built gives about 250 durable one-row commits per second on this disk, falling as the session fills, against about 1,700 for one append and one sync. So it is about 6.6 times the lower bound.
- None of the three costs needs a second commit point to remove: the encode (stage raw rows, as the streaming path does for full row groups), the three syncs (#798), and the per-block file with its name scan.
- What this run **cannot** say is whether #907's partial-fragment staging would be cheap enough, because it is not built. `StreamWriter` stages only full row groups (2¹⁶ rows), so a 1-row commit cannot go through it today.
- No budget is set for small commits. With one, this table decides whether the existing path is enough for a given consumer; without one it only gives the factor.

## Not measured

- A local disk without virtiofs. For scale: vigil's ADR-0005 measured 23–34 ms per `sync_data` on another host's ext4, which would put three syncs near 80 ms.
- Power loss; a torn journal line; more than one lane or writer; group commit; read cost over thousands of tiny blocks.
- Real data: the strings here do not compress, which favours neither arm but inflates bytes.

<details><summary>Probe (<code>commit_cost.rs</code>, built as a tessera-io example) and job flake</summary>

```rust
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
//! Output: one JSON object per line on stdout. Timings are context (shared host); the counts of
//! sync calls come from `strace -c` in the job script, not from this program.
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use tessera_core::block::table::TableSpec;
use tessera_io::table::{table_block, ColumnData, TableData};
use tessera_io::WriteSession;

const TS: &str = "2026-11-18T00:00:00Z";

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
    let wall = Instant::now();
    for commit in 0..commits {
        let data = rows_of(commit, rows);
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
    println!(
        "{}",
        serde_json::json!({
            "arm": "tessera_append_block", "medium": label, "rows_per_commit": rows,
            "commits": commits, "wall_s": wall_s,
            "commits_per_s": commits as f64 / total.iter().sum::<u64>() as f64 * 1e9,
            "encode": stats_json(&stats(&encode_ns)),
            "append": stats_json(&stats(&append_ns)),
            "commit_total": stats_json(&stats(&total)),
            "payload_bytes_per_commit": payload_bytes / commits as u64,
            "journal_bytes_per_commit": journal_bytes / commits as u64,
            "stage_bytes": stage_bytes,
            "recover_s": recover_s, "recovered_commits": watermark,
            "mismatches": mismatches + usize::from(watermark != commits),
            "seal_s": seal_s, "sealed_bytes": sealed_bytes,
        })
    );
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
    for commit in 0..commits {
        let data = rows_of(commit, rows);
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
    println!(
        "{}",
        serde_json::json!({
            "arm": "raw_row_log_lower_bound", "medium": label, "rows_per_commit": rows,
            "commits": commits,
            "commits_per_s": commits as f64 / ns.iter().sum::<u64>() as f64 * 1e9,
            "commit_total": stats_json(&stats(&ns)),
            "payload_bytes_per_commit": bytes / commits as u64,
            "replay_s": replay_s, "recovered_commits": replayed,
            "mismatches": mismatches + usize::from(replayed != commits),
        })
    );
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
    println!(
        "{}",
        serde_json::json!({"arm": "sync_cost", "medium": label, "sync_all": stats_json(&stats(&ns))})
    );
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
    println!(
        "{}",
        serde_json::json!({
            "arm": "tessera_crash_sigkill", "medium": label, "rows_per_commit": rows,
            "acknowledged_commits": acked, "recovered_commits": watermark,
            "lost_acknowledged": acked.saturating_sub(watermark),
            "mismatches": mismatches, "recover_s": recover_s,
        })
    );
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
        other => panic!("unknown mode {other}"),
    }
}

```

```nix
# #905: what one small durable commit costs through the write path tessera has today
# (`table_block` + `WriteSession::append_block`), at 1, 100 and 10,000 rows per commit, on the SSD
# scratch and on tmpfs (tmpfs = CPU cost only: a sync there is free). Beside it a raw row log as a
# lower bound. Builds `commit_cost.rs` as a tessera-io example at one tessera revision, after the
# pattern of bench/ksb/635-vortex-ab.
#
#   ksb run --name tessera-905-commit-N --cpu 4 --exclusive-cpus --mem 8Gi --time 45 "path:$PWD#default"
#   ksb out tessera-905-commit-N --path commit-cost.jsonl --dest ./commit-cost.jsonl
{
  description = "ksb job: tessera commit cost for small keyed commits (#905)";

  inputs = {
    # The root flake's pins (flake.lock at the head revision).
    nixpkgs.url = "github:NixOS/nixpkgs/567a49d1913ce81ac6e9582e3553dd90a955875f";
    rust-overlay = {
      url = "github:oxalica/rust-overlay/77a8263847fb02dc49dbe377278ef6b952f1c6bb";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    crane.url = "github:ipetkov/crane/469fd08d0bcf6926321fa973c6777fbc87785dd7";
    head = {
      url = "github:vig-os/tessera/81914f64ea5fefcf3b066690ccbb19535adaacf1";
      flake = false;
    };
    # The vortex fork revision the head pins. ksb evaluates offline without `git`, so cargo's git
    # source can't be fetched there; the job's source copy points the [patch] at this input by path.
    vortex-head = {
      url = "github:gerchowl/vortex/5c55cf7fa35bac652849cf1a0e76a1a97035f715";
      flake = false;
    };
  };

  outputs =
    { nixpkgs, rust-overlay, crane, head, vortex-head, ... }:
    let
      pkgs = import nixpkgs {
        system = "x86_64-linux";
        overlays = [ (import rust-overlay) ];
      };
      vortexRev = "5c55cf7fa35bac652849cf1a0e76a1a97035f715";
      gitSource = "git+https://github.com/gerchowl/vortex?rev=${vortexRev}#${vortexRev}";
      lock = builtins.replaceStrings [ "source = \"${gitSource}\"\n" ] [ "" ] (
        builtins.readFile "${head}/tessera/Cargo.lock"
      );
      toolchain = pkgs.rust-bin.fromRustupToolchainFile "${head}/tessera/rust-toolchain.toml";
      craneLib = (crane.mkLib pkgs).overrideToolchain toolchain;
      src = pkgs.runCommand "tessera-905-src" { } ''
        cp -r ${head}/tessera $out && chmod -R u+w $out
        cp ${./commit_cost.rs} $out/crates/tessera-io/examples/commit_cost_905.rs
        # [patch] git -> path: each crate's directory in the fork checkout, by its manifest's package name.
        git_dep='{ git = "https://github.com/gerchowl/vortex", rev = "${vortexRev}" }'
        grep -q "$git_dep" $out/Cargo.toml
        for crate in $(grep -F "$git_dep" $out/Cargo.toml | cut -d' ' -f1); do
          manifest=$(grep -rlx --include=Cargo.toml "name = \"$crate\"" ${vortex-head} | head -1)
          test -n "$manifest"
          sed -i "s|^$crate = $git_dep|$crate = { path = \"$(dirname "$manifest")\" }|" $out/Cargo.toml
        done
        ! grep -qF "$git_dep" $out/Cargo.toml
        cp ${pkgs.writeText "Cargo.lock" lock} $out/Cargo.lock
      '';
      probe = craneLib.mkCargoDerivation {
        pname = "tessera-905-commit-cost";
        version = "0.0.0";
        inherit src;
        cargoArtifacts = null;
        cargoVendorDir = craneLib.vendorCargoDeps {
          cargoLockContents = lock;
        };
        nativeBuildInputs = with pkgs; [ clang pkg-config cmake ];
        buildInputs = with pkgs; [ stdenv.cc.cc.lib hdf5 ];
        LIBCLANG_PATH = "${pkgs.libclang.lib}/lib";
        buildPhaseCargoCommand = ''
          cargo build --release --locked -p tessera-io --example commit_cost_905
        '';
        installPhaseCommand = ''
          install -Dm755 target/release/examples/commit_cost_905 $out/bin/commit_cost
        '';
        doCheck = false;
      };
      job = pkgs.writeShellApplication {
        name = "tessera-905-commit-cost";
        runtimeInputs = [ pkgs.coreutils pkgs.strace ];
        excludeShellChecks = [ "SC2129" ];
        text = ''
          out="''${KSB_JOB_OUT:-$PWD/out}"
          mkdir -p "$out"
          res="$out/commit-cost.jsonl"
          : > "$res"
          bin=${probe}/bin/commit_cost
          echo "# #905 commit cost, tessera ${head.rev}, nproc=$(nproc), $(date -u +%FT%TZ)" | tee "$out/run.txt"

          run_medium() {
            medium="$1"
            parent="$2"
            if [ ! -d "$parent" ] || [ ! -w "$parent" ]; then
              echo "skip $medium: $parent is not writable" | tee -a "$out/run.txt"
              return 0
            fi
            dir=$(mktemp -d "$parent/tessera-905.XXXXXX")
            echo "medium $medium at $dir ($(stat -f -c %T "$dir"))" | tee -a "$out/run.txt"
            "$bin" sync-cost "$dir" "$medium" >> "$res"
            for trial in 1 2 3; do
              echo "trial $trial on $medium" | tee -a "$out/run.txt"
              "$bin" tessera "$dir" 1 2000 "$medium" seal >> "$res"
              "$bin" rowlog "$dir" 1 2000 "$medium" >> "$res"
              "$bin" tessera "$dir" 100 2000 "$medium" seal >> "$res"
              "$bin" rowlog "$dir" 100 2000 "$medium" >> "$res"
              "$bin" tessera "$dir" 10000 200 "$medium" seal >> "$res"
              "$bin" rowlog "$dir" 10000 200 "$medium" >> "$res"
            done
            # Growth with the number of commits already in the session.
            "$bin" tessera "$dir" 1 20000 "$medium" >> "$res"
            # A writer killed with SIGKILL: every acknowledged commit must be recovered.
            "$bin" crash "$dir" 1 500 "$medium" >> "$res"
            "$bin" crash "$dir" 100 200 "$medium" >> "$res"
            # Sync calls per commit (200 commits each; not timed).
            strace -f -c -e trace=fsync,fdatasync -o "$out/strace-tessera-$medium.txt" \
              "$bin" tessera "$dir" 1 200 "$medium" > /dev/null \
              || echo "strace failed for tessera on $medium" | tee -a "$out/run.txt"
            strace -f -c -e trace=fsync,fdatasync -o "$out/strace-rowlog-$medium.txt" \
              "$bin" rowlog "$dir" 1 200 "$medium" > /dev/null \
              || echo "strace failed for rowlog on $medium" | tee -a "$out/run.txt"
            rm -rf "$dir"
          }

          run_medium ssd /data/ssd-outputs
          run_medium tmpfs /dev/shm
          echo "done $(date -u +%FT%TZ)" | tee -a "$out/run.txt"
          cat "$res"
        '';
      };
    in
    {
      packages.x86_64-linux = { inherit probe; default = job; };
      apps.x86_64-linux.default = {
        type = "app";
        program = "${job}/bin/tessera-905-commit-cost";
      };
    };
}

```

Run on the batch host from a directory holding both files: `nix flake lock`, then `ksb run --name tessera-905-commit-N --cpu 4 --exclusive-cpus --mem 8Gi --time 45 "path:$PWD#default"`. Outputs: `commit-cost.jsonl`, `run.txt`, `strace-*.txt`.
</details>


---

# [Comment #2]() by [gerchowl]()

_Posted on November 19, 2026 at 02:16 AM_

**A budget for small commits, and the existing path judged against it** (agent, 2026-11-19; for nerdmachines/kit#97)

**Budget.** The owner accepted this on 2026-11-19 (recorded on nerdmachines/kit#97, "Writer control"): for commits up to 100 rows, p50 at most 2 ms and p99 at most 10 ms; at least 500 commits per second from one writer; cost flat up to 100,000 commits in a session; at most twice the raw row bytes on disk before sealing.

**How I judge it** (my reading, not the owner's words): latency is encode plus append, acknowledged after the sync, one commit at a time, no group commit. p50 and rate are medians of three trials of 2,000 commits; p99 is the worst of the three trials (the median is given beside it). "Flat" is the mean of the last tenth over the mean of the first tenth of a 100,000-commit session, at most 1.2 (`default_tolerance = 0.20` in `perf-budgets.toml`). The long run is at one row per commit only. Bytes are fragment plus journal bytes per commit over the rows' raw column bytes (358 per row here).

**Run `tessera-905-commit-3`.** Same setup as the comment above (`dev` at `81914f6`, fork pin, 4 exclusive CPUs, SSD scratch through virtiofs and tmpfs, three trials per cell), plus a 100,000-commit session for both arms on the SSD scratch. 42 records, no mismatch. The job reached its 120-minute limit in the tmpfs growth run, after all SSD work was done. The probe's own verdict step therefore did not run; the verdicts below are my recomputation from the report with the same rules.

**The two runs differ, and the host was shared.** On the SSD scratch run 3 is slower than run 2 by 1.5 to 2.3 times in median p50 and in rate. At 1 and 100 rows per commit, p99 (worst of three) moved between 0.96 and 3.5 times. Encoding a one-row block, which touches no disk, took 1.2 ms on tmpfs and 1.3 ms on the SSD scratch in run 2, against 2.0 ms and 3.5 ms in run 3. One `sync_all` on the SSD scratch went from p50 0.43 ms to 0.61 ms. Three other batch jobs were running when I looked after the run, and the load average was about 66 on 88 threads. Host load is a possible explanation; I did not isolate it. Both runs are shown. *(Corrected 2026-11-19 after review: this paragraph first said "1.5 to 2.3 times slower in every cell" and named the busy host as the cause.)*

## One row per commit, SSD scratch

| Line | Budget | Existing path, run 2 | Existing path, run 3 | Lower bound, run 2 | Lower bound, run 3 |
|---|---:|---:|---:|---:|---:|
| p50 | ≤ 2 ms | 3.71 ms | 8.64 ms | 0.56 ms | 0.87 ms |
| p99, worst of three (median) | ≤ 10 ms | 12.76 (9.05) ms | 29.38 (26.06) ms | 4.91 (0.93) ms | 6.35 (3.13) ms |
| Commits per second | ≥ 500 | 256 | 111 | 1,718 | 904 |
| Last tenth over first tenth, 100,000 commits | ≤ 1.2 | not run (1.79 at 20,000) | **5.68** (14.3 → 81.2 ms) | not run | **1.00** (0.957 → 0.956 ms) |
| Staged bytes over raw bytes | ≤ 2 | 19.96 (7,145 / 358) | 19.96 | 1.38 (493 / 358) | 1.38 |
| Verdict | | fail | **fail, every line** | passes what was run | **pass, every line** |

## 100 rows per commit, SSD scratch

| Line | Budget | Existing path, run 2 | Existing path, run 3 | Lower bound, run 2 | Lower bound, run 3 |
|---|---:|---:|---:|---:|---:|
| p50 | ≤ 2 ms | 15.57 ms | 27.62 ms | 1.06 ms | 1.74 ms |
| p99, worst of three (median) | ≤ 10 ms | 31.54 (31.13) ms | 110.26 (79.29) ms | 6.57 (6.18) ms | 6.30 (5.14) ms |
| Commits per second | ≥ 500 | 57 | 33 | 781 | 509 (trials: 509, 435, 523) |
| Staged bytes over raw bytes | ≤ 2 | 1.22 (43,528 / 35,800) | 1.22 | 1.03 (36,727 / 35,800) | 1.03 |
| Verdict | | fail (p50, p99, rate) | fail (p50, p99, rate) | passes what was run | passes what was run; rate at the limit |

Flatness was not measured at 100 rows per commit.

## The 100,000-commit session (one row per commit, SSD scratch, one trial)

| | Existing path | Lower bound |
|---|---:|---:|
| Wall time | 5,269 s (88 minutes) | 91 s |
| Commits per second over the session | 19 | 1,102 |
| p50 / p99 per commit | 49.4 / 109.0 ms | 0.79 / 2.94 ms |
| Mean of first tenth → last tenth | 14.3 → 81.2 ms | 0.957 → 0.956 ms |
| of which append | 9.8 → 77.1 ms | |
| of which encode | 4.5 → 4.2 ms | |
| Recovery after the run | `recover`: 19.5 s | replay: 0.41 s |
| Bytes staged | 715 MB for 35.8 MB of rows | 49.3 MB |

The growth is in the append, not in the encode. From the first to the last tenth it is about 0.75 µs per already-committed block per append in this run (0.16 µs on the SSD scratch over 20,000 commits in run 2, 0.1 µs on tmpfs). Whether the larger figure is the name scan, the directory with 100,000 files on virtiofs, or the host load, this run cannot say.

Unchanged from run 2: a writer killed with SIGKILL after 501 and after 201 acknowledged commits lost none; `strace -c` counts 612 `fsync` for 200 commits on the existing path and 200 `fdatasync` plus 2 `fsync` for the lower bound.

## What this says

- **The existing path does not meet the budget.** At one row per commit it fails every line that was measured, in both runs (run 2 had a 20,000-commit session, not 100,000). At 100 rows it fails p50, p99 and rate; the bytes are inside the budget there.
- **The budget can be met on this host** with one append and one sync per commit: the lower bound passes every line at one row, including flatness over 100,000 commits. At 100 rows per commit its rate is at the limit in the slower run (median 509 per second, one trial 435).
- **The encode alone uses most or all of the p50 budget:** 1.2 to 3.5 ms for a one-row block, 10 to 21 ms for 100 rows. So removing two of the three syncs (#798) is not enough; small commits need to be staged without a block encode per commit, and without a per-block file whose cost grows with the session.
- **Timings on this host moved by about a factor of two between the two runs.** The verdict on the existing path does not depend on that (it fails in both runs by a wide margin). A result near a line, such as the lower bound's rate at 100 rows, needs a quiet host or more runs. The byte ratio and the sync counts do not depend on load.

## Next

- Run `tessera-905-commit-5` is running: the same probe, with the verdict written after every stage and a 240-minute limit. It adds the probe's own verdict records and the tmpfs growth run.
- A PR that adds the probe as `bench/ksb/905-commit-cost` follows once that run has finished.
- I am adding the budget to this issue's Acceptance list as a proposal.

Not measured, as before: a local disk without virtiofs, power loss, a torn journal line, more than one lane or writer, group commit, #907's partial-fragment staging (not built).



---

# [Comment #3]() by [gerchowl]()

_Posted on November 19, 2026 at 03:48 AM_

**Run `tessera-905-commit-5`: the probe's own verdicts, and the growth run on tmpfs** (agent, 2026-11-19; for nerdmachines/kit#97)

This is the run of the files in #911. Same setup as the two comments above: `dev` at `81914f6`, fork pin, 4 exclusive CPUs, SSD scratch through virtiofs and tmpfs, three trials of 2,000 commits per cell, one 100,000-commit session per arm and medium at one row per commit. 54 records, no mismatch; the job ran for 81 minutes. The budget and the judging rules are those of the comment above; the judging rules are my reading, not the owner's words.

**The probe's verdict records** (`verdict.jsonl`; I recomputed each from the report and got the same values):

| Arm | Medium | Rows per commit | p50 | p99, worst (median) | Commits per second | Last tenth over first tenth | Staged over raw bytes | Verdict |
|---|---|---:|---:|---:|---:|---:|---:|---|
| Existing path | SSD scratch | 1 | 6.27 ms ✗ | 23.62 (9.70) ms ✗ | 156 ✗ | 2.92 ✗ | 19.96 ✗ | **fail**, every line |
| Existing path | SSD scratch | 100 | 25.78 ms ✗ | 74.66 (49.22) ms ✗ | 37 ✗ | (2.92, from the one-row session) | 1.22 | **fail** |
| Existing path | tmpfs | 1 | 1.35 ms | 2.08 (1.56) ms | 738 | 7.92 ✗ | 19.96 ✗ | **fail** (growth, bytes) |
| Existing path | tmpfs | 100 | 10.45 ms ✗ | 14.48 (12.83) ms ✗ | 94 ✗ | (7.92, from the one-row session) | 1.22 | **fail** |
| Lower bound | SSD scratch | 1 | 0.78 ms | 3.71 (2.56) ms | 1,155 | 0.86 | 1.38 | **pass**, every line |
| Lower bound | SSD scratch | 100 | 1.60 ms | 8.15 (8.04) ms | 494 ✗ (trials 494, 530, 485) | (0.86, from the one-row session) | 1.03 | **fail** (rate) |
| Lower bound | tmpfs | 1 and 100 | 1.5 µs and 26 µs | 3.9 µs and 64 µs | 604,918 and 33,989 | 0.37 | 1.38 and 1.03 | pass |

The long session is at one row per commit only. The probe applies its ratio to every row count of the same arm and medium and says so in the record (`flat_rows_per_commit: 1`). Growth at 100 rows per commit is still not measured.

## The 100,000-commit sessions (one row per commit, one trial each)

| | Existing path, SSD scratch | Existing path, tmpfs | Lower bound, SSD scratch | Lower bound, tmpfs |
|---|---:|---:|---:|---:|
| Wall time | 2,589 s (43 minutes) | 895 s (15 minutes) | about 94 s | about 0.2 s |
| Commits per second over the session | 39 | 112 | 1,069 | 469,160 |
| p50 / p99 per commit | 24.3 / 56.8 ms | 7.4 / 19.8 ms | 0.81 / 3.18 ms | 1.5 / 4.3 µs |
| Mean of first tenth → last tenth | 8.16 → 23.83 ms | 1.86 → 14.70 ms | 1.023 → 0.880 ms | |
| of which append | 5.22 → 21.87 ms | 0.49 → 13.14 ms | | |
| of which encode | 2.93 → 1.96 ms | 1.36 → 1.56 ms | | |
| Recovery after the run | `recover`: 12.4 s | `recover`: 1.6 s | replay: 0.53 s | replay: 0.29 s |
| Bytes staged | 715 MB for 35.8 MB of rows | 715 MB | 49.3 MB | 49.3 MB |

**The growth is in tessera's append, not in the mount.** On tmpfs there is no device and no virtiofs, and the append still grows from 0.49 ms to 13.14 ms. From the first to the last tenth that is about 0.14 µs per already-committed block per append on tmpfs and 0.19 µs on the SSD scratch. Run 2 gave 0.16 µs on the SSD scratch over 20,000 commits. Run 3's 0.75 µs is the outlier. So the comment above was too cautious on one point: a per-append cost that is linear in the blocks already in the session exists without virtiofs. I did not profile which call it is.

## Three runs side by side (SSD scratch)

| | Run 2 (11-18) | Run 3 (11-19) | Run 5 (11-19) |
|---|---:|---:|---:|
| Existing path, 1 row: p50 / worst p99 / per second | 3.71 ms / 12.76 ms / 256 | 8.64 ms / 29.38 ms / 111 | 6.27 ms / 23.62 ms / 156 |
| Existing path, 100 rows: p50 / worst p99 / per second | 15.57 ms / 31.54 ms / 57 | 27.62 ms / 110.26 ms / 33 | 25.78 ms / 74.66 ms / 37 |
| Existing path, growth at 1 row | 1.79 over 20,000 | 5.68 over 100,000 | 2.92 over 100,000 |
| Lower bound, 1 row: p50 / worst p99 / per second | 0.56 ms / 4.91 ms / 1,718 | 0.87 ms / 6.35 ms / 904 | 0.78 ms / 3.71 ms / 1,155 |
| Lower bound, 100 rows: p50 / worst p99 / per second | 1.06 ms / 6.57 ms / 781 | 1.74 ms / 6.30 ms / 509 | 1.60 ms / 8.15 ms / 494 |
| Lower bound, growth at 1 row | not run | 1.00 | 0.86 |
| Encode of a one-row block, SSD scratch / tmpfs (median of the trials' p50) | 1.3 / 1.2 ms | 3.5 / 2.0 ms | 2.6 / 1.2 ms |
| One `sync_all`, p50 | 0.43 ms | 0.61 ms | 0.52 ms |

**Host load.** Run 5 wrote the load average before every stage: 110 to 146 before the SSD trials, 183 before the SSD session, 93 to 115 before the tmpfs trials, 140 before the tmpfs session, on 88 threads. After run 3 I had seen about 66, once; the load during runs 2 and 3 was not recorded. Run 5 ran under a higher load average than that one figure and was faster than run 3. The job has 4 exclusive CPUs, so the host's load average may say little about it. What causes the difference between the runs was not isolated. *(Corrected 2026-11-19 after review: this first said "the load average does not explain the difference between the runs", which the one figure for run 3 cannot carry.)*

Unchanged: a writer killed with SIGKILL after 501 and after 201 acknowledged commits lost none, on both media. `strace -c` counts 612 `fsync` for 200 commits on the existing path, and 200 `fdatasync` plus 2 `fsync` for the lower bound.

## What three runs say

- **The existing path fails the budget in all three runs.** At one row per commit it fails every line that was measured. At 100 rows it fails p50, p99 and rate. Without any device (tmpfs) it meets latency and rate at one row, and still fails on growth and on bytes; at 100 rows it fails latency and rate there too, because the encode alone takes 10 ms.
- **The lower bound passes every line at one row per commit** in both runs that measured all five (runs 3 and 5).
- **At 100 rows per commit the lower bound is near or below the rate line on this mount in the two later runs:** 509 and 494 commits per second (781 in run 2); single trials between 435 and 530 in runs 3 and 5. The probe's verdict for run 5 is `fail` there. One write of 36.7 kB and one `fdatasync` is the least any design with one sync per commit can do. These two runs leave little or no measured rate margin at 100 rows per commit; whether a real commit path can meet the line there is not measured. p50, p99 and bytes have room (1.1 to 1.7 ms, 6.3 to 8.2 ms, 1.03).
- **What has to go** is unchanged, and now holds on three runs and two media: the block encode per commit (1.2 to 3.5 ms for one row, 10 to 21 ms for 100 rows), two of the three syncs, and the per-block cost that grows with the session.

## Open, for the owner of the budget

The rate line at 100 rows per commit needs a ruling before it can be an acceptance line on this host. Three ways, none of them decided:

1. Judge the rate line on a local disk without virtiofs. Not measured yet; the lower bound may have room there.
2. Keep the host, and report a line as "limited by the medium" instead of `fail` when the lower bound misses it in the same run. This is a change to my judging rules, not to the budget.
3. Keep both and accept that the line can only be met at fewer rows per commit on this mount.

I recommend the first, with the second as the rule until a local disk is measured. The probe in #911 is unchanged and reports `fail`, as the budget is written.

## State

- #911 holds exactly the files that produced this run; its CI is green, and I am marking it ready for review.
- Still not measured: a local disk without virtiofs, power loss, a torn journal line, more than one lane or writer, group commit, growth at 100 rows per commit, #907's partial-fragment staging (not built).


---

# [Comment #4]() by [gerchowl]()

_Posted on November 20, 2026 at 11:54 PM_

*Edited 2026-11-21 after review: "which kit#97 had paraphrased" → "first recorded on nerdmachines/kit#42"; "Durability classes ... are four, not three" → "owner-agreed direction (2026-11-21), to be finished under nerdmachines/kit#97"; "filed as a vigil issue" now cites [vigil#254](https://github.com/vig-os/vigil/issues/254) inline.*

Correction, 2026-11-21: recording the owner's answers on D61 (nerdmachines/kit) and amending the earlier "Arrow IPC log" paraphrase this issue repeated.

**On the 2026-11-18 sentence about logs.** Owner's words today (on nerdmachines/kit#42): "the question was how logs are written, or cached, batched,.. this is vigil territory we said, which should stay compressed json. The other general one was, how does 'live' app state etc. be handled. First guess was sqlite, which was then dropped through various arguments."

"The log is Arrow IPC, batched into tessera blocks" (which was **first recorded on nerdmachines/kit#42** on 2026-11-18 as an agent paraphrase of the sentence about logs, and this issue, kit#97 and kit#115 then repeated) is an **agent paraphrase**. It did not decide that keyed collections commit through an Arrow IPC log. Logs **about the system** are vigil's (nerdmachines/kit#55; OTLP JSON Lines per vigil ADR-0001; compression of rotated segments filed as [vigil#254](https://github.com/vig-os/vigil/issues/254)). **The keyed-collection commit path is open (D61 in nerdmachines/kit's `docs/open-questions.md`).** Append-only domain data of the app (chat history, run logbook, edge/receipt log, device operation records) stays in **typed collections**.

**Target for tessera#905 (point 3 of the owner answer).** The existing path reduced to **one sync per commit** (today three: 612 `fsync` calls for 200 commits) and **no growth** over a session. The lower bound shows this shape meets the budget.

**Budget scope (point 2 of the owner answer).** The small-commit budget applies to the new `committed` durability class and is judged on **reference hardware with local storage**, not on a loaded VM's virtiofs mount. The 100-rows rate line (lower bound measured 781 / 509 / 494 commits per second) is **not judged** from the virtiofs runs. **Re-measure on local storage: pending.**

**Durability classes** per collection, as values of the P9 `durability` tag (nerdmachines/kit#97), are **owner-agreed direction (2026-11-21), to be finished under nerdmachines/kit#97**: `ephemeral`, `relaxed`, `committed`, `sealed`. `committed` means durable before the acknowledgement, one sync per commit, with group commit allowed across concurrent writers. Dropping SQLite stands.

Tiering (owner direction): a local commit log internal to the store (one sync), the in-memory overlay for reads, and sealed blocks tiered asynchronously to object storage. One engine and one contract; the commit log is never a second database that apps see.

Recorded in kit#119 (branch `docs/storage-review-marks`), the usual dated markers on the pages that had carried the paraphrase.


---

# [Comment #5]() by [gerchowl]()

_Posted on November 21, 2026 at 02:44 AM_

**Run `kit-r119-local`: the #905 probe on a local ext4 medium (virtio-blk, no virtiofs), with an fdatasync baseline** (agent, 2026-11-21; for nerdmachines/kit#97)

**Why.** The three earlier runs (`tessera-905-commit-2`, `-3`, `-5`) all used `/data/ssd-outputs`, which is a **virtiofs** mount from the ksb batch pod to a compute-VM volume on the host's NTFS-on-NVMe (`fuseblk`); two userspace FS daemons sit in the fsync path. The owner decided on 2026-11-21 that the small-commit budget is judged on local-disk numbers, not that path's. This run moves the probe onto the only writable **non-virtiofs** medium the ksb batch pod exposes to an operator job: its own overlayfs root, whose upperdir is on `/dev/vdc` ext4 inside the compute VM. `/dev/vdc` is a virtio-blk device from the pod's perspective, so `fsync`/`fdatasync` go through the Linux block layer, not virtiofsd.

`commit_cost.rs` is byte-identical to tessera#911 head `81d9d7869c332a627f2726d0deb427ad24b82eec`; the only change from #911's flake is the job script (one medium, plus the fdatasync baseline). Tessera dev `81914f6` on its Vortex fork pin, same row shape (≈ 350 bytes). 28 records, **no mismatch** in any record; the SIGKILL check loses nothing (acked=501 recovered=501 at 1 row, acked=201 recovered=201 at 100 rows).

## Setup

- Host: ksb (`WS00860001`), batch pod `kit-r119-local`; **4 exclusive CPUs**, 8 GiB; Intel Xeon w9-3575X, 84 cores.
- Medium: **ext4 on `/dev/vdc` (virtio-blk)**, 512 GiB (overlayfs upperdir on the pod's root). `rotational=1` (virtio-blk default; the host backing is almost certainly SSD given ksb's layout, but this is not confirmed from inside the pod). No virtiofs, no NFS, no tmpfs in the sync path.
- `strace -c` on 200 commits: **612 fsync** for the existing path (3 syncs per commit + session setup), **200 fdatasync + 2 fsync** for the lower bound — same counts as the three virtiofs runs.
- **Host was heavily and unevenly loaded throughout the run**: load averages before each stage were fdatasync-baseline 177, trial 1 143, trial 2 97, trial 3 54, rowlog growth 134, tessera growth 77, crash 25. The host runs 88 threads. The p99 numbers below are dominated by this contention, not by the medium.

## fdatasync baseline on local-ext4

4 KiB append + `fdatasync`, 2000 calls on the same medium, timed under load 177 → 143:

| | p50 | p99 | max |
|---|---:|---:|---:|
| 4 KiB append + fdatasync | **0.72 ms** | 51.0 ms | 9.73 s |

Probe's own `sync_all` (128 B + fsync, 64 calls on the same medium): p50 **0.64 ms**, p99 1.19 ms (median over the fewer calls; `/data/ssd-outputs` virtiofs on `tessera-905-commit-5`: p50 0.52 ms, so the medium itself is comparable).

## One row per commit, local-ext4 (medians of three trials; p99 worst over trials)

| Line | Budget | Existing path | Lower bound |
|---|---:|---:|---:|
| p50 | ≤ 2 ms | 4.12 ms ✗ | 0.49 ms |
| p99, worst of three (median) | ≤ 10 ms | 1,469 ms ✗ (1,285) | 204 ms ✗ (178) |
| Commits per second | ≥ 500 | 12 ✗ (trials 92, 12, 11) | 124 ✗ (trials 1,251, 124, 108) |
| Last tenth / first tenth, 100,000 commits | ≤ 1.2 | 2.82 ✗ | 3.60 ✗ |
| Staged / raw bytes | ≤ 2 | 19.96 ✗ | 1.38 |
| **Verdict (probe)** | | **fail**, every line | **fail** (p99, rate, flat) |

## 100 rows per commit, local-ext4

| Line | Budget | Existing path | Lower bound |
|---|---:|---:|---:|
| p50 | ≤ 2 ms | 16.1 ms ✗ | 0.94 ms |
| p99, worst of three (median) | ≤ 10 ms | 2,068 ms ✗ (1,407) | 2,086 ms ✗ (1,247) |
| Commits per second | ≥ 500 | 9 ✗ (trials 53, 5, 9) | 16 ✗ (trials 10, 16, **1,437**) |
| Staged / raw bytes | ≤ 2 | 1.22 | 1.03 |
| **Verdict (probe)** | | **fail** | **fail** (p99, rate, flat) |

The lower bound's third trial at 100 rows produced **1,437 commits/s**, which is 2.9× the 500/s budget on this medium when the host was momentarily quieter; the median across the three trials is pulled down by the loaded trials.

## 100,000-commit session (one row per commit, one trial per arm)

| | Existing path | Lower bound |
|---|---:|---:|
| Wall time | 3,147 s (52 min) | 849 s (ratio from rate) |
| Commits per second over the session | 32 | 118 |
| p50 / p99 per commit | 13.1 / 421.9 ms | 0.60 / 120.8 ms |
| Mean first tenth → last tenth | 11.05 → 31.21 ms (ratio 2.82) | 7.73 → 27.83 ms (ratio 3.60) |
| of which append | 9.10 → 29.26 ms | — |
| of which encode | 1.95 → 1.94 ms | — |
| Recovery after the run | `recover`: **1.89 s** | replay: **0.29 s** |
| Bytes staged | 715 MB | 49.3 MB |

**Recover is 6.5× faster than on virtiofs** (run 5: 12.4 s on the same 100,000 commits). Append-growth within the session (ratio 2.82 existing / 3.60 lower bound) is driven mostly by the host-load swing within the session, not by the probe's intrinsic admit cost; run 5's lower-bound ratio on a quieter trial was 0.86.

## This run vs. the three virtiofs runs

| | commit-2 (SSD/virtiofs) | commit-3 (SSD/virtiofs) | commit-5 (SSD/virtiofs) | **local-ext4 (this run)** |
|---|---:|---:|---:|---:|
| Existing path, 1 row: p50 / worst p99 / per second | 3.71 ms / 12.76 ms / 256 | 8.64 ms / 29.38 ms / 111 | 6.27 ms / 23.62 ms / 156 | 4.12 ms / **1,469 ms** / 12 |
| Existing path, 100 rows: p50 / worst p99 / per second | 15.57 ms / 31.54 ms / 57 | 27.62 ms / 110.26 ms / 33 | 25.78 ms / 74.66 ms / 37 | 16.1 ms / 2,068 ms / 9 |
| Lower bound, 1 row: p50 / worst p99 / per second | 0.56 ms / 4.91 ms / 1,718 | 0.87 ms / 6.35 ms / 904 | 0.78 ms / 3.71 ms / 1,155 | **0.49 ms** / 204 ms / 124 (best trial 1,251) |
| Lower bound, 100 rows: p50 / worst p99 / per second | 1.06 ms / 6.57 ms / 781 | 1.74 ms / 6.30 ms / 509 | 1.60 ms / 8.15 ms / 494 | 0.94 ms / 2,086 ms / 16 (best trial **1,437**) |
| Existing path, growth at 1 row | 1.79 (20,000) | 5.68 (100,000) | 2.92 (100,000) | 2.82 (100,000) |
| Lower bound, growth at 1 row | not run | 1.00 | 0.86 | 3.60 (100,000) |
| `recover` of the 100,000-commit session | — | 19.5 s | 12.4 s | **1.89 s** |
| `sync_all` p50 on the medium | 0.43 ms | 0.61 ms | 0.52 ms | 0.64 ms |
| Host load during the run | ~66 (one reading) | not recorded | 93 to 183 | **25 to 177 (per stage)** |

## Probe's own verdict records (`verdict.jsonl`)

| Arm | Rows per commit | Verdict |
|---|---:|---|
| Existing path | 1 | **fail**, every line |
| Existing path | 100 | **fail** |
| Lower bound | 1 | **fail** (p99, rate, flat) |
| Lower bound | 100 | **fail** (p99, rate, flat) |

The judging rules are #911's probe's reading, not the owner's words.

## What differs from the earlier setup

- **Medium: local ext4 on virtio-blk** (no virtiofs, no NFS, no tmpfs in the sync path), instead of `/data/ssd-outputs` virtiofs on NTFS-via-fuseblk.
- **One medium per run**, not two (tmpfs skipped — tmpfs has free syncs and doesn't test the medium claim).
- **fdatasync baseline added** (4 KiB append + fdatasync × 2000), so the probe's commit cost can be read against the device's raw sync cost on the same medium.
- **Host load recorded before every stage** (in `host.txt`).

Unchanged: probe source byte-identical to #911, Tessera dev `81914f6` on Vortex `5c55cf7`, row shape, trial counts, SIGKILL recovery check, `WriteSession::recover` watermark + byte-equal fragments, `strace -c` of the sync calls.

## What this run does and does not say

- **The medium is not the bottleneck on this host.** `sync_all` p50 is 0.64 ms, inside the virtiofs band (0.43–0.61 ms); the fdatasync baseline on this medium is 0.72 ms p50. On the lower bound's one quiet trial at 100 rows per commit, the rate reached **1,437 commits/s** (2.9× the 500/s budget).
- **The run does not settle the budget on reference hardware**, because the host load was between 25 and 177 on 88 threads over the run's 105 minutes. Every stage with load above ~90 saw p99 tail latencies in the hundreds of milliseconds to seconds, and the fdatasync baseline alone recorded a 9.7 s maximum. The medians across trials are pulled down by the busy trials; the probe's "fail every line" verdict on this run reflects host contention, not the device.
- **The existing path fails every line in every one of the four runs so far**, by a wide margin, independent of medium or load. This is unchanged.
- **Recover improved dramatically** on local ext4 (1.89 s vs. 12.4 s on virtiofs for 100,000 commits).

## Not measured

A quiet host (either a dedicated run with the batch queue drained, or reference hardware outside ksb); power loss; a torn journal line; more than one lane or writer; group commit; growth at 100 rows per commit; #907's partial-fragment staging (not built). The fdatasync baseline is single-threaded on the same ext4.


---

# [Comment #6]() by [gerchowl]()

_Posted on November 21, 2026 at 09:23 PM_

**Two requirements from a kit consumer: one commit across several collections, and preconditions** (agent for the owner, 2026-11-21; from nerdmachines/kit#214, the document slice's requirements). Both extend points this issue already names.

1. **One commit record for rows in several collections.** A document is held in several keyed collections (blocks, edges, anchors, comments). A merge has to land in all of them as one version with one receipt, or not at all. Item 6 here says "One writer per collection. Commit order is log order." With a log per collection, such a commit has no single commit point before the flush. Request: the log's scope is a declared **group of collections** (one writer and one log for the group), and a commit record may carry rows for any of them. Replay applies a record to all its collections or to none. The flush then advances each collection's `current` under one parent collection (#947), so the sealed state is atomic too.

2. **Preconditions on a commit.** The Open section asks: "Is the row-version check done by the caller before the commit, or does the commit carry an expected version?" The consumer needs the second: a commit carries a list of (key, expected version) checks, the single writer evaluates them against its newest committed state, and a failed check refuses the whole commit with a typed error. This is what gives one head authority per document without a ref per document: the head is a row, and a merge commits "only if the head is still X". With one fenced writer the check needs no lock beyond the writer's own order.

Acceptance to add:
- [ ] A commit with rows for two collections of one group: after a crash at any point, both have it or neither has.
- [ ] Two commits with the same precondition on one key: exactly one succeeds, the other gets the typed error, in either order.

Not asked here: transactions that span groups, or more than one writer per group (#288).


