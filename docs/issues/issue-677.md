---
type: issue
state: closed
created: 2026-10-25T11:53:24Z
updated: 2026-10-29T12:09:49Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/677
comments: 3
labels: bug, priority:high, effort:large, area:io
assignees: none
milestone: none
projects: none
parent: none
children: 709, 710, 711, 712, 713, 714, 715, 716, 717
synced: 2026-10-30T08:02:27.148Z
---

# [Issue 677]: [feat(io): bounded single-table output sink and end-to-end StreamWriter backpressure](https://github.com/vig-os/tessera/issues/677)

Consumer: MorePET/visionx-rs#39, requiring one scan product with single named `pairs`, `markers`, `ms_stats` table blocks and a configurable buffer bound independent of input size. Audited dev / Cargo.lock revision `9424f90c88cee918f2c68784b2bb8d98291a4c53` (also current upstream dev).

Two gaps prevent that contract:

1. `tessera-io/src/table.rs::encode_streaming` returns `Result<Vec<u8>>`, writes all encoded table bytes into `ByteBufferMut`, then calls `freeze().to_vec()`. Input iteration is lazy, but encoded output memory grows with the whole table. `TableStreamWriter::finish` and `table_job_from_fragments` reach this path. `TableMultiBlockSink` bounds individual payloads by partitioning, but changes the requested logical block into suffixed physical blocks, and `with_block_rows` is not a consumer memory-budget API preserving the canonical layout.
2. `tessera-io/src/stream.rs::commit_loop` continuously drains the bounded output channel into an unrestricted `BTreeMap`. Deterministic counterexample: hold sequence 0 on one worker, let another worker complete sequences 1..N. The committer stores all N payloads while producers continue enqueueing, regardless of channel capacity. Channel capacity therefore does not bound total live encoded payloads.

Requested API: encode a single canonical table block to a file-backed sink (with file-backed encoded jobs/commit support) without materializing all output bytes; bound all submitted-but-uncommitted jobs, including pending reorder completions, with credits released on ordered commit. Preserve deterministic canonical bytes/content hashes across worker counts. Expose or instrument the high-water mark so consumers can test the contract.

Suggested regression tests: (a) stream a single table many times the configured budget into one named block and assert total buffered high-water stays within the documented bound; (b) block job 0 with a synchronization barrier while later jobs finish and assert outstanding submissions/completions cannot grow beyond the configured window, then release and verify ordered output. No timing-based assertion is needed.

This is distinct from #655 batch durability/performance and #654 query TableProvider. No Tessera internals have been reimplemented in the consumer; its brief explicitly requires stopping on a missing upstream API.

---

# [Comment #1]() by [gerchowl]()

_Posted on October 25, 2026 at 11:55 AM_

## Architect triage (2026-10-25): accepted, P0, next implementation slot

I verified both gaps at `9424f90`:
- `table.rs:996` `encode_streaming` returns `Vec<u8>` built by `buf.freeze().to_vec()` (:1037). Input is lazy, but
  the output grows with the whole table.
- `stream.rs` `commit_loop` drains the bounded channel into an unbounded `BTreeMap` reorder buffer. So the channel
  capacity doesn't bound the live payloads, and the counterexample (job 0 held, 1..N finish) holds.

This is a defect against ADR-0026 (bounded memory), not a new feature. The sealed bytes and `content_hash` must not
change, so this doesn't change the format and doesn't need a new ADR. The design below is decided, and ADR-0026
gets a dated amendment that records the contract.

**Decided design**
1. **File-backed encoded output.** A streaming encode writes to a sink (the target Vortex's `write` accepts at the
   pinned version; check it against `Cargo.lock`), not to a `Vec`. `encode_streaming` keeps its signature as a thin
   wrapper over the sink, so no caller breaks. Encoded jobs that don't fit the budget live in files in the write
   session's staging directory, never in RAM.
2. **Streaming commit.** `WriteSession` gets a reader-based append (bytes streamed from the staged file, hashed on
   write: ADR-0002 first-moment integrity), alongside today's `append_block(&[u8])`. One logical block stays one
   canonical block: no suffixed physical blocks, and `TableMultiBlockSink` is not the answer to this.
