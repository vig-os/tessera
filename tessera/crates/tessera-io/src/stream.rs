//! Bounded-memory streaming write pipeline (ROADMAP P3 / #203) — the DAQ-facing front of
//! [`WriteSession`].
//!
//! The DAQ rule (S17): never encode on the producer's hot path. [`StreamWriter`] decouples the
//! producer from encoding with a bounded pipeline:
//!
//! ```text
//!   push(job) ──▶ job queue ──▶ N encode workers ──▶ ordered committer ──▶ WriteSession
//!   (producer)                  (parallel encode)    (reorder by seq,      (durable frag +
//!                                                     append in push order) journal + Merkle)
//!      └──────────── one credit window over the whole span (#677) ────────────┘
//! ```
//!
//! - **Bounded RAM (the credit window, #677 / ADR-0026 "StreamWriter memory contract"):** a job takes
//!   a credit when it is **pushed** and gives it back only when it is **committed in order**, so one
//!   [`StreamWindow`] bounds every job the pipeline holds — queued, encoding, *and* finished but
//!   waiting in the reorder buffer for an earlier job. `push` blocks while the window is full. The
//!   bound, over [`StreamWindow::max_in_flight`] = `J`, [`StreamWindow::max_in_flight_bytes`] = `B`,
//!   the worker count `W` and the largest single job's encoded payload `P` (data block plus its
//!   companions):
//!   - jobs pushed but not yet committed `≤ J`, always;
//!   - encoded payload bytes held in RAM `< B + min(W, J) × P` (and trivially `≤ J × P`).
//!
//!   A **staged** job ([`StagedEncodeJob`], pushed with [`StreamWriter::push_staged`]) writes its
//!   data block to a file in the session's staging directory instead of RAM, so only its companions
//!   count towards `P` — zero for [`StreamWriter::push_table`]'s leaves and
//!   [`table_job_from_fragments`]: a table block's encoded bytes are on disk, not in the window
//!   (#677). A table above [`BLOCK_ROWS`] rows is written by `push_table` as the leaves of one
//!   logical table (ADR-0064), each leaf one staged job. Disk is not bounded by `B`: the staged files of finished-but-uncommitted
//!   jobs hold `≤ J × S` bytes (`S` = the largest staged block, [`StreamStats::max_staged_job_bytes`]),
//!   plus the files of the encodes in progress.
//! - **Parallel encode:** `workers` threads encode concurrently (the expensive pcodec/Vortex step).
//! - **Deterministic output:** the committer commits blocks in **push order** (a reorder buffer holds
//!   out-of-order completions), so the running Merkle — and therefore the sealed `content_hash` /
//!   `id` / `manifest_hash` — is **byte-identical to a batch writer** that added the same blocks in
//!   the same order. Streaming changes *how* the bytes are produced, never *which* bytes; neither
//!   the worker count nor the window changes them.
//!
//! An in-memory job ([`EncodeJob`]) still materialises its encoded payload in RAM (`P` above); a
//! table block too large for that goes through a staged job. Staged blocks are committed with
//! [`WriteSession::append_staged`], which renames the file into place as the fragment after checking
//! it against the digest it was born with (hashed as the encoder wrote it). A staged block is
//! byte-identical to the in-memory encode of the same table, so the sealed product does not depend on
//! which kind of job wrote it.
//!
//! Neither bound counts the encoders' own working sets (one row-group of input, the compressor's
//! buffers, and the encoded segments the Vortex writer holds before it writes them — about 0.4 × a
//! table's encoded size at its peak, a known limit tracked in #681, see
//! [`crate::table::encode_streaming_to`]) or the committer's fixed 64 KiB verify buffer.
//!
//! # How the window is enforced
//!
//! Two gates, both on one mutex + condvar:
//! 1. **Submit** ([`StreamWriter::push`]): a job is admitted while fewer than `J` jobs are in flight
//!    *and* the encoded bytes held are below `B`. With nothing in flight the bytes held are `0`, so a
//!    job is always admitted into an empty window — including one whose payload alone exceeds `B`.
//! 2. **Encode start** (a worker, after dequeuing job `seq`): it starts encoding only while the
//!    encoded bytes held are below `B` — **unless `seq` is the job the committer waits on**, which
//!    always runs. This is what makes `B` bind: once `B` is reached, only the in-progress encodes
//!    (at most `min(W, J)`) can still add bytes.
//!
//! **Deadlock freedom.** Credits are released only by an in-order commit, so the window must never
//! stall the job the committer waits on (`next`). It can't: `next` was pushed (every job below the
//! newest push was), the job queue is FIFO so `next` is dequeued before any later job, and gate 2
//! exempts it. So `next` always encodes, commits, and frees its credit; a job too large for `B` simply
//! runs alone once it is `next`. Gate 1 never waits on an empty window. A failed encode, a failed
//! commit or a panicking worker **closes** the window, which wakes every waiter: `push` then returns
//! an error and the workers waiting at gate 2 exit, so a dead pipeline never hangs the producer
//! (idle workers exit once [`StreamWriter::finish`] or a drop closes the job queue).

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};

use tessera_core::block::table::TableSpec;
use tessera_core::block::{BlockName, BlockRef, StreamedTableError};
use tessera_core::manifest::Manifest;
use tessera_core::{Error, Result};

use crate::accumulate::{read_fragment_from, write_fragment_to};
use crate::table::{
    block_name, block_partition, partition_blocks, ColumnData, TableData, BLOCK_ROWS,
    ROWS_PER_GROUP,
};
use crate::write::{BlockStaging, StagedBlock, WriteSession};
use crate::BlockPayload;

/// A committed block: its manifest reference paired with its payload bytes.
type EncodedBlock = (BlockRef, BlockPayload);

/// A unit of work the pipeline encodes off the hot path. The producer hands the *raw* data captured
/// in a closure; the encode (pcodec/Vortex) runs on a worker thread. Returns the data block plus any
/// **additive companion blocks** (e.g. an ADR-0028 §5 chunk-index sidecar) to commit right after it, in
/// order. Use [`array_job`]/[`table_job`] (no companions) or the `*_indexed` variants (with a sidecar).
pub type EncodeJob =
    Box<dyn FnOnce() -> Result<(EncodedBlock, Vec<EncodedBlock>)> + Send + 'static>;

/// A data block whose encoded bytes are in a [`StagedBlock`] (a file), not in RAM. Its
/// [`BlockRef::digest`] may be `None`: the commit fills in the digest the [`StagedBlock`] computed as
/// the bytes were written (a digest that is set must match it).
pub type StagedEncodedBlock = (BlockRef, StagedBlock);

/// A unit of work like [`EncodeJob`] whose **data block is written to a file** in the write
/// session's staging directory rather than returned in RAM: the worker hands the job the session's
/// [`BlockStaging`], the job writes its block into a [`BlockStaging::create`]d [`StagedBlock`], and
/// the committer renames that file into the product ([`WriteSession::append_staged`]). Companions
/// stay in RAM. Push it with [`StreamWriter::push_staged`]. Use it for a block that may not fit in
/// memory — [`table_job_streaming`] and [`table_job_from_fragments`] are staged jobs.
pub type StagedEncodeJob = Box<
    dyn FnOnce(&BlockStaging) -> Result<(StagedEncodedBlock, Vec<EncodedBlock>)> + Send + 'static,
>;

/// The table block's manifest reference without a digest (a staged commit fills it in).
fn staged_table_ref(
    name: String,
    spec: &tessera_core::block::table::TableSpec,
) -> Result<BlockRef> {
    Ok(BlockRef {
        name: tessera_core::block::BlockName::new(name)?,
        kind: tessera_core::block::BlockKind::Table,
        digest: None,
        spec: serde_json::to_value(spec)?,
    })
}

/// Wrap ONE canonical table block of at most [`crate::table::BLOCK_ROWS`] rows as a
/// [`StagedEncodeJob`]: the job pulls `groups` (row-groups of ≤ [`crate::table::ROWS_PER_GROUP`]
/// rows, in order, totalling `spec.rows`) lazily through [`crate::table::encode_streaming_to`] into a
/// staged file, so neither the input nor the encoded output is collected in RAM. The committed block
/// is byte-identical to [`crate::table::encode`] of the concatenated rows (and to [`table_job`] of
/// them), so its digest and the product's `content_hash` do not depend on the job kind.
///
/// **Deprecated (ADR-0064 §5.4):** a table above `BLOCK_ROWS` rows is partitioned into the leaves
/// of one logical table, which [`StreamWriter::push_table`] writes. Above `BLOCK_ROWS` the job fails
/// with [`Error::StreamedTable`] ([`StreamedTableError::NeedsPartition`]) before it stages anything.
/// The job runs on a worker, so the error surfaces at [`StreamWriter::finish`], not at this call or
/// at [`StreamWriter::push_staged`]. The "any size" contract of alpha.4 is withdrawn. At or below
/// it the block is unchanged, and `push_table` writes the same bytes.
#[deprecated(
    note = "use StreamWriter::push_table, which partitions above BLOCK_ROWS (ADR-0064 §5.4)"
)]
pub fn table_job_streaming<I>(
    name: impl Into<String>,
    spec: tessera_core::block::table::TableSpec,
    groups: I,
) -> StagedEncodeJob
where
    I: IntoIterator<Item = crate::table::TableData> + Send + 'static,
    I::IntoIter: Send + 'static,
{
    let name = name.into();
    Box::new(move |staging| {
        if spec.rows > BLOCK_ROWS as u64 {
            return Err(Error::StreamedTable {
                name,
                reason: StreamedTableError::NeedsPartition {
                    rows: spec.rows,
                    max: BLOCK_ROWS as u64,
                },
            });
        }
        let mut staged = staging.create()?;
        crate::table::encode_streaming_to(&spec, groups, &mut staged)?;
        Ok(((staged_table_ref(name, &spec)?, staged), Vec::new()))
    })
}

/// Wrap an array block as an [`EncodeJob`] (encoding deferred to a worker). No companion blocks — the
/// streamed bytes stay byte-identical to a batch write.
pub fn array_job(
    name: impl Into<String>,
    spec: tessera_core::block::array::ArraySpec,
    data: crate::array::ArrayData,
) -> EncodeJob {
    let name = name.into();
    Box::new(move || Ok((crate::array::array_block(&name, &spec, &data)?, Vec::new())))
}

/// Like [`array_job`] but also emits the block's `{hash, stats}` chunk-index **sidecar** (ADR-0028 §5)
/// as an additive companion, committed right after the data block. Opt-in: only `*_indexed` jobs add a
/// sidecar, so plain streaming stays byte-identical to batch. Float arrays emit no sidecar.
pub fn array_job_indexed(
    name: impl Into<String>,
    spec: tessera_core::block::array::ArraySpec,
    data: crate::array::ArrayData,
) -> EncodeJob {
    let name = name.into();
    Box::new(move || {
        let (block, sidecar) = crate::array::array_block_with_index(&name, &spec, &data)?;
        Ok((block, sidecar.into_iter().collect()))
    })
}

/// Wrap a table block as an [`EncodeJob`] (no companions).
pub fn table_job(
    name: impl Into<String>,
    spec: tessera_core::block::table::TableSpec,
    data: crate::table::TableData,
) -> EncodeJob {
    let name = name.into();
    Box::new(move || Ok((crate::table::table_block(&name, &spec, &data)?, Vec::new())))
}

/// Like [`table_job`] but also emits the table's chunk-index sidecar over `stat_column` (ADR-0028 §5),
/// when that names an integer column. Opt-in, additive companion.
pub fn table_job_indexed(
    name: impl Into<String>,
    spec: tessera_core::block::table::TableSpec,
    data: crate::table::TableData,
    stat_column: Option<String>,
) -> EncodeJob {
    let name = name.into();
    Box::new(move || {
        let (block, sidecar) =
            crate::table::table_block_with_index(&name, &spec, &data, stat_column.as_deref())?;
        Ok((block, sidecar.into_iter().collect()))
    })
}