3. **Credit window.** One configurable bound, `max_in_flight` (jobs, plus a byte budget), is acquired when a job is
   submitted and released only on its **ordered commit**. It covers the queued, encoding and reorder-pending jobs.
   A producer blocks when it runs out of credits. Determinism across worker counts is unchanged: the commit order
   is still `seq`.
4. **Instrumentation.** `finish` (or a stats accessor) reports the high-water marks: jobs in flight, and bytes held
   in RAM. The documented bound is a formula over the config knobs, stated in the rustdoc and in the ADR-0026
   amendment.
5. **Tests**, timing-free and failing before the fix:
   (a) one named table block, streamed at many times the budget, stays within the documented high-water bound, and
       its bytes equal the batch `encode`;
   (b) job 0 is held at a barrier while the later jobs finish, the outstanding jobs never pass the window, and on
       release the output is ordered;
   (c) the content hash is the same for 1, 4 and N workers;
   plus an RSS proof as a ksb job, measuring anon memory and not page cache, on a table larger than the budget.

**Ordering:** this is the next implementation PR, ahead of #655 and the #628 spike. It starts once the current
in-flight PRs (#664, then #660, #662, #661) have landed, so there's no file overlap. #656 (with #566 and a
security pre-mortem) follows as an ADR. #654's implementation waits on ADR-0062 (#661) being accepted by the owner.

**Consumer acceptance:** yes, please contribute the reproducer (the 19 GB list-mode, under 8 GB RSS). It can be a
script or a test we run as a ksb job before merge, not a CI test, since the data is too large. A synthetic
generator with the same shape is ideal.


---

# [Comment #2]() by [gerchowl]()

_Posted on October 25, 2026 at 12:12 PM_

Consumer reproducer is pushed on `MorePET/visionx-rs` branch `chore/39-tessera-677-repro`, commit `5ce849ee87f7ab808749f7e2bddc0d3dbac42e82`, draft PR https://github.com/MorePET/visionx-rs/pull/41 (keep draft; #39 remains on hold).

- Example: `crates/visionx-tsra/examples/stream_memory.rs`
- Complete commands, KSB packaging/submission and patch overrides: https://github.com/MorePET/visionx-rs/blob/chore/39-tessera-677-repro/docs/implementation/39-memory.md
- No patient input or dataset; deterministic seed and bounded native batches matching the decoder Arrow column types/domains. Default is 2.4G events / 19.2GB packet-equivalent input, with one named `pairs` table and small `markers`/`ms_stats` tables.

**Additional blocker discovered at 9424f90:** the exact decoder schema has nullable `ms`. `encode_streaming` constructs the empty stream dtype with bare nonnullable columns, but receives nullable chunks. At only 65,536 rows this hits the Vortex `sequence.rs:312` dtype assertion (`ms=u32?` vs `ms=u32`). Before adding the fatal panic hook, this background-thread panic was swallowed: the caller sealed a file and returned exit 0. The harness now exits 1 on any panic. Please cover nullable columns in the file-backed streaming regression too.

An explicit `--nonnullable-ms` diagnostic removes only that nullable property to isolate the memory defect at your current revision. These measurements are **not** an exact-schema acceptance pass.

Build and reproduce safely on a seat:

```bash
git clone --branch chore/39-tessera-677-repro https://github.com/MorePET/visionx-rs /tmp/visionx-677-repro
cd /tmp/visionx-677-repro
export CARGO_TARGET_DIR=/tmp/claude-1004/visionx-target/39 CARGO_BUILD_JOBS=8
nix develop -c cargo build --locked --release -p visionx-tsra --example stream_memory
# Exact nullable shape: expected exit 1 at 9424f90.
"$CARGO_TARGET_DIR/release/examples/stream_memory" --rows 65536 --output /tmp/677-nullable.tsra
# Isolated output-memory defect: 2 GiB packet-equivalent input, 8 GiB VA cap.
watcher-s1 --no-s1 --pipe --events /tmp/677-memory.events --   bash -c 'ulimit -v 8388608; exec "$1" --rows 268435456 --nonnullable-ms --rss-budget 6442450944 --output /tmp/677-memory.tsra'   _ "$CARGO_TARGET_DIR/release/examples/stream_memory"
# Public API reorder reproduction: expected exit 0 while defect is present.
"$CARGO_TARGET_DIR/release/examples/stream_memory" --reorder --workers 2 --output /tmp/677-reorder.tsra
```

Measured locally under watcher-s1 at pinned `9424f90c88cee918f2c68784b2bb8d98291a4c53`, with 4 configured encode workers, 8 GiB virtual-address cap and 6 GiB anonymous budget (shared seat CPUs, no pinned-core reservation):

| Packet-equivalent input bytes | Peak RssAnon bytes | Sealed output bytes | Seconds |
|---:|---:|---:|---:|
| 268,435,456 | 656,941,056 | 256,088,980 | 5.234 |
| 536,870,912 | 1,226,682,368 | 512,143,652 | 9.854 |
| 1,073,741,824 | 2,307,702,784 | 1,024,252,741 | 20.459 |
| 2,147,483,648 | 4,758,114,304 | 2,048,470,381 | 40.803 |

All four memory-probe exits were 0. Largest-run curve: 90,955,776 anon bytes at 7,929,856 rows; 1,051,115,520 at 126,943,232; 2,355,593,216 at 266,403,840; peak 4,758,114,304 during finalization. Throughput 6.58M events/s. `RssAnon` is sampled every 10ms, excluding file-backed page cache; `VmHWM` (total resident, not the anon criterion) reached 4,783,673,344. No full 19GB seat run was attempted.

Gap 2 is externally reproducible: while sequence zero is held on a condition variable, 128 later jobs complete with two workers and channel capacity two. With no public pending counter, conservatively subtracting worker/output slots gives pending lower bound **124**. The watchdog only limits waiting; it cannot cause a false reproduction. The exact timing-free pending count remains an in-crate upstream test.

The README gives a validated temporary Nix app wrapper for the already built binary and this exact large-job submission (no dataset):

```bash
ksb run --project dogplet-analysis --cpu 4 --mem 12Gi --time 2h   --no-egress "$jobdir#default" -- --rows 2400000000 --rss-budget 8589934592
```

For 9424f90's isolated memory diagnostic append `--nonnullable-ms`; for full acceptance use the nullable default after fixing that stream dtype. We validated wrapper execution and KSB dry-run admission, not a full 19GB job.

For a local upstream PR checkout, build the consumer with:

```bash
nix develop -c cargo build --release -p visionx-tsra --example stream_memory   --config 'patch."https://github.com/vig-os/tessera".tessera-core.path="/absolute/tessera/tessera/crates/tessera-core"'   --config 'patch."https://github.com/vig-os/tessera".tessera-io.path="/absolute/tessera/tessera/crates/tessera-io"'
```

After #677 lands we must switch the example's single encode job to the new file-backed API: retaining `encode_streaming`'s compatibility Vec wrapper would deliberately retain output materialization. The generator, RSS-budget assertion and job wrapper are reusable acceptance scaffolding. The old-defect reorder diagnostic will likewise switch to the new high-water assertions.

Consumer checks: release example tests (3) and precommit pass. `cargo deny` currently blocks `just rust-check` on Tessera-IO's transitive unmaintained `paste` (RUSTSEC-2024-0436); no policy exemption was added.


---

# [Comment #3]() by [gerchowl]()

_Posted on October 29, 2026 at 12:09 PM_

**Owner decision (2026-10-29), from the #681 design note (PR #692, b6de2d1):** option **P-logical**. Streamed tables above `BLOCK_ROWS` are written as hierarchical leaves `pairs/0000`, `pairs/0001`, … (ADR-0063), and every logical reader presents `pairs` as one table. This retires ADR-0026's "one canonical block of any size" for streamed tables, and #677's literal "single physical block" requirement with it. The memory bound (flat 128–143 MB from 67 M to 1.07 G rows, W=4) is in the note. Next: a new ADR (amends SPEC §5b, ADR-0026, ADR-0062's `RowAlignment` and ADR-0063), which folds in the re-review nits recorded on #681, then the implementation. The Vortex writer gate F stays a separate upstream track (vortex#10046).