/// Wrap a table block as a [`StagedEncodeJob`] that **lazily reads row-group fragments from disk**
/// — the multi-block listmode peer of [`table_job_streaming`]. The job pulls one fragment at a time
/// (the crate's fragment reader) into [`crate::table::encode_streaming_to`], writing the
/// encoded block to a staged file, so the per-block encode holds ≈ one row-group of input (plus the
/// Vortex working set, #681) and doesn't collect the output. This is the piece that lets the multi-block sink
/// dispatch one parallel encode job per block to [`StreamWriter`] without holding a block payload
/// in RAM.
///
/// Inputs:
/// - `name`: the manifest block name (already chosen via [`crate::table::block_name`]).
/// - `spec`: the per-block [`TableSpec`] — its `rows` MUST equal the sum of the fragments' rows.
/// - `frag_paths`: row-group fragment file paths in append order (the order the encoder consumes
///   them).
///
/// A fragment that can't be read fails the job with that error (its staged file is deleted).
///
/// [`TableSpec`]: tessera_core::block::table::TableSpec
pub fn table_job_from_fragments(
    name: impl Into<String>,
    spec: tessera_core::block::table::TableSpec,
    frag_paths: Vec<std::path::PathBuf>,
) -> StagedEncodeJob {
    let name = name.into();
    Box::new(move |staging| {
        let mut staged = staging.create()?;
        crate::accumulate::compact_fragments_to(&spec, frag_paths, &mut staged)?;
        Ok(((staged_table_ref(name, &spec)?, staged), Vec::new()))
    })
}

/// A job of either kind, as the pipeline queues it.
enum Job {
    InMemory(EncodeJob),
    Staged(StagedEncodeJob),
}

impl Job {
    fn run(self, staging: &BlockStaging) -> Result<JobOutput> {
        match self {
            Job::InMemory(job) => job().map(|(data, companions)| JobOutput {
                data: DataBlock::InMemory(data),
                companions,
            }),
            Job::Staged(job) => job(staging).map(|(data, companions)| JobOutput {
                data: DataBlock::Staged(data),
                companions,
            }),
        }
    }
}

/// A job's data block: its bytes in RAM, or in a staged file.
enum DataBlock {
    InMemory(EncodedBlock),
    Staged(StagedEncodedBlock),
}

/// The encoded result of one job: the data block plus its (in-RAM) companions.
struct JobOutput {
    data: DataBlock,
    companions: Vec<EncodedBlock>,
}

impl JobOutput {
    /// What the job holds until its commit: `(bytes in RAM, bytes in staged files)`.
    fn held(&self) -> Held {
        let companions: u64 = self
            .companions
            .iter()
            .map(|(_, payload)| payload.bytes.len() as u64)
            .sum();
        match &self.data {
            DataBlock::InMemory((_, payload)) => Held {
                ram: companions + payload.bytes.len() as u64,
                staged: 0,
            },
            DataBlock::Staged((_, staged)) => Held {
                ram: companions,
                staged: staged.len(),
            },
        }
    }
}

/// The bytes a finished job holds until it is committed: encoded payloads in RAM (charged to the
/// window's byte budget) and staged files on disk (reported, not budgeted).
#[derive(Clone, Copy, Default)]
struct Held {
    ram: u64,
    staged: u64,
}

/// What a worker hands the committer: the job's `seq`, the bytes it charged to the window (so the
/// commit can give exactly that back), and the encode result.
type Encoded = (usize, Held, Result<JobOutput>);

/// The credit window bounding everything a [`StreamWriter`] holds between [`StreamWriter::push`]
/// and the in-order commit of that job (#677). See the [module docs](self) for the bound it gives
/// and how it is enforced.
///
/// Build it with [`StreamWindow::new`]; the fields are public to read. Both limits are clamped to
/// at least 1, so a window can always admit a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct StreamWindow {
    /// The most jobs pushed but not yet committed (queued + encoding + waiting in the reorder
    /// buffer). [`StreamWriter::push`] blocks while this many are in flight. Always ≥ 1.
    pub max_in_flight: usize,
    /// The encoded-payload byte budget (data blocks + companions) the window aims to hold in RAM.
    /// While the bytes of encoded-but-uncommitted jobs are at or above it, `push` blocks and
    /// workers start no new encode other than the one the committer waits on. It is a soft
    /// ceiling: the in-progress encodes can overshoot it by at most `min(workers, max_in_flight)`
    /// payloads, and a single job larger than the budget still runs (alone). Always ≥ 1.
    pub max_in_flight_bytes: u64,
}

impl StreamWindow {
    /// A window of `max_in_flight` jobs and `max_in_flight_bytes` encoded bytes, each clamped to ≥ 1.
    pub fn new(max_in_flight: usize, max_in_flight_bytes: u64) -> Self {
        StreamWindow {
            max_in_flight: max_in_flight.max(1),
            max_in_flight_bytes: max_in_flight_bytes.max(1),
        }
    }
}

/// High-water marks and counters of a [`StreamWriter`]'s credit window, so a caller can assert the
/// memory contract (#677). Read it live with [`StreamWriter::stats`] or as the final account from
/// [`StreamWriter::finish_with_stats`].
///
/// Against the writer's [`StreamWindow`] (`J` jobs, `B` bytes), `W` workers and the largest single
/// job's encoded payload `P`: `peak_in_flight_jobs ≤ J` and `peak_in_flight_bytes < B + min(W, J) × P`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct StreamStats {
    /// The most jobs that were pushed but not yet committed at any one time.
    pub peak_in_flight_jobs: usize,
    /// The most encoded payload bytes (data blocks + companions) held at any one time by jobs that
    /// had finished encoding but were not yet committed.
    pub peak_in_flight_bytes: u64,
    /// The largest encoded payload a single job held in RAM (data block + companions; a staged
    /// job's data block is on disk, so only its companions count) — `P` in the bound.
    pub max_job_bytes: u64,
    /// Jobs pushed so far.
    pub jobs_pushed: usize,
    /// Jobs committed to the session so far, in push order.
    pub jobs_committed: usize,
    /// How many [`StreamWriter::push`] calls had to wait for a credit (backpressure events).
    pub producer_stalls: u64,
    /// The most bytes held at any one time in the staged files of jobs that had finished encoding
    /// but were not yet committed — disk, not RAM, so not part of `peak_in_flight_bytes`.
    pub peak_staged_bytes: u64,
    /// The largest staged data block of a single job (`S` in the disk bound `≤ J × S`).
    pub max_staged_job_bytes: u64,
    /// The most blocks of [`StreamWriter::push_table`] input staged on disk at one time: a block
    /// counts from the creation of its staging file, while it is still being filled, until its
    /// worker deletes the file (ADR-0064 §8). At most `W + 1` for `W` workers.
    pub peak_staged_input_blocks: usize,
    /// The most bytes of [`StreamWriter::push_table`] input staged on disk at one time, the block
    /// being filled included: the staging files' sizes, updated as each row-group is written.
    pub peak_staged_input_bytes: u64,
}

/// The window's mutable state, behind [`CreditWindow::state`].
struct WindowState {
    /// Jobs pushed and not yet committed.
    in_flight_jobs: usize,
    /// Encoded payload bytes of jobs that finished encoding and are not yet committed.
    in_flight_bytes: u64,
    /// Staged-file bytes of jobs that finished encoding and are not yet committed.
    staged_bytes: u64,
    /// The `seq` the committer commits next — the job the window must never stall.
    next_commit: usize,
    /// Set when the pipeline can no longer make progress (encode/commit error, worker panic,
    /// committer exit); every waiter gives up.
    closed: bool,
    /// Workers currently waiting at gate 2 (tests observe it; it is not part of [`StreamStats`]).
    gated_encodes: usize,
    /// Jobs admitted whose `run` has not returned (queued or on a worker); a worker is free while
    /// this is below the worker count.
    busy_jobs: usize,
    /// [`StreamWriter::push_table`] input staged on disk now: files and their bytes.
    staged_input_blocks: usize,
    staged_input_bytes: u64,
    /// Set by a [`ReadHold`]: `push_table` jobs wait before reading their input.
    hold_reads: bool,
    stats: StreamStats,
}

/// The shared credit window: the limits, the state, and one condvar every gate waits on.
struct CreditWindow {
    limits: StreamWindow,
    workers: usize,
    state: Mutex<WindowState>,
    changed: Condvar,
}

impl CreditWindow {
    fn new(limits: StreamWindow, workers: usize) -> Self {
        CreditWindow {
            limits,
            workers,
            state: Mutex::new(WindowState {
                in_flight_jobs: 0,
                in_flight_bytes: 0,
                staged_bytes: 0,
                next_commit: 0,
                closed: false,
                gated_encodes: 0,
                busy_jobs: 0,
                staged_input_blocks: 0,
                staged_input_bytes: 0,
                hold_reads: false,
                stats: StreamStats::default(),
            }),
            changed: Condvar::new(),
        }
    }

    /// The state is plain counters updated under the lock without any panicking code, so a
    /// poisoned lock (a panic elsewhere on that thread) still holds consistent state — take it,
    /// so [`Self::close`] works while a worker unwinds.
    fn lock(&self) -> MutexGuard<'_, WindowState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn wait<'a>(&self, guard: MutexGuard<'a, WindowState>) -> MutexGuard<'a, WindowState> {
        self.changed
            .wait(guard)
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Gate 1 (producer): wait for a job credit — and, with `free_worker`, for a worker with no
    /// job — then take it. `Err` once the window is closed.
    fn acquire(&self, free_worker: bool) -> Result<()> {
        let mut st = self.lock();
        let mut stalled = false;
        loop {
            if st.closed {
                return Err(closed_error());
            }
            // With no job in flight no bytes are held, so an empty window always admits.
            if st.in_flight_jobs < self.limits.max_in_flight
                && st.in_flight_bytes < self.limits.max_in_flight_bytes
                && (!free_worker || st.busy_jobs < self.workers)
            {
                break;
            }
            if !stalled {
                stalled = true;
                st.stats.producer_stalls += 1;
            }
            st = self.wait(st);
        }
        st.in_flight_jobs += 1;
        st.busy_jobs += 1;
        st.stats.jobs_pushed += 1;
        st.stats.peak_in_flight_jobs = st.stats.peak_in_flight_jobs.max(st.in_flight_jobs);
        Ok(())
    }

    /// Gate 2 (worker): wait until job `seq` may start encoding — always if it is the job the
    /// committer waits on, otherwise while the held bytes are below the budget. `false` once the
    /// window is closed (the worker should exit).
    fn wait_encode_turn(&self, seq: usize) -> bool {
        let mut st = self.lock();
        let mut gated = false;
        let turn = loop {
            if st.closed {
                break false;
            }
            if seq == st.next_commit || st.in_flight_bytes < self.limits.max_in_flight_bytes {
                break true;
            }
            if !gated {
                gated = true;
                st.gated_encodes += 1;
            }
            st = self.wait(st);
        };
        if gated {
            st.gated_encodes -= 1;
        }
        turn
    }

    /// A worker finished encoding a job that holds `held`: hold it until the job's commit. The
    /// worker is free again.
    fn charge(&self, held: Held) {
        let mut st = self.lock();
        st.busy_jobs -= 1;
        st.in_flight_bytes += held.ram;
        st.staged_bytes += held.staged;
        st.stats.peak_in_flight_bytes = st.stats.peak_in_flight_bytes.max(st.in_flight_bytes);
        st.stats.max_job_bytes = st.stats.max_job_bytes.max(held.ram);
        st.stats.peak_staged_bytes = st.stats.peak_staged_bytes.max(st.staged_bytes);
        st.stats.max_staged_job_bytes = st.stats.max_staged_job_bytes.max(held.staged);
        drop(st);
        self.changed.notify_all();
    }

    /// Staged input changed by `files` files and `bytes` bytes (created/grown, or deleted).
    fn stage_input(&self, files: isize, bytes: i64) {
        let mut st = self.lock();
        st.staged_input_blocks = st.staged_input_blocks.saturating_add_signed(files);
        st.staged_input_bytes = st.staged_input_bytes.saturating_add_signed(bytes);
        st.stats.peak_staged_input_blocks = st
            .stats
            .peak_staged_input_blocks
            .max(st.staged_input_blocks);
        st.stats.peak_staged_input_bytes =
            st.stats.peak_staged_input_bytes.max(st.staged_input_bytes);
    }

    /// Set or clear the [`ReadHold`].
    fn hold_reads(&self, hold: bool) {
        self.lock().hold_reads = hold;
        self.changed.notify_all();
    }

    /// A `push_table` job may read its input: wait while a [`ReadHold`] is held. `false` once the
    /// window is closed.
    fn wait_read_turn(&self) -> bool {
        let mut st = self.lock();
        while st.hold_reads && !st.closed {
            st = self.wait(st);
        }
        !st.closed
    }

    /// The committer appended job `next_commit` (which held `held`): give its credit back.
    fn release(&self, held: Held) {
        let mut st = self.lock();
        st.in_flight_jobs -= 1;
        st.in_flight_bytes -= held.ram;
        st.staged_bytes -= held.staged;
        st.next_commit += 1;
        st.stats.jobs_committed += 1;
        drop(st);
        self.changed.notify_all();
    }

    /// The pipeline can make no more progress: wake every waiter so none hangs.
    fn close(&self) {
        self.lock().closed = true;
        self.changed.notify_all();
    }

    fn stats(&self) -> StreamStats {
        self.lock().stats
    }
}

fn closed_error() -> Error {
    Error::Container("stream pipeline closed (a worker/committer died)".into())
}

/// Closes the window when dropped — always (the committer: once it returns, by error or not, no
/// credit is released again) or only while unwinding (a worker: a panic loses the result of the
/// job it holds, whose credit then never returns). Either way no producer or worker is left
/// waiting on a pipeline that can't progress. A worker's *normal* exit must not close: it means
/// the input drained, and a sibling may still hold a job waiting at the encode gate.
struct CloseOnDrop {
    window: Arc<CreditWindow>,
    only_on_panic: bool,
}

impl Drop for CloseOnDrop {
    fn drop(&mut self) {
        if !self.only_on_panic || thread::panicking() {
            self.window.close();
        }
    }
}

/// The rows of `group` after checking its columns against `spec` (names, order, equal lengths).
fn group_rows(spec: &TableSpec, group: &TableData) -> Result<usize> {
    if group.len() != spec.columns.len() {
        return Err(Error::Codec("push_table: column count mismatch".into()));
    }
    let rows = rows_of(group);
    for ((name, data), col) in group.iter().zip(&spec.columns) {
        if name != &col.name {
            return Err(Error::Codec(format!(
                "push_table: column '{name}' != spec '{}'",
                col.name
            )));
        }
        if data.len() != rows {
            return Err(Error::Codec("push_table: ragged group columns".into()));
        }
    }
    Ok(rows)
}

fn rows_of(group: &TableData) -> usize {
    group.first().map_or(0, |(_, c)| c.len())
}

/// The block [`StreamWriter::push_table`] is filling: its index, its staging file and its rows.
#[derive(Default)]
struct Fill {
    block: u64,
    input: Option<StagedInput>,
    rows: u64,
}

/// One block's staged input: consecutive fragment records in a file of the session's staging
/// directory. It counts in the window's staged input from its creation until it is dropped, which
/// deletes the file — once its worker has read the last record, or on any error or abort.
struct StagedInput {
    path: PathBuf,
    out: Option<BufWriter<File>>,
    groups: usize,
    bytes: u64,
    window: Arc<CreditWindow>,
}

impl StagedInput {
    fn create(staging: &BlockStaging, window: &Arc<CreditWindow>) -> Result<Self> {
        let path = staging.input_path();
        let out = BufWriter::new(File::create(&path)?);
        window.stage_input(1, 0);
        Ok(StagedInput {
            path,
            out: Some(out),
            groups: 0,
            bytes: 0,
            window: Arc::clone(window),
        })
    }

    /// Write one row-group, flushed so the file's size on disk is what the window counts.
    fn append(&mut self, group: &TableData) -> Result<()> {
        let out = self.out.as_mut().expect("still being filled");
        let n = write_fragment_to(out, group)?;
        out.flush()?;
        self.groups += 1;
        self.bytes += n;
        self.window.stage_input(0, n as i64);
        Ok(())
    }

    /// The leaf's job: wait out a [`ReadHold`], then encode the records one at a time into a
    /// staged block, deleting this file once the last record is read.
    fn into_job(mut self, bref: BlockRef, spec: TableSpec) -> Result<StagedEncodeJob> {
        drop(
            self.out
                .take()
                .expect("still being filled")
                .into_inner()
                .map_err(|e| e.into_error())?,
        );
        Ok(Box::new(move |staging| {
            if !self.window.wait_read_turn() {
                return Err(closed_error());
            }
            let groups = self.groups;
            let reader = BufReader::new(File::open(&self.path)?);
            let mut source = Some((reader, self));
            let mut staged = staging.create()?;
            let read = move |i: usize, columns: &[tessera_core::block::table::Column]| {
                let (reader, input) = source.as_mut().expect("one read per record");
                let group = read_fragment_from(reader, columns, &input.path);
                if i + 1 == groups {
                    source = None; // read in full: delete it
                }
                group
            };
            crate::accumulate::compact_to(&spec, 0..groups, read, &mut staged)?;
            Ok(((bref, staged), Vec::new()))
        }))
    }
}

impl Drop for StagedInput {
    fn drop(&mut self) {
        drop(self.out.take());
        let _ = fs::remove_file(&self.path);
        self.window.stage_input(-1, -(self.bytes as i64));
    }
}

/// Holds [`StreamWriter::push_table`]'s jobs before they read their input while it lives; see
/// [`StreamWriter::hold_table_reads`]. Dropping it releases them.
#[doc(hidden)]
pub struct ReadHold(Arc<CreditWindow>);

impl ReadHold {
    /// A live snapshot of the writer's [`StreamStats`], readable while `push_table` blocks.
    pub fn stats(&self) -> StreamStats {
        self.0.stats()
    }
}

impl Drop for ReadHold {
    fn drop(&mut self) {
        self.0.hold_reads(false);
    }
}

/// A bounded-memory, parallel-encode streaming writer over a [`WriteSession`].
///
/// Every job holds a credit of the writer's [`StreamWindow`] from [`Self::push`] until it is
/// committed in order, so the jobs and encoded bytes held stay within the bound in the
/// [module docs](self), whatever the producer's pace or the encode order.
pub struct StreamWriter {
    input: Option<Sender<(usize, Job)>>,
    workers: Vec<JoinHandle<()>>,
    committer: Option<JoinHandle<Result<WriteSession>>>,
    window: Arc<CreditWindow>,
    staging: BlockStaging,
    seq: usize,
}

impl StreamWriter {
    /// Build a pipeline over `session` with `workers` encode threads and a queue of `capacity` jobs
    /// waiting for a worker. Both are clamped to ≥ 1. The window is
    /// `StreamWindow::new(capacity + workers, DEFAULT_RAM_BUDGET)`: room for `capacity` queued jobs
    /// plus one encoding on every worker, so the pool is never starved by the window, and the
    /// default 1 GiB ([`crate::DEFAULT_RAM_BUDGET`]) of encoded bytes. Use [`Self::with_window`] to
    /// set the window directly.
    pub fn new(session: WriteSession, workers: usize, capacity: usize) -> Self {
        let workers = workers.max(1);
        let window = StreamWindow::new(
            capacity.max(1).saturating_add(workers),
            crate::config::DEFAULT_RAM_BUDGET,
        );
        Self::with_window(session, workers, window)
    }

    /// Build a pipeline over `session` with `workers` encode threads (clamped to ≥ 1) and the given
    /// credit `window`.
    pub fn with_window(session: WriteSession, workers: usize, window: StreamWindow) -> Self {
        let workers = workers.max(1);
        let window = Arc::new(CreditWindow::new(window, workers));
        // The channels only carry jobs; the window is what bounds them (at most `max_in_flight`
        // jobs sit in both together). Unbounded channels allocate nothing up front, so a window of
        // `usize::MAX` jobs (bound by bytes only) is fine.
        let (in_tx, in_rx) = channel::<(usize, Job)>();
        let in_rx = Arc::new(Mutex::new(in_rx));
        let (out_tx, out_rx) = channel::<Encoded>();
        let staging = session.staging();

        let worker_handles = (0..workers)
            .map(|_| {
                let rx = Arc::clone(&in_rx);
                let tx = out_tx.clone();
                let window = Arc::clone(&window);
                let staging = staging.clone();
                thread::spawn(move || {
                    let _close = CloseOnDrop {
                        window: Arc::clone(&window),
                        only_on_panic: true,
                    };
                    loop {
                        // Hold the lock only to dequeue, then encode without it (true parallelism).
                        let job = {
                            let guard = rx.lock().expect("input mutex");
                            guard.recv()
                        };
                        let Ok((seq, job)) = job else {
                            break; // input closed + drained
                        };
                        if !window.wait_encode_turn(seq) {
                            break; // pipeline closed
                        }
                        let res = job.run(&staging);
                        let held = res.as_ref().map_or(Held::default(), JobOutput::held);
                        window.charge(held);
                        if tx.send((seq, held, res)).is_err() {
                            break; // committer gone (it closed the window on its way out)
                        }
                    }
                })
            })
            .collect();
        drop(out_tx); // only workers hold senders now → out_rx closes when all workers exit

        let committer = {
            let window = Arc::clone(&window);
            thread::spawn(move || {
                let _close = CloseOnDrop {
                    window: Arc::clone(&window),
                    only_on_panic: false,
                };
                commit_loop(session, out_rx, &window)
            })
        };

        StreamWriter {
            input: Some(in_tx),
            workers: worker_handles,
            committer: Some(committer),
            window,
            staging,
            seq: 0,
        }
    }

    /// The encode-thread pool size this writer was built with — for observability (bench output,
    /// tracing). The committer and the producer thread run outside this count.
    pub fn worker_count(&self) -> usize {
        self.workers.len()
    }

    /// The credit window this writer enforces.
    pub fn window(&self) -> StreamWindow {
        self.window.limits
    }

    /// A live snapshot of the window's high-water marks and counters.
    pub fn stats(&self) -> StreamStats {
        self.window.stats()
    }

    /// Enqueue a block for encode+commit. Blocks (backpressure) while the [`StreamWindow`] is full:
    /// `max_in_flight` jobs pushed and not yet committed, or `max_in_flight_bytes` of encoded
    /// payloads held. Blocks are committed in call order, so push in the order you want the
    /// manifest to record them. Errors once the pipeline died (a failed encode or commit, or a
    /// panicking worker); [`Self::finish`] then reports the cause.
    pub fn push(&mut self, job: EncodeJob) -> Result<()> {
        self.submit(Job::InMemory(job), false)
    }

    /// Enqueue a [`StagedEncodeJob`]: like [`Self::push`] (same window, same commit order), but the
    /// job writes its data block to a staged file, which the committer streams into the product and
    /// deletes, so the block's bytes never count against the window's byte budget.
    pub fn push_staged(&mut self, job: StagedEncodeJob) -> Result<()> {
        self.submit(Job::Staged(job), false)
    }

    /// Write the table `name` of `spec.rows` rows from `groups` as the leaves of one logical table
    /// (ADR-0064 §5), each leaf one staged job under this writer's window, and return the number of
    /// leaves. The table is partitioned at [`BLOCK_ROWS`] ([`crate::table::block_count`]): a table
    /// of at most `BLOCK_ROWS` rows is the one block `name`, byte-identical to
    /// [`table_job_streaming`]; a larger one is the leaves `name/0000`, `name/0001`, … named by
    /// [`block_name`] and marked by [`block_partition`], byte for byte the leaves
    /// [`crate::TableMultiBlockSink`] writes for the same rows. Names and markers follow from
    /// `spec.rows` before the first row is read. Call [`Self::finish`] to commit and seal as usual.
    ///
    /// **Input.** `groups` is pulled on the calling thread, in order; each group holds at most
    /// [`ROWS_PER_GROUP`] rows with `spec`'s columns. Groups of exactly `ROWS_PER_GROUP` rows (the
    /// last may be shorter) are staged as they come; other sizes are re-chunked to that grid
    /// through a buffer of less than one row-group, so the leaves don't depend on how the input
    /// was chunked. `spec.partition` must be `None`: each leaf's marker is set here.
    ///
    /// **Staging and ordering.** Each block's row-groups are written, as they are pulled, to one
    /// file in the session's `staging/` directory, in the fragment format of
    /// [`crate::TableMultiBlockSink`]. A complete block is handed to a free encode worker (the call
    /// blocks while every worker has a job, or while the window is full), which reads the file back
    /// one row-group at a time and deletes it once read. The next block's file is created only after
    /// that hand-over, so at most `W + 1` blocks of input are on disk for `W` workers: the `W` the
    /// workers hold and the one being filled ([`StreamStats::peak_staged_input_blocks`],
    /// [`StreamStats::peak_staged_input_bytes`]). That is up to `(W + 1) · BLOCK_ROWS` rows of
    /// staged input, plus the encoded blocks staged until their commit, on the filesystem of the
    /// session directory. The last leaf is handed over once `groups` has ended, and the call
    /// returns then.
    ///
    /// **Memory.** With `W` encode workers and a table of `n` leaves, the heap is bounded by
    ///
    /// ```text
    /// peak heap  ≤  min(W, n) · p_block + s + c_leaf · n
    /// s          =  2 · ROWS_PER_GROUP · r_mem + 4 MiB
    /// c_leaf     =  2 · (e_leaf + b_leaf),    b_leaf = 1 KiB
    /// ```
    ///
    /// which, beyond the per-leaf term, doesn't grow with the table's row count (ADR-0064 §8, as
    /// amended by #760). `p_block` is the peak heap of a one-leaf write of the same schema in the
    /// same binary: measured, not a constant, since `available_parallelism` sets the encoder's
    /// queue depths, and it still holds the encoder's per-column terms, so it grows with the
    /// table's width. `r_mem` is the input's in-memory bytes per row (the column widths, plus one
    /// byte per nullable column). `s` is two row-groups of input (the one being staged and the one
    /// a worker has just read back) and 4 MiB of buffers and bookkeeping: 7 MiB for 24-byte rows.
    /// `c_leaf` is the manifest entry the session keeps for each committed leaf until the seal
    /// writes it: `e_leaf` is that entry's length in the manifest's canonical JSON (name, kind,
    /// digest and spec, so it grows with the column specs and their descriptions), `b_leaf` its
    /// bookkeeping, and the factor 2 an allowance for allocator retention around it. For a
    /// 12-column schema with a 3,180 B entry that is 8,408 B per leaf, 4.8 MB at 573 leaves. The
    /// term holds once #759 lands; until then a committed leaf's spec is held as a
    /// `serde_json::Value` tree, several times `e_leaf` (12.8 KB live for that schema).
    /// `tests/staged_job_memory.rs` checks the ceiling at `W ∈ {1, 2}` without the per-leaf term
    /// (at most 25.3 KB at its 8 leaves, which it can't resolve), that the reader keeps no more
    /// than `s` of input in RAM while every worker is held, and that a one-leaf write's peak is
    /// within `s` of the same encode into a sink, so a worker reads its staged block back a
    /// row-group at a time.
    ///
    /// The bound is on the heap, not on the process's memory. Staged input is on disk, and its
    /// pages are page cache: they count toward a cgroup's memory (`memory.current`) and are
    /// reclaimed only under pressure, once written back; on tmpfs they are RAM that can't be
    /// reclaimed at all, so the bound gains every byte in the session directory: the staged input
    /// above, the encoded blocks staged until their commit and the committed fragments (ADR-0026's
    /// disk paragraph). Keep the session directory on a disk-backed filesystem.
    ///
    /// # Errors
    ///
    /// [`Error::StreamedTable`] when the input runs past `spec.rows` (when it does), ends short
    /// of it (at its end) or yields a group of more than `ROWS_PER_GROUP` rows; the name, column and
    /// dtype errors of the sinks; and the pipeline's errors (a dead pipeline closes the window). A
    /// failed call never hands over the last leaf, so a partitioned table stays without its
    /// terminal leaf and [`Self::finish`] refuses to seal it ([`Error::LogicalTable`]); a single
    /// block is never handed over at all. Every staging file is deleted on every path: the one being
    /// filled when the call fails, the others by their workers or when their jobs are dropped.
    pub fn push_table<I>(
        &mut self,
        name: impl Into<String>,
        spec: TableSpec,
        groups: I,
    ) -> Result<u64>
    where
        I: IntoIterator<Item = TableData>,
    {
        self.push_table_with_block_rows(name, spec, groups, BLOCK_ROWS as u64)
    }

    /// [`Self::push_table`] partitioned at `block_rows` (a positive multiple of
    /// [`ROWS_PER_GROUP`]) instead of [`BLOCK_ROWS`]: a test seam, like
    /// [`crate::TableMultiBlockSink::with_block_rows`], for multi-block products at test sizes.
    /// Anything but `BLOCK_ROWS` writes a non-canonical partition.
    #[doc(hidden)]
    pub fn push_table_with_block_rows<I>(
        &mut self,
        name: impl Into<String>,
        spec: TableSpec,
        groups: I,
        block_rows: u64,
    ) -> Result<u64>
    where
        I: IntoIterator<Item = TableData>,
    {
        let name = name.into();
        if block_rows == 0 || !block_rows.is_multiple_of(ROWS_PER_GROUP as u64) {
            return Err(Error::Codec(format!(
                "push_table: block_rows ({block_rows}) must be a positive multiple of ROWS_PER_GROUP ({ROWS_PER_GROUP})"
            )));
        }
        if spec.partition.is_some() {
            return Err(Error::Invalid(format!(
                "push_table '{name}': spec.partition must be None, each leaf's marker is set by the writer"
            )));
        }
        for c in &spec.columns {
            ColumnData::validate_dtype(&c.dtype)?;
        }
        let blocks = partition_blocks(spec.rows, block_rows);
        // The last name is the longest: if it is valid, every name is.
        BlockName::new(block_name(&name, blocks - 1, blocks)?)?;
        let refuse = |reason| Error::StreamedTable {
            name: name.clone(),
            reason,
        };
        let mut fill = Fill::default();
        let mut seen = 0u64;
        // Rows short of a full row-group, when the input is not on the ROWS_PER_GROUP grid.
        let mut pending: Option<TableData> = None;
        for group in groups {
            let rows = group_rows(&spec, &group)?;
            if rows > ROWS_PER_GROUP {
                return Err(refuse(StreamedTableError::OversizedGroup {
                    rows: rows as u64,
                    max: ROWS_PER_GROUP as u64,
                }));
            }
            seen += rows as u64;
            if seen > spec.rows {
                return Err(refuse(StreamedTableError::TooManyRows {
                    declared: spec.rows,
                }));
            }
            let mut buf = match pending.take() {
                None if rows == ROWS_PER_GROUP => {
                    self.stage_group(&mut fill, &name, &spec, blocks, block_rows, &group)?;
                    continue;
                }
                None => group,
                Some(mut buf) => {
                    for ((_, into), (_, from)) in buf.iter_mut().zip(&group) {
                        into.extend(from)?;
                    }
                    buf
                }
            };
            while rows_of(&buf) >= ROWS_PER_GROUP {
                let (head, tail) = buf
                    .into_iter()
                    .map(|(n, c)| {
                        (
                            (n.clone(), c.slice(0, ROWS_PER_GROUP)),
                            (n, c.slice(ROWS_PER_GROUP, c.len())),
                        )
                    })
                    .unzip();
                self.stage_group(&mut fill, &name, &spec, blocks, block_rows, &head)?;
                buf = tail;
            }
            pending = (rows_of(&buf) > 0).then_some(buf);
        }
        if seen != spec.rows {
            return Err(refuse(StreamedTableError::TooFewRows {
                declared: spec.rows,
                actual: seen,
            }));
        }
        if let Some(rest) = pending {
            self.stage_group(&mut fill, &name, &spec, blocks, block_rows, &rest)?;
        }
        // The input has ended: the last leaf (the terminal one) is complete.
        self.dispatch(&mut fill, &name, &spec, blocks)?;
        Ok(blocks)
    }

    /// Append one canonical row-group to the block being filled (creating its staging file), and
    /// hand the block over once it is full, unless it is the last one.
    fn stage_group(
        &mut self,
        fill: &mut Fill,
        name: &str,
        spec: &TableSpec,
        blocks: u64,
        block_rows: u64,
        group: &TableData,
    ) -> Result<()> {
        if fill.input.is_none() {
            fill.input = Some(StagedInput::create(&self.staging, &self.window)?);
        }
        fill.input.as_mut().expect("created").append(group)?;
        fill.rows += rows_of(group) as u64;
        if fill.rows == block_rows && fill.block + 1 < blocks {
            self.dispatch(fill, name, spec, blocks)?;
        }
        Ok(())
    }

    /// Hand the filled block to a free worker as one staged leaf job.
    fn dispatch(
        &mut self,
        fill: &mut Fill,
        name: &str,
        spec: &TableSpec,
        blocks: u64,
    ) -> Result<()> {
        let leaf = TableSpec {
            rows: fill.rows,
            partition: block_partition(fill.block, blocks),
            ..spec.clone()
        };
        let bref = staged_table_ref(block_name(name, fill.block, blocks)?, &leaf)?;
        let input = match fill.input.take() {
            Some(input) => input,
            None => StagedInput::create(&self.staging, &self.window)?, // the empty table
        };
        self.submit(Job::Staged(input.into_job(bref, leaf)?), true)?;
        *fill = Fill {
            block: fill.block + 1,
            ..Fill::default()
        };
        Ok(())
    }

    /// Hold every [`Self::push_table`] job before it reads its staged input, until the returned
    /// [`ReadHold`] is dropped: a test seam (ADR-0064 §8's barrier test, #714's heap model) that
    /// pins the `W` dispatched blocks on disk so the staged-input bound can be observed.
    #[doc(hidden)]
    pub fn hold_table_reads(&self) -> ReadHold {
        self.window.hold_reads(true);
        ReadHold(Arc::clone(&self.window))
    }

    fn submit(&mut self, job: Job, free_worker: bool) -> Result<()> {
        let tx = self
            .input
            .as_ref()
            .ok_or_else(|| Error::Container("stream writer already finished".into()))?;
        self.window.acquire(free_worker)?;
        // The window admitted the job; the send never blocks, and fails only if every worker is gone.
        tx.send((self.seq, job)).map_err(|_| closed_error())?;
        self.seq += 1;
        Ok(())
    }

    /// Drain the pipeline, then compact + seal to `out`. Returns the sealed manifest (identical to a
    /// batch seal of the same blocks in push order).
    pub fn finish(self, out: &Path) -> Result<Manifest> {
        self.finish_with_stats(out).map(|(manifest, _)| manifest)
    }

    /// [`Self::finish`], also returning the window's final [`StreamStats`] — the high-water marks to
    /// assert the memory contract against.
    pub fn finish_with_stats(mut self, out: &Path) -> Result<(Manifest, StreamStats)> {
        drop(self.input.take()); // close input → workers drain + exit → out_rx closes → committer returns
                                 // Join EVERY thread before reporting anything (#680): a sibling worker may still be
                                 // encoding into a staged file, and the committer may still hold staged results; returning
                                 // early would let a caller reuse the session directory under them. A panic closes the
                                 // window, so the others exit promptly. The first error wins, workers before the committer.
        let mut first_error = None;
        for w in self.workers.drain(..) {
            if w.join().is_err() && first_error.is_none() {
                first_error = Some(Error::Container("encode worker panicked".into()));
            }
        }
        let committed = self
            .committer
            .take()
            .expect("committer present")
            .join()
            .map_err(|_| Error::Container("committer thread panicked".into()))
            .and_then(|r| r);
        if let Some(e) = first_error {
            return Err(e);
        }
        let session = committed?;
        let stats = self.window.stats();
        Ok((session.seal(out)?, stats))
    }
}

/// The single committer: receives encoded blocks, commits them to the session in `seq` order (a
/// reorder buffer holds completions that arrive early), releases each job's credit once it is
/// committed, and returns the session for sealing. The window bounds the reorder buffer to
/// `max_in_flight` entries.
fn commit_loop(
    mut session: WriteSession,
    out_rx: Receiver<Encoded>,
    window: &CreditWindow,
) -> Result<WriteSession> {
    let mut pending: BTreeMap<usize, (Held, JobOutput)> = BTreeMap::new();
    let mut next = 0usize;
    // Commit the data block then its additive companions (ADR-0028 §5 sidecars), in order, then
    // give the job's credit back. A staged data block streams from its file, which is deleted.
    let mut commit_ready = |session: &mut WriteSession,
                            pending: &mut BTreeMap<usize, (Held, JobOutput)>|
     -> Result<()> {
        while let Some((held, out)) = pending.remove(&next) {
            match out.data {
                DataBlock::InMemory((bref, payload)) => {
                    session.append_block(bref, &payload.bytes)?;
                }
                DataBlock::Staged((bref, staged)) => {
                    session.append_staged(bref, staged)?;
                }
            }
            for (cref, cpayload) in out.companions {
                session.append_block(cref, &cpayload.bytes)?;
            }
            window.release(held);
            next += 1;
        }
        Ok(())
    };
    while let Ok((seq, held, res)) = out_rx.recv() {
        pending.insert(seq, (held, res?));
        commit_ready(&mut session, &mut pending)?;
    }
    // Channel closed: all workers exited → every pushed block has arrived → drain the contiguous rest.
    commit_ready(&mut session, &mut pending)?;
    // Every pushed job must have committed. Workers exit only after the queue drained or on a
    // panic (which `finish` reports first), so a shortfall here is a pipeline bug: fail loudly
    // rather than seal a truncated product.
    let pushed = window.stats().jobs_pushed;
    if !pending.is_empty() || next != pushed {
        return Err(Error::Container(format!(
            "stream pipeline lost jobs: {next} of {pushed} committed, {} stranded",
            pending.len()
        )));
    }
    Ok(session)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::array::{self, ArrayData};
    use crate::Reader;
    use tessera_core::block::array::ArraySpec;
    use tessera_core::ProductBuilder;

    const TS: &str = "2024-01-01T00:00:00Z";

    fn ablock(name: &str, n: i16) -> (BlockRef, ArraySpec, ArrayData) {
        let mut spec = ArraySpec::new(vec![8, 8, 8], "int16");
        spec.codec = "pcodec".into();
        let data = ArrayData::I16((0..512).map(|k| (k as i16).wrapping_mul(3) + n).collect());
        let (r, _p) = array::array_block(name, &spec, &data).unwrap();
        (r, spec, data)
    }

    #[test]
    fn streamed_equals_batch_for_many_blocks() {
        let dir = tempfile::tempdir().unwrap();
        let n = 50;
        let blocks: Vec<_> = (0..n)
            .map(|i| ablock(&format!("b{i:03}"), i as i16))
            .collect();

        // streamed: small capacity + several workers → forces reorder + backpressure
        let ws = WriteSession::create(&dir.path().join("stage"), "recon", "p", "d", TS).unwrap();
        let mut sw = StreamWriter::new(ws, 4, 3);
        for (i, (_, spec, data)) in blocks.iter().enumerate() {
            sw.push(array_job(format!("b{i:03}"), spec.clone(), data.clone()))
                .unwrap();
        }
        let out = dir.path().join("stream.tsra");
        let sealed = sw.finish(&out).unwrap();

        // batch equivalent in the same (push) order
        let mut bb = ProductBuilder::new("recon", "p", "d", TS);
        for (r, _, _) in &blocks {
            bb.add_block_ref(r.clone());
        }
        let batch = bb.seal().unwrap();
        assert_eq!(sealed.id, batch.id);
        assert_eq!(sealed.content_hash, batch.content_hash);
        assert_eq!(
            sealed.manifest_hash, batch.manifest_hash,
            "streamed seal != batch"
        );

        let mut rdr = Reader::open(&out).unwrap();
        assert_eq!(rdr.block_names().len(), n);
        for nm in rdr.block_names() {
            rdr.read_block(&nm).unwrap();
        }
    }

    #[test]
    fn streamed_product_carries_session_metadata() {
        // metadata-first durability through the streaming path: fields set on the WriteSession (persisted
        // to the durable header) flow through StreamWriter to the sealed product — so a DAQ writer can set
        // modality/study up front and they survive the stream → seal.
        let dir = tempfile::tempdir().unwrap();
        let mut ws =
            WriteSession::create(&dir.path().join("stage"), "recon", "p", "d", TS).unwrap();
        ws.with_study("study-9").unwrap();
        ws.with_field(
            "modality",
            serde_json::json!({"_vocabulary": "DICOM", "_code": "PT"}),
        )
        .unwrap();
        let mut sw = StreamWriter::new(ws, 2, 2);
        let (_, spec, data) = ablock("b000", 0);
        sw.push(array_job("b000", spec, data)).unwrap();
        let sealed = sw.finish(&dir.path().join("s.tsra")).unwrap();
        assert_eq!(sealed.study.as_deref(), Some("study-9"));
        assert_eq!(
            sealed.metadata.get("modality"),
            Some(&serde_json::json!({"_vocabulary": "DICOM", "_code": "PT"}))
        );
    }

    #[test]
    fn indexed_jobs_emit_chunk_index_sidecars_matching_batch() {
        // ADR-0028 §5 — an indexed job emits each block's chunk-index sidecar during streaming, so the
        // sealed result must equal a batch that adds block then sidecar interleaved in push order.
        let dir = tempfile::tempdir().unwrap();
        let n = 6;
        let blocks: Vec<_> = (0..n)
            .map(|i| ablock(&format!("b{i:03}"), i as i16))
            .collect();

        let ws = WriteSession::create(&dir.path().join("stage"), "recon", "p", "d", TS).unwrap();
        let mut sw = StreamWriter::new(ws, 4, 3); // small capacity → reorder/backpressure
        for (i, (_, spec, data)) in blocks.iter().enumerate() {
            sw.push(array_job_indexed(
                format!("b{i:03}"),
                spec.clone(),
                data.clone(),
            ))
            .unwrap();
        }
        let out = dir.path().join("stream.tsra");
        let sealed = sw.finish(&out).unwrap();

        // batch equivalent: data block then its sidecar, interleaved in the same push order.
        let mut bb = ProductBuilder::new("recon", "p", "d", TS);
        for (i, (r, spec, data)) in blocks.iter().enumerate() {
            bb.add_block_ref(r.clone());
            let (_, sidecar) =
                array::array_block_with_index(&format!("b{i:03}"), spec, data).unwrap();
            bb.add_block_ref(sidecar.expect("integer array → sidecar").0);
        }
        let batch = bb.seal().unwrap();
        assert_eq!(
            sealed.content_hash, batch.content_hash,
            "indexed stream != batch + interleaved sidecars"
        );
        assert_eq!(sealed.manifest_hash, batch.manifest_hash);

        // both the data blocks and their `.cidx` sidecars are present + readable.
        let rdr = Reader::open(&out).unwrap();
        let names = rdr.block_names();
        assert_eq!(names.len(), n * 2);
        assert_eq!(names.iter().filter(|nm| nm.ends_with(".cidx")).count(), n);
    }

    // ---- #677: the credit window -------------------------------------------------------------

    use std::sync::atomic::{AtomicUsize, Ordering};

    /// The `n` distinct array blocks the window tests stream.
    fn blocks(n: usize) -> Vec<(BlockRef, ArraySpec, ArrayData)> {
        (0..n)
            .map(|i| ablock(&format!("b{i:03}"), i as i16))
            .collect()
    }

    /// A job that encodes block `i` only after `gate` fires — the barrier that holds job 0.
    fn held_job(i: usize, spec: ArraySpec, data: ArrayData, gate: Receiver<()>) -> EncodeJob {
        Box::new(move || {
            gate.recv().expect("gate sender dropped");
            array_job(format!("b{i:03}"), spec, data)()
        })
    }

    /// A job that encodes block `i` and then reports `i` on `done`.
    fn reporting_job(i: usize, spec: ArraySpec, data: ArrayData, done: Sender<usize>) -> EncodeJob {
        Box::new(move || {
            let out = array_job(format!("b{i:03}"), spec, data)();
            done.send(i).expect("done receiver dropped");
            out
        })
    }

    /// Spin until `cond` holds on the window's live state. Timing-free: it waits for a state, never
    /// for a duration, and every caller waits only for a state the window must reach.
    fn spin_until(window: &CreditWindow, cond: impl Fn(&WindowState) -> bool) {
        while !cond(&window.lock()) {
            thread::yield_now();
        }
    }

    /// The unthrottled reference: every block streamed through a wide window with plenty of workers.
    fn unthrottled(dir: &Path, blocks: &[(BlockRef, ArraySpec, ArrayData)]) -> (Manifest, Vec<u8>) {
        let ws = WriteSession::create(&dir.join("ref_stage"), "recon", "p", "d", TS).unwrap();
        let mut sw = StreamWriter::new(ws, 4, blocks.len());
        for (i, (_, spec, data)) in blocks.iter().enumerate() {
            sw.push(array_job(format!("b{i:03}"), spec.clone(), data.clone()))
                .unwrap();
        }
        let out = dir.join("ref.tsra");
        let m = sw.finish(&out).unwrap();
        (m, std::fs::read(&out).unwrap())
    }

    #[test]
    fn held_first_job_cannot_grow_in_flight_past_the_window() {
        // #677 test (b): job 0 is held at a barrier while later jobs finish. Before the window, the
        // committer drained every later completion into its reorder buffer and the producer never
        // blocked, so all N-1 payloads piled up behind job 0. Now a job holds its credit until its
        // in-order commit: exactly `J` jobs are admitted, the producer blocks on job `J`, and nothing
        // past the window is ever encoded while job 0 is held.
        const J: usize = 4;
        const N: usize = 32;
        let dir = tempfile::tempdir().unwrap();
        let blocks = blocks(N);
        let ws = WriteSession::create(&dir.path().join("stage"), "recon", "p", "d", TS).unwrap();
        // Byte budget out of reach: only the job credits bind here.
        let sw = StreamWriter::with_window(ws, 2, StreamWindow::new(J, u64::MAX));
        let window = Arc::clone(&sw.window);

        let (gate_tx, gate_rx) = channel();
        let (done_tx, done_rx) = channel();
        let pushed = Arc::new(AtomicUsize::new(0));
        let producer = {
            let pushed = Arc::clone(&pushed);
            let blocks = blocks.clone();
            let out = dir.path().join("held.tsra");
            thread::spawn(move || {
                let mut sw = sw;
                let mut gate = Some(gate_rx);
                for (i, (_, spec, data)) in blocks.into_iter().enumerate() {
                    let job = match gate.take() {
                        Some(g) => held_job(i, spec, data, g),
                        None => reporting_job(i, spec, data, done_tx.clone()),
                    };
                    sw.push(job).unwrap();
                    pushed.fetch_add(1, Ordering::SeqCst);
                }
                let res = sw.finish_with_stats(&out);
                (res, out)
            })
        };

        // The free worker finishes every admitted job behind job 0: jobs 1..J.
        let mut done: Vec<usize> = (1..J).map(|_| done_rx.recv().unwrap()).collect();
        done.sort_unstable();
        assert_eq!(done, (1..J).collect::<Vec<_>>());
        // … and the producer then blocks on job J: the window is full and nothing commits.
        spin_until(&window, |st| st.stats.producer_stalls == 1);
        {
            let st = window.lock();
            assert_eq!(st.in_flight_jobs, J, "the held window is exactly full");
            assert_eq!(st.stats.jobs_pushed, J);
            assert_eq!(st.stats.jobs_committed, 0, "nothing commits behind job 0");
            assert_eq!(st.stats.peak_in_flight_jobs, J);
        }
        assert_eq!(pushed.load(Ordering::SeqCst), J, "push J blocks");
        assert!(
            done_rx.try_recv().is_err(),
            "no job past the window is encoded while job 0 is held"
        );

        // Release job 0: everything drains in order and seals to the unthrottled bytes.
        gate_tx.send(()).unwrap();
        let ((sealed, stats), out) = {
            let (res, out) = producer.join().unwrap();
            (res.unwrap(), out)
        };
        assert_eq!(stats.jobs_pushed, N);
        assert_eq!(stats.jobs_committed, N);
        assert!(stats.peak_in_flight_jobs <= J, "{stats:?}");
        assert!(stats.producer_stalls >= 1);
        let (reference, ref_bytes) = unthrottled(dir.path(), &blocks);
        assert_eq!(sealed.content_hash, reference.content_hash);
        assert_eq!(sealed.manifest_hash, reference.manifest_hash);
        assert_eq!(
            std::fs::read(&out).unwrap(),
            ref_bytes,
            "sealed bytes differ"
        );
        let names = Reader::open(&out).unwrap().block_names();
        let expected: Vec<String> = (0..N).map(|i| format!("b{i:03}")).collect();
        assert_eq!(names, expected, "commit order is push order");
    }

    #[test]
    fn byte_budget_bounds_encoded_bytes_while_the_first_job_is_held() {
        // The byte credit: with job 0 held and a job window wider than the stream, only the byte
        // budget can stop the producer — and it must, and the encoded bytes held must stay within
        // `B + min(W, J) × P`, not grow to (N-1) × P.
        const N: usize = 24;
        const W: usize = 4;
        let dir = tempfile::tempdir().unwrap();
        let blocks = blocks(N);
        let p_est = array::array_block("x", &blocks[1].1, &blocks[1].2)
            .unwrap()
            .1
            .bytes
            .len() as u64;
        let budget = 2 * p_est; // about two payloads
        let window_limits = StreamWindow::new(N + 8, budget);
        let ws = WriteSession::create(&dir.path().join("stage"), "recon", "p", "d", TS).unwrap();
        let sw = StreamWriter::with_window(ws, W, window_limits);
        let window = Arc::clone(&sw.window);

        let (gate_tx, gate_rx) = channel();
        let producer = {
            let blocks = blocks.clone();
            let out = dir.path().join("bytes.tsra");
            let window = Arc::clone(&window);
            thread::spawn(move || {
                let mut sw = sw;
                let mut gate = Some(gate_rx);
                for (i, (_, spec, data)) in blocks.into_iter().enumerate() {
                    let job = match gate.take() {
                        Some(g) => held_job(i, spec, data, g),
                        None => array_job(format!("b{i:03}"), spec, data),
                    };
                    let before = window.lock().in_flight_bytes;
                    sw.push(job).unwrap();
                    // Until the first stall, pace the producer on the encodes: wait for job i's
                    // payload to be charged, so each push sees every earlier payload. (Nothing
                    // commits while job 0 is held, so the held bytes only grow.)
                    if i > 0 && window.lock().stats.producer_stalls == 0 {
                        spin_until(&window, |st| st.in_flight_bytes > before);
                    }
                }
                (sw.finish_with_stats(&out), out)
            })
        };

        // The job window can't fill (N < J), so a stall here is the byte budget blocking the producer.
        spin_until(&window, |st| st.stats.producer_stalls == 1);
        {
            let st = window.lock();
            assert!(
                st.in_flight_bytes >= budget,
                "stalled below the byte budget"
            );
            assert_eq!(st.stats.jobs_committed, 0);
            assert!(
                st.stats.jobs_pushed < N,
                "the producer was stopped mid-stream"
            );
        }
        gate_tx.send(()).unwrap();
        let (res, out) = producer.join().unwrap();
        let (sealed, stats) = res.unwrap();
        let p = stats.max_job_bytes;
        assert!(p > 0);
        let in_progress = W.min(window_limits.max_in_flight) as u64;
        assert!(
            stats.peak_in_flight_bytes < budget + in_progress * p,
            "peak {} ≥ B {budget} + {in_progress} × P {p}",
            stats.peak_in_flight_bytes
        );
        assert!(stats.peak_in_flight_bytes < (N as u64 - 1) * p);
        assert_eq!(stats.jobs_committed, N);
        let (reference, ref_bytes) = unthrottled(dir.path(), &blocks);
        assert_eq!(sealed.content_hash, reference.content_hash);
        assert_eq!(std::fs::read(&out).unwrap(), ref_bytes);
    }

    #[test]
    fn encode_gate_stops_new_encodes_once_the_byte_budget_is_held() {
        // Gate 2 on its own: jobs 2..5 are admitted while no bytes are held (so gate 1 lets them
        // in), then job 1 finishes and holds the 1-byte budget while job 0 is still held. The free
        // worker must now wait at the encode gate instead of encoding 2..5 behind job 0 — without
        // gate 2 it would hold ~5 payloads; with it the peak stays `< B + min(W, J) × P = 1 + 2P`.
        const N: usize = 6;
        let dir = tempfile::tempdir().unwrap();
        let blocks = blocks(N);
        let ws = WriteSession::create(&dir.path().join("stage"), "recon", "p", "d", TS).unwrap();
        let mut sw = StreamWriter::with_window(ws, 2, StreamWindow::new(N, 1));
        let window = Arc::clone(&sw.window);
        let (gate0_tx, gate0_rx) = channel();
        let (gate1_tx, gate1_rx) = channel();
        let (done_tx, done_rx) = channel();
        let mut gates = vec![gate0_rx, gate1_rx].into_iter();
        for (i, (_, spec, data)) in blocks.iter().cloned().enumerate() {
            let job = match gates.next() {
                Some(g) => held_job(i, spec, data, g),
                None => reporting_job(i, spec, data, done_tx.clone()),
            };
            // Nothing has finished encoding yet (jobs 0 and 1 are held, and occupy both
            // workers), so every push is admitted without waiting.
            sw.push(job).unwrap();
        }
        assert_eq!(window.lock().stats.producer_stalls, 0);
        gate1_tx.send(()).unwrap();
        // Job 1's worker comes back for job 2 and must wait at the gate (with no gate it would
        // encode 2..5 and report all four).
        let done = std::cell::Cell::new(0);
        spin_until(&window, |st| {
            while done_rx.try_recv().is_ok() {
                done.set(done.get() + 1);
            }
            st.gated_encodes == 1 || done.get() == N - 2
        });
        assert_eq!(done.get(), 0, "jobs encoded past a held byte budget");
        assert_eq!(window.lock().gated_encodes, 1);
        gate0_tx.send(()).unwrap();
        let (_, stats) = sw.finish_with_stats(&dir.path().join("gate.tsra")).unwrap();
        assert_eq!(stats.jobs_committed, N);
        assert!(
            stats.peak_in_flight_bytes < 1 + 2 * stats.max_job_bytes,
            "{stats:?}"
        );
    }

    #[test]
    fn job_larger_than_the_byte_budget_completes_alone() {
        // Deadlock rule: a job whose payload alone exceeds the byte budget is still admitted into an
        // empty window and always runs once it is the job the committer waits on — so a 1-byte budget
        // degrades to one encoded job at a time, never to a hang. The peak is reported.
        let dir = tempfile::tempdir().unwrap();
        let blocks = blocks(8);
        for workers in [1, 4] {
            let stage = dir.path().join(format!("stage{workers}"));
            let ws = WriteSession::create(&stage, "recon", "p", "d", TS).unwrap();
            let mut sw = StreamWriter::with_window(ws, workers, StreamWindow::new(4, 1));
            for (i, (_, spec, data)) in blocks.iter().enumerate() {
                sw.push(array_job_indexed(
                    format!("b{i:03}"),
                    spec.clone(),
                    data.clone(),
                ))
                .unwrap();
            }
            let out = dir.path().join(format!("big{workers}.tsra"));
            let (_, stats) = sw.finish_with_stats(&out).unwrap();
            assert_eq!(stats.jobs_committed, 8);
            assert!(stats.max_job_bytes > 1, "every payload exceeds the budget");
            assert!(stats.peak_in_flight_bytes >= stats.max_job_bytes);
            // B = 1: once one payload is held no other encode may start, so at most the encodes
            // already in progress add theirs: `< B + min(W, J) × P`.
            let in_progress = workers.min(4) as u64;
            assert!(stats.peak_in_flight_bytes < 1 + in_progress * stats.max_job_bytes);
        }
    }

    #[test]
    fn content_hash_is_identical_across_workers_and_windows() {
        // #677 test (c): the window throttles, it never reorders — bytes and content_hash are the same
        // for every worker count and window size, including windows smaller than the pool.
        let dir = tempfile::tempdir().unwrap();
        let blocks = blocks(20);
        let mut runs: Vec<(String, Manifest, Vec<u8>)> = Vec::new();
        for workers in [1usize, 4, 16] {
            let windows = [
                ("1", Some(StreamWindow::new(1, u64::MAX))),
                ("2", Some(StreamWindow::new(2, u64::MAX))),
                ("2/1B", Some(StreamWindow::new(2, 1))),
                ("default", None),
            ];
            for (label, window) in windows {
                let tag = format!("w{workers}-{}", label.replace('/', "_"));
                let ws =
                    WriteSession::create(&dir.path().join(&tag), "recon", "p", "d", TS).unwrap();
                let mut sw = match window {
                    Some(w) => StreamWriter::with_window(ws, workers, w),
                    None => StreamWriter::new(ws, workers, 3),
                };
                for (i, (_, spec, data)) in blocks.iter().enumerate() {
                    sw.push(array_job_indexed(
                        format!("b{i:03}"),
                        spec.clone(),
                        data.clone(),
                    ))
                    .unwrap();
                }
                let out = dir.path().join(format!("{tag}.tsra"));
                let (m, stats) = sw.finish_with_stats(&out).unwrap();
                assert!(stats.peak_in_flight_jobs <= sw_window(window, workers).max_in_flight);
                runs.push((tag, m, std::fs::read(&out).unwrap()));
            }
        }
        let (first_tag, first, first_bytes) = &runs[0];
        for (tag, m, bytes) in &runs[1..] {
            assert_eq!(m.content_hash, first.content_hash, "{tag} vs {first_tag}");
            assert_eq!(m.manifest_hash, first.manifest_hash, "{tag} vs {first_tag}");
            assert_eq!(
                bytes, first_bytes,
                "{tag} vs {first_tag}: sealed bytes differ"
            );
        }
    }

    /// The window a test run used: the explicit one, or [`StreamWriter::new`]'s default for a
    /// capacity-3 queue.
    fn sw_window(window: Option<StreamWindow>, workers: usize) -> StreamWindow {
        window.unwrap_or(StreamWindow::new(3 + workers, crate::DEFAULT_RAM_BUDGET))
    }

    #[test]
    fn new_keeps_one_job_per_worker_beyond_the_queue() {
        // The default window: the queue capacity plus one encoding job per worker, so the window
        // never starves the pool (today's throughput), and the default RAM budget in bytes.
        let dir = tempfile::tempdir().unwrap();
        let ws = WriteSession::create(&dir.path().join("stage"), "recon", "p", "d", TS).unwrap();
        let sw = StreamWriter::new(ws, 4, 2);
        assert_eq!(sw.window(), StreamWindow::new(6, crate::DEFAULT_RAM_BUDGET));
        assert_eq!(
            StreamWindow::new(0, 0),
            StreamWindow::new(1, 1),
            "clamped to ≥ 1"
        );
        sw.finish(&dir.path().join("s.tsra")).unwrap();
    }

    /// A listmode-shaped table for the staged (#677 gap 1) tests: a sequential row index, a NULLABLE
    /// timestamp whose first 1024 rows are NULL (the consumer's decoder shape), and an
    /// incompressible 64-bit column so the encoded block is ~8 bytes a row, far above the budgets.
    mod listmode {
        use crate::table::{ColumnData, TableData, ROWS_PER_GROUP};
        use tessera_core::block::table::{Column, TableSpec};

        fn col(name: &str, dtype: &str) -> Column {
            Column {
                name: name.into(),
                dtype: dtype.into(),
                ..Default::default()
            }
        }

        pub(super) fn spec(rows: u64) -> TableSpec {
            TableSpec {
                columns: vec![
                    col("t", "u8"),
                    col("ms", "u4").nullable(),
                    col("r", "u8"),
                    col("e", "u2"),
                ],
                rows,
                row_index: Some("t".into()),
                partition: None,
            }
        }

        /// SplitMix64 of the absolute row, so values don't depend on how the rows are chunked.
        fn mix(row: u64) -> u64 {
            let mut x = row.wrapping_add(0x9e37_79b9_7f4a_7c15);
            x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            x ^ (x >> 31)
        }

        /// Rows `start..start + n`.
        pub(super) fn rows(start: u64, n: usize) -> TableData {
            let rows = start..start + n as u64;
            vec![
                ("t".into(), ColumnData::U64(rows.clone().collect())),
                (
                    "ms".into(),
                    ColumnData::Nullable {
                        values: Box::new(ColumnData::U32(
                            rows.clone().map(|i| (i / 1024) as u32).collect(),
                        )),
                        validity: rows.clone().map(|i| i >= 1024).collect(),
                    },
                ),
                ("r".into(), ColumnData::U64(rows.clone().map(mix).collect())),
                (
                    "e".into(),
                    ColumnData::U16(rows.map(|i| (mix(i ^ 0x55) & 0x1ff) as u16).collect()),
                ),
            ]
        }

        /// The table as a lazy iterator of canonical row-groups: nothing is held but the group the
        /// encoder is consuming.
        pub(super) fn groups(total: u64) -> impl Iterator<Item = TableData> + Send + 'static {
            (0..total)
                .step_by(ROWS_PER_GROUP)
                .map(move |start| rows(start, (total - start).min(ROWS_PER_GROUP as u64) as usize))
        }

        /// The whole table in RAM (the batch reference).
        pub(super) fn all(total: u64) -> TableData {
            rows(0, total as usize)
        }
    }

    /// Stage the listmode table's row-groups as fragments, as [`crate::TableMultiBlockSink`] does.
    fn stage_fragments(dir: &Path, total: u64) -> Vec<std::path::PathBuf> {
        std::fs::create_dir_all(dir).unwrap();
        listmode::groups(total)
            .enumerate()
            .map(|(i, group)| {
                let path = dir.join(format!("g{i:08}.frag"));
                crate::accumulate::write_fragment(&path, &group).unwrap();
                path
            })
            .collect()
    }

    /// The staged files left in a session's staging directory.
    fn staged_files(stage: &Path) -> Vec<std::path::PathBuf> {
        std::fs::read_dir(stage.join("staging"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect()
    }

    /// #677 test (a): ONE named table block many times the window's byte budget streams through the
    /// `StreamWriter` with its encoded bytes on disk, not in the window. The RAM high-water stays
    /// within the documented bound `B + min(W, J) × P` with `P = 0` (a staged table job holds no
    /// payload in RAM), the staged high-water is the block itself, and the sealed block is
    /// byte-identical to the batch encode.
    ///
    /// Before the fix (`table_job_from_fragments` returned the encoded block as a `Vec`, pushed
    /// with `push`), the same run held the whole block in the window:
    /// `peak_in_flight_bytes` = the block's size, ~150× the budget.
    ///
    /// These checks are the window's accounting. The real heap, which would also catch a job that
    /// buffered its encode before writing the staged file, is checked by
    /// `tests/staged_job_memory.rs` (its own binary, for its counting allocator).
    #[test]
    fn single_table_block_many_times_the_budget_stays_within_the_window() {
        const BUDGET: u64 = 64 * 1024;
        let total = 16 * crate::table::ROWS_PER_GROUP as u64 + 4321; // 17 row-groups
        let spec = listmode::spec(total);
        let batch = crate::table::encode(&spec, &listmode::all(total)).unwrap();
        assert!(
            batch.len() as u64 > 100 * BUDGET,
            "the block must dwarf the budget: {} bytes",
            batch.len()
        );

        let dir = tempfile::tempdir().unwrap();
        let frags = stage_fragments(&dir.path().join("frags"), total);
        for workers in [1usize, 4] {
            let stage = dir.path().join(format!("stage-{workers}"));
            let ws = WriteSession::create(&stage, "recon", "p", "d", TS).unwrap();
            let window = StreamWindow::new(4, BUDGET);
            let mut sw = StreamWriter::with_window(ws, workers, window);
            sw.push_staged(table_job_from_fragments(
                "events",
                spec.clone(),
                frags.clone(),
            ))
            .unwrap();
            let out = dir.path().join(format!("w{workers}.tsra"));
            let (manifest, stats) = sw.finish_with_stats(&out).unwrap();

            // The RAM bound, as documented: peak < B + min(W, J) × P, where P (the RAM payload of
            // the one job) is zero for a staged table job — so the peak is below the budget.
            assert!(
                stats.peak_in_flight_bytes < BUDGET,
                "w{workers}: {} encoded bytes held in RAM, budget {BUDGET}",
                stats.peak_in_flight_bytes
            );
            assert_eq!(
                stats.max_job_bytes, 0,
                "a staged table job holds no payload in RAM"
            );
            // The disk side: the staged high-water is this one block (≤ J × S).
            assert_eq!(stats.max_staged_job_bytes, batch.len() as u64);
            assert!(
                stats.peak_staged_bytes <= window.max_in_flight as u64 * stats.max_staged_job_bytes
            );
            assert_eq!((stats.jobs_pushed, stats.jobs_committed), (1, 1));

            // One canonical block, byte-identical to the batch encode, and no staged file left.
            assert_eq!(manifest.blocks.len(), 1);
            assert_eq!(manifest.blocks[0].name, "events");
            assert_eq!(
                manifest.blocks[0].digest.as_deref(),
                Some(tessera_core::hash::digest(&batch).as_str())
            );
            let mut rdr = Reader::open(&out).unwrap();
            assert!(
                rdr.read_block("events").unwrap() == batch,
                "w{workers}: block != batch encode"
            );
            assert!(
                staged_files(&stage).is_empty(),
                "w{workers}: staged file left behind"
            );
        }
    }

    /// A staged table job seals the same product as the in-memory `table_job` of the same rows —
    /// bytes, `content_hash` and `manifest_hash` — for every worker count and window, so which
    /// kind of job wrote a block is invisible in the product. Mixed with in-memory jobs, the commit
    /// order is still push order.
    #[test]
    #[allow(deprecated)] // pins the deprecated job's remaining (≤ BLOCK_ROWS) contract until it is removed
    fn staged_table_jobs_seal_the_in_memory_product_for_every_worker_count_and_window() {
        let total = 3 * crate::table::ROWS_PER_GROUP as u64 + 99;
        let spec = listmode::spec(total);
        let arrays = blocks(3);
        let dir = tempfile::tempdir().unwrap();
        let run = |tag: &str, workers: usize, window: StreamWindow, staged: bool| {
            let stage = dir.path().join(tag);
            let ws = WriteSession::create(&stage, "recon", "p", "d", TS).unwrap();
            let mut sw = StreamWriter::with_window(ws, workers, window);
            for (i, (_, aspec, adata)) in arrays.iter().enumerate() {
                sw.push(array_job(format!("a{i}"), aspec.clone(), adata.clone()))
                    .unwrap();
                let name = format!("events{i}");
                if staged {
                    sw.push_staged(table_job_streaming(
                        name,
                        spec.clone(),
                        listmode::groups(total),
                    ))
                    .unwrap();
                } else {
                    sw.push(table_job(name, spec.clone(), listmode::all(total)))
                        .unwrap();
                }
            }
            let out = dir.path().join(format!("{tag}.tsra"));
            let m = sw.finish(&out).unwrap();
            assert!(
                staged_files(&stage).is_empty(),
                "{tag}: staged file left behind"
            );
            (m, std::fs::read(&out).unwrap())
        };
        let (reference, reference_bytes) = run(
            "in-memory",
            1,
            StreamWindow::new(usize::MAX, u64::MAX),
            false,
        );
        for workers in [1usize, 4] {
            for (label, window) in [
                ("1", StreamWindow::new(1, u64::MAX)),
                ("2-1B", StreamWindow::new(2, 1)),
                ("wide", StreamWindow::new(64, u64::MAX)),
            ] {
                let tag = format!("staged-w{workers}-{label}");
                let (m, bytes) = run(&tag, workers, window, true);
                assert_eq!(m.content_hash, reference.content_hash, "{tag}");
                assert_eq!(m.manifest_hash, reference.manifest_hash, "{tag}");
                assert!(bytes == reference_bytes, "{tag}: sealed bytes differ");
            }
        }
    }

    /// A staged job that fails mid-encode (a fragment vanished) fails `finish` with that error and
    /// leaves no staged file and no committed block behind.
    #[test]
    fn failed_staged_job_leaves_no_staged_file_and_no_block() {
        let total = 3 * crate::table::ROWS_PER_GROUP as u64;
        let dir = tempfile::tempdir().unwrap();
        let mut frags = stage_fragments(&dir.path().join("frags"), total);
        frags.insert(2, dir.path().join("frags").join("missing.frag"));
        let stage = dir.path().join("stage");
        let ws = WriteSession::create(&stage, "recon", "p", "d", TS).unwrap();
        let mut sw = StreamWriter::with_window(ws, 2, StreamWindow::new(4, 1024));
        sw.push_staged(table_job_from_fragments(
            "events",
            listmode::spec(total),
            frags,
        ))
        .unwrap();
        let err = sw.finish(&dir.path().join("s.tsra")).unwrap_err();
        assert!(
            err.to_string().to_lowercase().contains("no such file"),
            "{err}"
        );
        assert!(staged_files(&stage).is_empty(), "staged file left behind");
        assert_eq!(std::fs::read_dir(stage.join("blocks")).unwrap().count(), 0);
        assert!(std::fs::read_to_string(stage.join("journal.jsonl"))
            .unwrap()
            .is_empty());
    }

    /// A staged job that writes part of its block and then panics: `finish` reports the panic only
    /// after every thread is joined, so by then no staged file is left — not the panicking job's,
    /// not a sibling's that was still encoding (#680).
    #[test]
    fn panicking_staged_job_leaves_no_staged_file_once_finish_returns() {
        let total = 2 * crate::table::ROWS_PER_GROUP as u64;
        let dir = tempfile::tempdir().unwrap();
        let stage = dir.path().join("stage");
        let ws = WriteSession::create(&stage, "recon", "p", "d", TS).unwrap();
        let mut sw = StreamWriter::with_window(ws, 4, StreamWindow::new(8, u64::MAX));
        sw.push_staged(Box::new(|staging: &BlockStaging| {
            let mut staged = staging.create().unwrap();
            std::io::Write::write_all(&mut staged, b"half a block").unwrap();
            panic!("encoder bug mid-block");
        }))
        .unwrap();
        for i in 0..3 {
            // Pushes after the panic closed the window fail; that is fine, the jobs already in the
            // queue are what matters.
            let _ = sw.push_table(
                format!("events{i}"),
                listmode::spec(total),
                listmode::groups(total),
            );
        }
        let err = sw.finish(&dir.path().join("s.tsra")).unwrap_err();
        assert!(err.to_string().contains("worker panicked"), "{err}");
        assert!(staged_files(&stage).is_empty(), "staged file left behind");
    }

    /// A commit that fails (here: a staged block whose declared digest is wrong) while later staged
    /// results wait in the reorder buffer: `finish` reports the integrity error, and neither the
    /// rejected block nor the waiting ones leave a staged file or a fragment behind.
    #[test]
    fn failed_staged_commit_drops_every_waiting_staged_file() {
        let total = 2 * crate::table::ROWS_PER_GROUP as u64;
        let dir = tempfile::tempdir().unwrap();
        let stage = dir.path().join("stage");
        let ws = WriteSession::create(&stage, "recon", "p", "d", TS).unwrap();
        let mut sw = StreamWriter::with_window(ws, 4, StreamWindow::new(8, u64::MAX));
        let (gate_tx, gate_rx) = std::sync::mpsc::channel::<()>();
        sw.push_staged(Box::new(move |staging: &BlockStaging| {
            // Held until the later jobs are encoded, so they wait in the reorder buffer.
            gate_rx.recv().unwrap();
            let mut staged = staging.create()?;
            std::io::Write::write_all(&mut staged, b"payload")?;
            let mut bref = staged_table_ref("bad".into(), &listmode::spec(0))?;
            bref.digest = Some(tessera_core::hash::digest(b"something else"));
            Ok(((bref, staged), Vec::new()))
        }))
        .unwrap();
        for i in 0..3 {
            sw.push_table(
                format!("events{i}"),
                listmode::spec(total),
                listmode::groups(total),
            )
            .unwrap();
        }
        // Timing-free: release job 0 only once the three later jobs are encoded and charged.
        let block = crate::table::encode(&listmode::spec(total), &listmode::all(total))
            .unwrap()
            .len() as u64;
        spin_until(&sw.window, |st| st.staged_bytes == 3 * block);
        gate_tx.send(()).unwrap();
        let err = sw.finish(&dir.path().join("s.tsra")).unwrap_err();
        assert!(
            matches!(
                err,
                Error::Integrity {
                    what: "block_payload",
                    ..
                }
            ),
            "{err}"
        );
        assert!(staged_files(&stage).is_empty(), "staged file left behind");
        assert_eq!(std::fs::read_dir(stage.join("blocks")).unwrap().count(), 0);
    }

    #[test]
    fn failed_encode_closes_the_window_instead_of_hanging_the_producer() {
        // A job whose encode fails never commits, so its credit never returns. The committer's exit
        // closes the window: the producer blocked on a full window gets an error, and finish
        // reports the encode failure.
        let dir = tempfile::tempdir().unwrap();
        let ws = WriteSession::create(&dir.path().join("stage"), "recon", "p", "d", TS).unwrap();
        let mut sw = StreamWriter::with_window(ws, 2, StreamWindow::new(1, u64::MAX));
        sw.push(Box::new(|| Err(Error::Codec("boom".into()))))
            .unwrap();
        let (_, spec, data) = ablock("b001", 1);
        let err = sw.push(array_job("b001", spec, data)).unwrap_err();
        assert!(err.to_string().contains("pipeline closed"), "{err}");
        let err = sw.finish(&dir.path().join("s.tsra")).unwrap_err();
        assert!(err.to_string().contains("boom"), "{err}");
    }

    #[test]
    fn panicking_worker_closes_the_window_instead_of_hanging_the_producer() {
        let dir = tempfile::tempdir().unwrap();
        let ws = WriteSession::create(&dir.path().join("stage"), "recon", "p", "d", TS).unwrap();
        let mut sw = StreamWriter::with_window(ws, 2, StreamWindow::new(1, u64::MAX));
        sw.push(Box::new(|| panic!("encoder bug"))).unwrap();
        let (_, spec, data) = ablock("b001", 1);
        let err = sw.push(array_job("b001", spec, data)).unwrap_err();
        assert!(err.to_string().contains("pipeline closed"), "{err}");
        let err = sw.finish(&dir.path().join("s.tsra")).unwrap_err();
        assert!(err.to_string().contains("worker panicked"), "{err}");
    }

    // ---- ADR-0064 §5: push_table -------------------------------------------------------------

    const RPG: u64 = ROWS_PER_GROUP as u64;

    /// The listmode table in groups of `size` rows (≤ ROWS_PER_GROUP), off the canonical grid
    /// unless `size` is ROWS_PER_GROUP.
    fn groups_of(total: u64, size: u64) -> impl Iterator<Item = TableData> {
        (0..total)
            .step_by(size as usize)
            .map(move |start| listmode::rows(start, (total - start).min(size) as usize))
    }

    /// A manifest without what ADR-0064 §6 lets differ between two writers of the same rows:
    /// `producer`, `timestamp` and what follows from them.
    fn canonical(m: &Manifest) -> serde_json::Value {
        let mut v = serde_json::to_value(m).unwrap();
        for key in ["producer", "timestamp", "id", "id_inputs", "manifest_hash"] {
            v.as_object_mut().unwrap().remove(key);
        }
        v
    }

    /// The batch partition of the listmode table at `block_rows`: `TableMultiBlockSink`'s product.
    fn sink_product(dir: &Path, total: u64, block_rows: u64) -> Manifest {
        let ws = WriteSession::create(&dir.join("sink"), "recon", "p", "d", TS).unwrap();
        let mut sw = StreamWriter::new(ws, 2, 4);
        let columns = listmode::spec(0).columns;
        let mut sink = crate::TableMultiBlockSink::with_block_rows(
            columns,
            "events",
            &dir.join("frags"),
            &mut sw,
            block_rows,
        )
        .unwrap()
        .with_row_index("t");
        for group in groups_of(total, 9999) {
            sink.push(group).unwrap();
        }
        sink.finish().unwrap();
        sw.finish(&dir.join("sink.tsra")).unwrap()
    }

    /// `push_table` of the listmode table at `block_rows` with `workers` workers.
    fn push_table_product(
        dir: &Path,
        tag: &str,
        workers: usize,
        block_rows: u64,
        groups: impl Iterator<Item = TableData>,
        total: u64,
    ) -> Manifest {
        let stage = dir.join(tag);
        let ws = WriteSession::create(&stage, "recon", "p", "d", TS).unwrap();
        let mut sw = StreamWriter::new(ws, workers, 4);
        sw.push_table_with_block_rows("events", listmode::spec(total), groups, block_rows)
            .unwrap();
        let m = sw.finish(&dir.join(format!("{tag}.tsra"))).unwrap();
        assert!(
            staged_files(&stage).is_empty(),
            "{tag}: staged file left behind"
        );
        m
    }

    /// `push_table` seals `TableMultiBlockSink`'s canonical manifest (names, markers, specs,
    /// digests, order) and `content_hash` for every worker count, nullable `ms` included, whether
    /// the input is on the row-group grid or not.
    #[test]
    fn push_table_seals_the_multi_block_sink_manifest_for_every_worker_count() {
        let total = 3 * RPG + 99; // four leaves at block_rows = RPG
        let dir = tempfile::tempdir().unwrap();
        let reference = sink_product(dir.path(), total, RPG);
        assert_eq!(reference.blocks.len(), 4);
        for (workers, size) in [(1, RPG), (2, 9999), (4, RPG)] {
            let tag = format!("w{workers}");
            let m = push_table_product(
                dir.path(),
                &tag,
                workers,
                RPG,
                groups_of(total, size),
                total,
            );
            assert_eq!(canonical(&m), canonical(&reference), "{tag}");
            assert_eq!(m.content_hash, reference.content_hash, "{tag}");
        }
    }

    /// At most BLOCK_ROWS rows, `push_table` writes the sealed bytes of `table_job_streaming`.
    #[test]
    #[allow(deprecated)] // the deprecated job is the reference this test compares against
    fn push_table_of_one_block_is_byte_identical_to_table_job_streaming() {
        let total = 3 * RPG + 99;
        let dir = tempfile::tempdir().unwrap();
        let ws = WriteSession::create(&dir.path().join("job"), "recon", "p", "d", TS).unwrap();
        let mut sw = StreamWriter::new(ws, 2, 4);
        sw.push_staged(table_job_streaming(
            "events",
            listmode::spec(total),
            listmode::groups(total),
        ))
        .unwrap();
        sw.finish(&dir.path().join("job.tsra")).unwrap();
        let ws = WriteSession::create(&dir.path().join("table"), "recon", "p", "d", TS).unwrap();
        let mut sw = StreamWriter::new(ws, 2, 4);
        let blocks = sw.push_table("events", listmode::spec(total), groups_of(total, 9999));
        assert_eq!(blocks.unwrap(), 1);
        sw.finish(&dir.path().join("table.tsra")).unwrap();
        assert!(
            std::fs::read(dir.path().join("job.tsra")).unwrap()
                == std::fs::read(dir.path().join("table.tsra")).unwrap(),
            "push_table's single block differs from table_job_streaming's"
        );
    }

    /// ADR-0064 §8's barrier test. With all `W` workers held before they read, the reader fills the
    /// `W` dispatched blocks and one more, then stalls: exactly `W + 1` staged input blocks, a
    /// partial block's bytes counted while it fills, and no further row-group pulled although `W`
    /// more blocks of input are offered. Released, the product is the batch partition.
    #[test]
    fn held_workers_pin_w_plus_one_staged_input_blocks() {
        let block_rows = 2 * RPG;
        for workers in [1usize, 2, 3] {
            let w = workers as u64;
            let total = (2 * w + 1) * block_rows + 1234;
            let dir = tempfile::tempdir().unwrap();
            let stage = dir.path().join("stage");
            let ws = WriteSession::create(&stage, "recon", "p", "d", TS).unwrap();
            let mut sw = StreamWriter::new(ws, workers, 64);
            let hold = sw.hold_table_reads();
            // The generator records each pulled group's staged size, and at the second group of
            // block `W` (block `W` partly filled) waits for the monitor to look.
            let staged = Arc::new(Mutex::new(Vec::<u64>::new()));
            let (at_mid, mid) = channel::<()>();
            let (ack, acked) = channel::<()>();
            let mid_pull = 2 * workers + 1;
            let sizes = Arc::clone(&staged);
            let input = listmode::groups(total).enumerate().map(move |(i, group)| {
                if i == mid_pull {
                    at_mid.send(()).unwrap();
                    let _ = acked.recv();
                }
                let mut record = Vec::new();
                crate::accumulate::write_fragment_to(&mut record, &group).unwrap();
                sizes.lock().unwrap().push(record.len() as u64);
                group
            });
            let sizes = Arc::clone(&staged);
            let monitor = thread::spawn(move || {
                let written = |n: usize| sizes.lock().unwrap()[..n].iter().sum::<u64>();
                mid.recv().unwrap();
                let st = hold.stats();
                assert_eq!(st.peak_staged_input_blocks, workers + 1, "mid-fill");
                assert_eq!(
                    st.peak_staged_input_bytes,
                    written(mid_pull),
                    "partial block"
                );
                ack.send(()).unwrap();
                // The reader must stall at the hand-over of block `W` (every worker has a job), not
                // pull past it: a writer that hands blocks to a queue instead of a free worker
                // keeps pulling, which fails here rather than hanging.
                let full = 2 * (workers + 1);
                while hold.stats().producer_stalls == 0 {
                    let pulled = sizes.lock().unwrap().len();
                    assert!(
                        pulled <= full,
                        "pulled {pulled} groups past the W + 1-th block"
                    );
                    thread::yield_now();
                }
                let st = hold.stats();
                let pulled = sizes.lock().unwrap().len();
                assert_eq!(pulled, full, "pulled past the W + 1-th block");
                assert_eq!(
                    st.jobs_pushed, workers,
                    "block W was handed over to a busy pool"
                );
                assert_eq!(st.peak_staged_input_blocks, workers + 1);
                assert_eq!(st.peak_staged_input_bytes, written(pulled));
                drop(hold);
            });
            let spec = listmode::spec(total);
            sw.push_table_with_block_rows("events", spec, input, block_rows)
                .unwrap();
            monitor.join().unwrap();
            let (m, stats) = sw.finish_with_stats(&dir.path().join("p.tsra")).unwrap();
            assert_eq!(stats.peak_staged_input_blocks, workers + 1, "w{workers}");
            assert!(
                staged_files(&stage).is_empty(),
                "w{workers}: staged file left behind"
            );
            let reference = sink_product(dir.path(), total, block_rows);
            assert_eq!(canonical(&m), canonical(&reference), "w{workers}");
        }
    }

    /// A row count that disagrees with `spec.rows`, or an oversized group, is a typed error; the
    /// partial table is never sealed and no staging file is left behind.
    #[test]
    fn a_wrong_row_count_is_a_typed_error_and_leaves_no_staging_file() {
        let total = 3 * RPG;
        let cases: [(u64, StreamedTableError); 3] = [
            (
                total + 10,
                StreamedTableError::TooManyRows { declared: total },
            ),
            (
                total - 5,
                StreamedTableError::TooFewRows {
                    declared: total,
                    actual: total - 5,
                },
            ),
            (
                RPG + 1,
                StreamedTableError::OversizedGroup {
                    rows: RPG + 1,
                    max: RPG,
                },
            ),
        ];
        for (fed, expected) in cases {
            let dir = tempfile::tempdir().unwrap();
            let stage = dir.path().join("stage");
            let ws = WriteSession::create(&stage, "recon", "p", "d", TS).unwrap();
            let mut sw = StreamWriter::new(ws, 2, 4);
            let size = if fed == RPG + 1 { RPG + 1 } else { RPG };
            let err = sw
                .push_table_with_block_rows(
                    "events",
                    listmode::spec(total),
                    groups_of(fed, size),
                    RPG,
                )
                .unwrap_err();
            assert!(
                matches!(&err, Error::StreamedTable { name, reason } if name == "events" && *reason == expected),
                "{err}"
            );
            // The leaves handed over before the error are committed; the terminal one never is.
            let sealed = sw.finish(&dir.path().join("p.tsra"));
            if matches!(expected, StreamedTableError::OversizedGroup { .. }) {
                assert_eq!(sealed.unwrap().blocks.len(), 0);
            } else {
                assert!(
                    matches!(sealed, Err(Error::LogicalTable { .. })),
                    "{sealed:?}"
                );
            }
            assert!(
                staged_files(&stage).is_empty(),
                "{expected}: staged file left behind"
            );
        }
    }

    /// Above BLOCK_ROWS, the deprecated single-block job fails with a typed error naming
    /// `push_table` instead of writing one oversized block (ADR-0064 §5.4).
    #[test]
    #[allow(deprecated)] // tests the deprecated job's refusal
    fn table_job_streaming_above_block_rows_names_push_table() {
        let dir = tempfile::tempdir().unwrap();
        let stage = dir.path().join("stage");
        let ws = WriteSession::create(&stage, "recon", "p", "d", TS).unwrap();
        let mut sw = StreamWriter::new(ws, 1, 1);
        let rows = BLOCK_ROWS as u64 + 1;
        sw.push_staged(table_job_streaming(
            "events",
            listmode::spec(rows),
            std::iter::empty(),
        ))
        .unwrap();
        let out = dir.path().join("p.tsra");
        let err = sw.finish(&out).unwrap_err();
        assert!(
            matches!(
                err,
                Error::StreamedTable {
                    reason: StreamedTableError::NeedsPartition { rows: r, .. },
                    ..
                } if r == rows
            ),
            "{err}"
        );
        assert!(err.to_string().contains("push_table"), "{err}");
        // Refused before anything is staged or sealed.
        assert!(staged_files(&stage).is_empty(), "staged file left behind");
        assert!(!out.exists(), "a product was sealed");
    }
}
