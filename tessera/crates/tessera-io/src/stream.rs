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
//!   logical table (ADR-0064), each leaf one staged job; [`StreamWriter::open_table`] writes the
//!   same leaves for a table whose row count is known only at its close (#806). Disk is not bounded by `B`: the staged files of finished-but-uncommitted
//!   jobs hold `≤ J × S` bytes (`S` = the largest staged block, [`StreamStats::max_staged_job_bytes`]),
//!   plus the files of the encodes in progress.
//! - **Parallel encode:** `workers` threads encode concurrently (the expensive pcodec/Vortex step).
//! - **Deterministic output:** every job is pushed on a [`Lane`] (ADR-0065 §7.3, #832): one per
//!   table role, plus the arrays lane, which takes every job pushed without one. Each lane has its
//!   own sequence, and the committer commits each lane's jobs in that lane's **push order**,
//!   independently of the other lanes (a reorder buffer keyed by `(lane, seq)` holds out-of-order
//!   completions). The seal orders the blocks by lane, then by push order within the lane (see
//!   [`WriteSession::seal`]), so the sealed `content_hash` / `id` / `manifest_hash` are
//!   **byte-identical to a batch writer** that added the same blocks in that order, however the
//!   lanes' pushes interleaved. Streaming changes *how* the bytes are produced, never *which*
//!   bytes; neither the worker count, the window nor the interleaving of lanes changes them. A
//!   writer fed on one lane seals in push order, as it did before lanes.
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
//! 2. **Encode start** (a worker, after dequeuing job `seq` of a lane): it starts encoding only
//!    while the encoded bytes held are below `B` — **unless `seq` is the job the committer waits on
//!    in that lane**, which always runs. This is what makes `B` bind: once `B` is reached, only the
//!    in-progress encodes (at most `min(W, J)`) can still add bytes.
//!
//! **Deadlock freedom.** Credits are released only by an in-order commit, so the window must never
//! stall the job the committer waits on in any lane (that lane's `next`). It can't: a lane's `next`
//! was pushed (every job of the lane below its newest push was), the job queue is FIFO so `next`
//! is dequeued before any later job of its lane, and gate 2 exempts it whatever the other lanes
//! hold. So every lane's `next` encodes, commits, and frees its credit; a job too large for `B`
//! simply runs alone once it is its lane's `next`. Gate 1 never waits on an empty window. A failed encode, a failed
//! commit or a panicking worker **closes** the window, which wakes every waiter: `push` then returns
//! an error and the workers waiting at gate 2 exit, so a dead pipeline never hangs the producer
//! (idle workers exit once [`StreamWriter::finish`] or a drop closes the job queue).

mod sequenced;

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, Weak};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use tessera_core::block::table::TableSpec;
use tessera_core::block::{BlockName, BlockRef, StreamedTableError};
use tessera_core::manifest::Manifest;
use tessera_core::{Error, Result};

use crate::accumulate::{read_fragment_from, write_fragment_to};
use crate::table::{
    block_name, block_partition, partition_blocks, ColumnData, TableData, BLOCK_ROWS,
    ROWS_PER_GROUP,
};
use crate::write::{BlockStaging, Lane, StagedBlock, WriteSession};
use crate::BlockPayload;

pub use sequenced::{SequencedOptions, SequencedStats, SequencedTable, MAX_STALL_TIMEOUT};

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

/// The table block's manifest reference without a digest (a staged commit fills it in). Its spec
/// records the writer's edition ([`crate::table::TABLE_EDITION`]), as every sealed table block's does.
fn staged_table_ref(
    name: String,
    spec: &tessera_core::block::table::TableSpec,
) -> Result<BlockRef> {
    Ok(BlockRef {
        name: tessera_core::block::BlockName::new(name)?,
        kind: tessera_core::block::BlockKind::Table,
        digest: None,
        spec: serde_json::to_value(crate::table::sealed_spec(spec)?)?,
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
        let bref = staged_table_ref(name, &spec)?;
        let mut staged = staging.create()?;
        crate::table::encode_streaming_to(&spec, groups, &mut staged)?;
        Ok(((bref, staged), Vec::new()))
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
        let bref = staged_table_ref(name, &spec)?;
        let mut staged = staging.create()?;
        crate::accumulate::compact_fragments_to(&spec, frag_paths, &mut staged)?;
        Ok(((bref, staged), Vec::new()))
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

/// Where a job sits in commit order: its lane (an index into the writer's lanes, in the order of
/// their first push) and its sequence number within that lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Slot {
    lane: usize,
    seq: usize,
}

/// A job as the queue carries it: its slot, its lane (for the commit) and the work.
type Queued = (Slot, Lane, Job);

/// What a worker hands the committer: the job's slot and lane, the bytes it charged to the window
/// (so the commit can give exactly that back), and the encode result.
type Encoded = (Slot, Lane, Held, Result<JobOutput>);

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
    /// Jobs committed to the session so far, each lane's in its push order.
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
    /// Per lane (by index), the `seq` the committer commits next in it — the jobs the window must
    /// never stall.
    next_commit: Vec<usize>,
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
    /// The ordering gate's lanes, by index (#812): empty in a writer's encode window.
    order: Vec<LaneGate>,
    /// The writer's first refusal by a sequenced input, with the table it happened on, held by
    /// both the ordering gate and the encode window (ADR-0065 §7.2: every refusal poisons the
    /// session): every waiter, every later admission and [`StreamWriter::finish`] return it.
    poison: Option<(String, StreamedTableError)>,
}

/// One [`SequencedTable`]'s state in the writer's ordering gate.
struct LaneGate {
    lane: Lane,
    name: String,
    options: SequencedOptions,
    /// The next sequence number due; `None` once `u64::MAX` was released, when no number is left.
    next: Option<u64>,
    /// Set by a successful [`SequencedTable::close`]: [`StreamWriter::finish`] refuses while a
    /// lane isn't.
    closed: bool,
    /// The sequence numbers entered and not yet released (waiting, admitted or being released).
    reserved: BTreeSet<u64>,
    /// Of those, the batches admitted.
    admitted: usize,
    /// When `next` last advanced (or the lane opened): the stall deadline runs from here.
    last_advance: Instant,
    stats: SequencedStats,
}

/// The shared credit window: the limits, the state, and one condvar every gate waits on.
struct CreditWindow {
    limits: StreamWindow,
    workers: usize,
    state: Mutex<WindowState>,
    changed: Condvar,
    /// A window closed with this one: the writer's ordering gate, which a dead pipeline closes.
    cascade: Option<Arc<CreditWindow>>,
    /// The window this one's refusals poison: the ordering gate's encode window (a `Weak`, as
    /// that window owns the gate through `cascade`).
    upstream: Weak<CreditWindow>,
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
                next_commit: Vec::new(),
                closed: false,
                gated_encodes: 0,
                busy_jobs: 0,
                staged_input_blocks: 0,
                staged_input_bytes: 0,
                hold_reads: false,
                stats: StreamStats::default(),
                order: Vec::new(),
                poison: None,
            }),
            changed: Condvar::new(),
            cascade: None,
            upstream: Weak::new(),
        }
    }

    /// A writer's encode window with `limits`, and its ordering gate (#812) as `cascade`: closing
    /// the window closes the gate, and the gate's refusals poison the window.
    fn with_order(limits: StreamWindow, workers: usize) -> Arc<Self> {
        // The sequenced inputs' byte credit is the encode window's byte budget; their per-lane
        // windows are counted by the gate itself.
        let order_limits = StreamWindow::new(usize::MAX, limits.max_in_flight_bytes);
        Arc::new_cyclic(|window| CreditWindow {
            cascade: Some(Arc::new(CreditWindow {
                upstream: window.clone(),
                ..CreditWindow::new(order_limits, 0)
            })),
            ..CreditWindow::new(limits, workers)
        })
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

    /// Gate 1 (producer): wait for a job credit for a job on `lane` — and, with `free_worker`, for
    /// a worker with no job — then take it. `Err` once the window is closed.
    fn acquire(&self, lane: usize, free_worker: bool) -> Result<()> {
        let mut st = self.lock();
        if st.next_commit.len() <= lane {
            st.next_commit.resize(lane + 1, 0); // a lane's first push: nothing of it committed
        }
        let mut stalled = false;
        loop {
            if let Some(e) = Self::refused(&st) {
                return Err(e);
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

    /// Gate 2 (worker): wait until the job at `slot` may start encoding — always if it is the job
    /// the committer waits on in its lane, otherwise while the held bytes are below the budget.
    /// `false` once the window is closed (the worker should exit).
    fn wait_encode_turn(&self, slot: Slot) -> bool {
        let mut st = self.lock();
        let mut gated = false;
        let turn = loop {
            if st.closed {
                break false;
            }
            if st.next_commit[slot.lane] == slot.seq
                || st.in_flight_bytes < self.limits.max_in_flight_bytes
            {
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

    /// The committer appended the job `next_commit[lane]` (which held `held`): give its credit back.
    fn release(&self, lane: usize, held: Held) {
        let mut st = self.lock();
        st.in_flight_jobs -= 1;
        st.in_flight_bytes -= held.ram;
        st.staged_bytes -= held.staged;
        if let Some(gate) = st.order.get_mut(lane) {
            // The ordering gate: release the lane's due batch (admitted, so `next` is `Some`).
            if let Some(next) = gate.next {
                gate.reserved.remove(&next);
                gate.next = next.checked_add(1);
            }
            gate.admitted -= 1;
            gate.last_advance = Instant::now();
        } else {
            st.next_commit[lane] += 1;
        }
        st.stats.jobs_committed += 1;
        drop(st);
        self.changed.notify_all();
    }

    /// The pipeline can make no more progress: wake every waiter so none hangs.
    fn close(&self) {
        self.lock().closed = true;
        self.changed.notify_all();
        if let Some(gate) = &self.cascade {
            gate.close();
        }
    }

    /// The ordering gate (#812): add the lane of a [`SequencedTable`], one per [`Lane`].
    fn add_lane(&self, lane: Lane, name: String, options: SequencedOptions) -> Result<usize> {
        let mut st = self.lock();
        if let Some(e) = Self::refused(&st) {
            return Err(e);
        }
        if st.order.iter().any(|gate| gate.lane == lane) {
            return Err(Error::Invalid(format!(
                "sequenced table '{name}': its lane {lane:?} already has a sequenced input"
            )));
        }
        st.order.push(LaneGate {
            lane,
            name,
            options,
            next: Some(options.first_seq),
            closed: false,
            reserved: BTreeSet::new(),
            admitted: 0,
            last_advance: Instant::now(),
            stats: SequencedStats::default(),
        });
        Ok(st.order.len() - 1)
    }

    /// The ordering gate: enter batch `seq` of `lane`, holding `bytes`, and wait for its
    /// admission (see [`SequencedTable`]).
    fn admit(&self, lane: usize, seq: u64, bytes: u64) -> Result<()> {
        let entered = Instant::now();
        let mut st = self.lock();
        if let Some(e) = Self::refused(&st) {
            return Err(e);
        }
        let gate = &st.order[lane];
        let o = gate.options;
        if seq < o.first_seq {
            let reason = StreamedTableError::BeforeFirst {
                seq,
                first_seq: o.first_seq,
            };
            return Err(self.poison_locked(st, lane, reason));
        }
        // `None`: every number up to `u64::MAX` was released, so `seq` was pushed before.
        let next = match gate.next {
            Some(next) if seq >= next && !gate.reserved.contains(&seq) => next,
            _ => return Err(self.poison_locked(st, lane, StreamedTableError::Duplicate { seq })),
        };
        let horizon = o.horizon() as u64;
        if seq - next >= horizon {
            let reason = StreamedTableError::TooFarAhead {
                seq,
                next,
                max_ahead: horizon,
            };
            return Err(self.poison_locked(st, lane, reason));
        }
        st.order[lane].reserved.insert(seq);
        let mut blocked = false;
        loop {
            if let Some(e) = Self::refused(&st) {
                return Err(e);
            }
            // `next` can't pass `seq` while it waits: `seq` is reserved and not yet released, so
            // `next` is a number at most `seq`.
            let next = st.order[lane]
                .next
                .expect("a reserved seq is not yet released");
            if seq == next
                || (seq - next < o.window as u64
                    && st.in_flight_bytes < self.limits.max_in_flight_bytes)
            {
                break;
            }
            if o.refuse_instead_of_block {
                let reason = StreamedTableError::WouldBlock { seq, next };
                return Err(self.poison_locked(st, lane, reason));
            }
            if !blocked {
                blocked = true;
                st.order[lane].stats.blocked_pushes += 1;
                st.stats.producer_stalls += 1;
            }
            let deadline = st.order[lane].last_advance.max(entered) + o.stall_timeout;
            let now = Instant::now();
            if now >= deadline {
                return Err(self.poison_locked(st, lane, StreamedTableError::Stalled { next }));
            }
            st = self
                .changed
                .wait_timeout(st, deadline - now)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        st.in_flight_jobs += 1;
        st.in_flight_bytes += bytes;
        st.stats.peak_in_flight_bytes = st.stats.peak_in_flight_bytes.max(st.in_flight_bytes);
        let peak_bytes = st.stats.peak_in_flight_bytes;
        let gate = &mut st.order[lane];
        gate.admitted += 1;
        gate.stats.peak_admitted = gate.stats.peak_admitted.max(gate.admitted);
        gate.stats.peak_admitted_bytes = peak_bytes;
        Ok(())
    }

    /// The window's refusal once it is closed: its poison, or the dead pipeline's error.
    fn refused(st: &WindowState) -> Option<Error> {
        match &st.poison {
            Some((name, reason)) => Some(Error::StreamedTable {
                name: name.clone(),
                reason: reason.clone(),
            }),
            None => st.closed.then(closed_error),
        }
    }

    fn refusal(&self) -> Option<Error> {
        Self::refused(&self.lock())
    }

    /// The poison alone: the first refusal of a sequenced input, if there was one.
    fn poisoned(&self) -> Option<Error> {
        let st = self.lock();
        st.poison.as_ref().and_then(|_| Self::refused(&st))
    }

    /// Poison the ordering gate with `reason` on `lane` (unless it already refused or the
    /// pipeline died), wake every waiter, poison the encode window with it too, and return the
    /// gate's refusal.
    fn poison(&self, lane: usize, reason: StreamedTableError) -> Error {
        self.poison_locked(self.lock(), lane, reason)
    }

    fn poison_locked(
        &self,
        mut st: MutexGuard<'_, WindowState>,
        lane: usize,
        reason: StreamedTableError,
    ) -> Error {
        let first =
            (st.poison.is_none() && !st.closed).then(|| (st.order[lane].name.clone(), reason));
        if first.is_some() {
            st.poison.clone_from(&first);
        }
        st.closed = true;
        let e = Self::refused(&st).expect("closed");
        drop(st);
        self.changed.notify_all();
        if let (Some(poison), Some(window)) = (first, self.upstream.upgrade()) {
            window.poison_with(poison);
        }
        e
    }

    /// Hold `poison` as this (encode) window's refusal, unless the pipeline already died, and
    /// close it: every waiter and every later push return it.
    fn poison_with(&self, poison: (String, StreamedTableError)) {
        {
            let mut st = self.lock();
            if st.poison.is_none() && !st.closed {
                st.poison = Some(poison);
            }
        }
        self.close();
    }

    /// A successful [`SequencedTable::close`] of `lane`.
    fn close_lane(&self, lane: usize) {
        self.lock().order[lane].closed = true;
    }

    /// Whether `lane` was never closed (see [`Self::close_lane`]).
    fn unclosed_lane(&self, lane: usize) -> bool {
        !self.lock().order[lane].closed
    }

    /// [`StreamedTableError::NotClosed`] for the first sequenced input that was never closed.
    fn unclosed(&self) -> Option<Error> {
        self.lock()
            .order
            .iter()
            .find(|gate| !gate.closed)
            .map(|gate| Error::StreamedTable {
                name: gate.name.clone(),
                reason: StreamedTableError::NotClosed,
            })
    }

    fn sequenced_stats(&self, lane: usize) -> SequencedStats {
        self.lock().order[lane].stats
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

/// A partitioned table being streamed into a [`StreamWriter`] whose row count is known only when
/// it is closed (ADR-0065 §7.3, #806). Open it with [`StreamWriter::open_table`], feed it with
/// [`Self::push`], end it with [`Self::close`].
///
/// The handle doesn't borrow the writer: each call takes the `&mut StreamWriter` it was opened on,
/// so a producer can keep the table between calls without holding the writer, and push other jobs
/// to the writer in between. The table's leaves are pushed on the [`Lane`] it was opened on
/// ([`StreamWriter::open_table_on`]; the arrays lane for [`StreamWriter::open_table`]): other jobs
/// on that lane commit in push order, interleaved with the leaves, and jobs on other lanes are
/// sealed apart from them, in lane order.
///
/// **One leaf of lookahead.** Rows are re-chunked to the [`ROWS_PER_GROUP`] grid and staged leaf
/// by leaf, exactly as [`StreamWriter::push_table`] stages them. A full leaf `i` is handed to an
/// encode worker as the non-terminal leaf `name/{i:04}` as soon as a row arrives after it, so only
/// the last leaf waits for [`Self::close`], which hands it over as the terminal leaf, or as the
/// bare-named single block `name` if it is the only one. Nothing is renamed: a leaf's name and
/// marker are fixed when it is handed over. The sealed product is byte-identical to
/// [`crate::TableMultiBlockSink`]'s for the same rows, whatever the worker count, the window and
/// the input's chunking.
///
/// **Disk and heap.** For one open table the bounds of [`StreamWriter::push_table`] hold
/// unchanged: at most `W + 1` leaves of input are staged on disk for `W` workers, up to
/// `(W + 1) · BLOCK_ROWS` rows ([`StreamStats::peak_staged_input_blocks`]), because the held full
/// leaf is handed over before the next leaf's staging file is created, and the heap bound of
/// `push_table` with the same `s` (one row-group re-chunk buffer). With `T` tables open on one
/// writer at once each holds its own leaf, so at most `W + T` leaves and `T` re-chunk buffers.
/// What differs is time, not size: a full leaf stays staged until the producer pushes another row
/// or closes, however long that takes. [`Self::push`] blocks while every worker is busy or the
/// window is full, like `push_table`.
///
/// **Failure.** Any error of [`Self::push`] or [`Self::close`] abandons the table: the staging
/// file of the leaf being filled is deleted at once, the leaves already handed over stay in the
/// pipeline (committed, unless the pipeline itself has died) but the terminal leaf never is (so [`StreamWriter::finish`] refuses to seal a partitioned
/// remnant with [`Error::LogicalTable`]; a table that never left leaf 0 leaves nothing, and the
/// product seals without it), and every later call returns [`Error::Invalid`]. Dropping the handle
/// without closing it does the same, which is why the handle is `#[must_use]`.
/// Staging files of leaves already handed over are deleted by their workers, or when their jobs
/// are dropped. [`Self::push`] on another writer than the one it was opened on is
/// [`Error::Invalid`] and changes nothing; [`Self::close`] on another writer is the same error,
/// and abandons the table, since `close` consumes it.
#[must_use = "a table is written only by OpenTable::close; dropping it abandons the table"]
pub struct OpenTable {
    name: String,
    /// The leaf template: `rows` 0, `partition` `None`.
    spec: TableSpec,
    block_rows: u64,
    /// [`StreamWriter::push_table`]'s declared row count; `None` when it's known only at close.
    declared: Option<u64>,
    /// The window of the writer this table was opened on: the identity [`Self::push`] checks.
    window: Arc<CreditWindow>,
    /// The lane the leaves are pushed on.
    lane: Lane,
    fill: Fill,
    /// Rows short of a full row-group, when the input is not on the ROWS_PER_GROUP grid.
    pending: Option<TableData>,
    seen: u64,
    failed: bool,
}

impl OpenTable {
    fn new(
        writer: &StreamWriter,
        lane: Lane,
        name: String,
        spec: TableSpec,
        block_rows: u64,
        declared: Option<u64>,
    ) -> Result<Self> {
        let call = if declared.is_some() {
            "push_table"
        } else {
            "open_table"
        };
        if block_rows == 0 || !block_rows.is_multiple_of(ROWS_PER_GROUP as u64) {
            return Err(Error::Codec(format!(
                "{call}: block_rows ({block_rows}) must be a positive multiple of ROWS_PER_GROUP ({ROWS_PER_GROUP})"
            )));
        }
        if spec.partition.is_some() {
            return Err(Error::Invalid(format!(
                "{call} '{name}': spec.partition must be None, each leaf's marker is set by the writer"
            )));
        }
        // Each leaf's spec records the writer's edition when it is handed over; a spec naming another
        // edition is refused here, before any row is staged.
        crate::table::sealed_spec(&spec)?;
        for c in &spec.columns {
            ColumnData::validate_dtype(&c.dtype)?;
        }
        // With a declared count the last name is the longest: if it is valid, every name is.
        // Without one, check the bare name and the first leaf, since the rows can't tell yet which
        // the table needs; a later leaf's longer name is checked when it is handed over.
        let names = match declared {
            Some(rows) => {
                let blocks = partition_blocks(rows, block_rows);
                vec![(blocks - 1, blocks)]
            }
            None => vec![(0, 1), (0, 2)],
        };
        for (idx, total) in names {
            BlockName::new(block_name(&name, idx, total)?)?;
        }
        Ok(OpenTable {
            name,
            spec,
            block_rows,
            declared,
            window: Arc::clone(&writer.window),
            lane,
            fill: Fill::default(),
            pending: None,
            seen: 0,
            failed: false,
        })
    }

    /// The rows pushed so far.
    pub fn rows(&self) -> u64 {
        self.seen
    }

    /// Append one row-group of at most [`ROWS_PER_GROUP`] rows with the spec's columns, on
    /// `writer`, the writer the table was opened on. Groups of any size up to that are re-chunked
    /// to the grid through a buffer of less than one row-group. Blocks while a leaf this push
    /// completes or makes non-terminal waits for a free worker or for the window.
    ///
    /// # Errors
    ///
    /// [`Error::StreamedTable`] for a group of more than `ROWS_PER_GROUP` rows; the column and
    /// dtype errors of [`StreamWriter::push_table`]; I/O errors staging the rows; the pipeline's
    /// errors; [`Error::Invalid`] for another writer or a table abandoned by an earlier error.
    /// Every error but the wrong writer abandons the table (see [`OpenTable`]).
    pub fn push(&mut self, writer: &mut StreamWriter, group: TableData) -> Result<()> {
        self.push_in(writer, group)
    }

    /// [`Self::push`] through a shared writer: a [`SequencedTable`]'s release.
    fn push_in(&mut self, writer: &StreamWriter, group: TableData) -> Result<()> {
        self.check(writer)?;
        let res = self.push_group(writer, group);
        self.abandon_on_error(res)
    }

    /// End the table on `writer`: stage the buffered rows and hand over the last leaf, and return
    /// the number of leaves. `1` is the single block named `name` (an empty table included);
    /// more are the leaves `name/0000` … with the terminal marker on the last. Call
    /// [`StreamWriter::finish`] to commit and seal as usual.
    ///
    /// # Errors
    ///
    /// As [`Self::push`]; a table abandoned by an earlier error is [`Error::Invalid`]. The table
    /// is abandoned on every error.
    pub fn close(self, writer: &mut StreamWriter) -> Result<u64> {
        self.close_in(writer)
    }

    /// [`Self::close`] through a shared writer.
    fn close_in(mut self, writer: &StreamWriter) -> Result<u64> {
        self.check(writer)?;
        if let Some(declared) = self.declared.filter(|&d| d != self.seen) {
            return Err(self.refuse(StreamedTableError::TooFewRows {
                declared,
                actual: self.seen,
            }));
        }
        if let Some(rest) = self.pending.take() {
            self.stage_group(writer, &rest)?;
        }
        let blocks = self.fill.block + 1;
        self.dispatch(writer, blocks)?;
        Ok(blocks)
    }

    fn check(&self, writer: &StreamWriter) -> Result<()> {
        if !Arc::ptr_eq(&self.window, &writer.window) {
            return Err(Error::Invalid(format!(
                "table '{}' was opened on another StreamWriter",
                self.name
            )));
        }
        if self.failed {
            return Err(Error::Invalid(format!(
                "table '{}' was abandoned by an earlier error",
                self.name
            )));
        }
        Ok(())
    }

    /// On an error, drop the leaf being filled (deleting its staging file) and the buffered rows.
    fn abandon_on_error<T>(&mut self, res: Result<T>) -> Result<T> {
        if res.is_err() {
            self.failed = true;
            self.fill = Fill::default();
            self.pending = None;
        }
        res
    }

    fn refuse(&self, reason: StreamedTableError) -> Error {
        Error::StreamedTable {
            name: self.name.clone(),
            reason,
        }
    }

    fn push_group(&mut self, writer: &StreamWriter, group: TableData) -> Result<()> {
        let rows = group_rows(&self.spec, &group)?;
        if rows > ROWS_PER_GROUP {
            return Err(self.refuse(StreamedTableError::OversizedGroup {
                rows: rows as u64,
                max: ROWS_PER_GROUP as u64,
            }));
        }
        self.seen += rows as u64;
        if let Some(declared) = self.declared.filter(|&d| self.seen > d) {
            return Err(self.refuse(StreamedTableError::TooManyRows { declared }));
        }
        let mut buf = match self.pending.take() {
            None if rows == ROWS_PER_GROUP => return self.stage_group(writer, &group),
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
            self.stage_group(writer, &head)?;
            buf = tail;
        }
        if rows_of(&buf) > 0 {
            self.release_full(writer)?;
            self.pending = Some(buf);
        }
        Ok(())
    }

    /// The lookahead: a row past a full leaf makes it non-terminal, so hand it over. `total =
    /// index + 2` names and marks it as the leaf it is in any table of more leaves than that.
    /// With a declared count a full leaf is handed over on filling, unless it is the last.
    fn release_full(&mut self, writer: &StreamWriter) -> Result<()> {
        if self.fill.rows == self.block_rows {
            self.dispatch(writer, self.fill.block + 2)?;
        }
        Ok(())
    }

    /// Append one canonical row-group to the leaf being filled (creating its staging file), and
    /// hand the leaf over once it is full and known not to be the last.
    fn stage_group(&mut self, writer: &StreamWriter, group: &TableData) -> Result<()> {
        self.release_full(writer)?;
        if self.fill.input.is_none() {
            self.fill.input = Some(StagedInput::create(&writer.staging, &writer.window)?);
        }
        self.fill.input.as_mut().expect("created").append(group)?;
        self.fill.rows += rows_of(group) as u64;
        if let Some(rows) = self.declared {
            let blocks = partition_blocks(rows, self.block_rows);
            if self.fill.rows == self.block_rows && self.fill.block + 1 < blocks {
                self.dispatch(writer, blocks)?;
            }
        }
        Ok(())
    }

    /// Hand the leaf being filled to a free worker as one staged job, named and marked as the
    /// leaf `fill.block` of `total`.
    fn dispatch(&mut self, writer: &StreamWriter, total: u64) -> Result<()> {
        let leaf = TableSpec {
            rows: self.fill.rows,
            partition: block_partition(self.fill.block, total),
            ..self.spec.clone()
        };
        let bref = staged_table_ref(block_name(&self.name, self.fill.block, total)?, &leaf)?;
        let input = match self.fill.input.take() {
            Some(input) => input,
            None => StagedInput::create(&writer.staging, &writer.window)?, // the empty table
        };
        writer.submit(&self.lane, Job::Staged(input.into_job(bref, leaf)?), true)?;
        self.fill = Fill {
            block: self.fill.block + 1,
            ..Fill::default()
        };
        Ok(())
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
    input: Option<Sender<Queued>>,
    workers: Vec<JoinHandle<()>>,
    committer: Option<JoinHandle<Result<WriteSession>>>,
    window: Arc<CreditWindow>,
    /// The ordering gate of the writer's [`SequencedTable`]s, closed with `window` (#812).
    order: Arc<CreditWindow>,
    staging: BlockStaging,
    /// The lanes pushed on, in the order of their first push (a [`Slot`]'s lane indexes it), each
    /// with the `seq` its next job takes.
    lanes: Mutex<Vec<(Lane, usize)>>,
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
        let window = CreditWindow::with_order(window, workers);
        let order = Arc::clone(
            window
                .cascade
                .as_ref()
                .expect("a writer's window has its gate"),
        );
        // The channels only carry jobs; the window is what bounds them (at most `max_in_flight`
        // jobs sit in both together). Unbounded channels allocate nothing up front, so a window of
        // `usize::MAX` jobs (bound by bytes only) is fine.
        let (in_tx, in_rx) = channel::<Queued>();
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
                        let Ok((slot, lane, job)) = job else {
                            break; // input closed + drained
                        };
                        if !window.wait_encode_turn(slot) {
                            break; // pipeline closed
                        }
                        let res = job.run(&staging);
                        let held = res.as_ref().map_or(Held::default(), JobOutput::held);
                        window.charge(held);
                        if tx.send((slot, lane, held, res)).is_err() {
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
            order,
            staging,
            lanes: Mutex::new(Vec::new()),
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
    /// payloads held. The job is pushed on [`Lane::Arrays`], and the jobs of a lane are committed
    /// in call order, so a writer fed only by this call (and the others without a lane) seals its
    /// blocks in push order. Errors once the pipeline died (a failed encode or commit, or a
    /// panicking worker); [`Self::finish`] then reports the cause.
    pub fn push(&mut self, job: EncodeJob) -> Result<()> {
        self.push_on(Lane::Arrays, job)
    }

    /// [`Self::push`] on `lane`: the block is committed in push order among `lane`'s jobs,
    /// independently of the other lanes, and sealed in lane order (see [`Lane`]). The window, its
    /// backpressure and the errors are those of `push`, shared by every lane.
    pub fn push_on(&mut self, lane: Lane, job: EncodeJob) -> Result<()> {
        self.submit(&lane, Job::InMemory(job), false)
    }

    /// Enqueue a [`StagedEncodeJob`]: like [`Self::push`] (same window, same lane, same commit
    /// order), but the job writes its data block to a staged file, which the committer streams into
    /// the product and deletes, so the block's bytes never count against the window's byte budget.
    pub fn push_staged(&mut self, job: StagedEncodeJob) -> Result<()> {
        self.push_staged_on(Lane::Arrays, job)
    }

    /// [`Self::push_staged`] on `lane`, ordered as [`Self::push_on`].
    pub fn push_staged_on(&mut self, lane: Lane, job: StagedEncodeJob) -> Result<()> {
        self.submit(&lane, Job::Staged(job), false)
    }

    /// Write the table `name` of `spec.rows` rows from `groups` as the leaves of one logical table
    /// (ADR-0064 §5), each leaf one staged job under this writer's window, and return the number of
    /// leaves, on [`Lane::Arrays`]. The table is partitioned at [`BLOCK_ROWS`] ([`crate::table::block_count`]): a table
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
    /// 12-column schema with a 2,966 B entry (the consumer's, read off the sealed manifests,
    /// ADR-0064 as amended by #770) that is 7,980 B per leaf, 4.6 MB at 573 leaves. The term holds
    /// since #759 (PR #765); before it, a committed leaf's spec was held as a `serde_json::Value`
    /// tree, several times `e_leaf` (12.8 KB live for that schema).
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
    /// of it (at its end) or yields a group of more than `ROWS_PER_GROUP` rows; [`Error::Codec`]
    /// before anything is staged when `spec.edition` names an edition other than
    /// [`crate::table::TABLE_EDITION`]; the name, column and dtype errors of the sinks; and the
    /// pipeline's errors (a dead pipeline closes the window). A
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
        self.push_table_in(Lane::Arrays, name.into(), spec, groups, BLOCK_ROWS as u64)
    }

    /// [`Self::push_table`] on `lane` (usually [`Lane::table`] of the table's role): the leaves are
    /// committed in order among `lane`'s jobs, independently of the other lanes, and sealed in
    /// lane order (see [`Lane`]). The bounds and errors are those of `push_table`.
    pub fn push_table_on<I>(
        &mut self,
        lane: Lane,
        name: impl Into<String>,
        spec: TableSpec,
        groups: I,
    ) -> Result<u64>
    where
        I: IntoIterator<Item = TableData>,
    {
        self.push_table_in(lane, name.into(), spec, groups, BLOCK_ROWS as u64)
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
        self.push_table_in(Lane::Arrays, name.into(), spec, groups, block_rows)
    }

    fn push_table_in<I>(
        &mut self,
        lane: Lane,
        name: String,
        spec: TableSpec,
        groups: I,
        block_rows: u64,
    ) -> Result<u64>
    where
        I: IntoIterator<Item = TableData>,
    {
        let declared = spec.rows;
        let spec = TableSpec { rows: 0, ..spec };
        let mut table = OpenTable::new(self, lane, name, spec, block_rows, Some(declared))?;
        for group in groups {
            table.push(self, group)?;
        }
        table.close(self)
    }

    /// Open the table `name` for streaming without its row count, on [`Lane::Arrays`] (see
    /// [`Self::open_table_on`]): an [`OpenTable`] that takes
    /// row-groups with [`OpenTable::push`] and decides at [`OpenTable::close`] how many leaves the
    /// table has, which one is the terminal leaf, and whether it is the one bare-named block
    /// `name` or the leaves `name/0000`, `name/0001`, … (ADR-0065 §7.3, #806). The sealed bytes
    /// are those of [`Self::push_table`] and [`crate::TableMultiBlockSink`] for the same rows.
    ///
    /// `spec` is the template of every leaf: its columns, `row_index` and any other field are
    /// copied into each leaf's spec, while `rows` and `partition` are set per leaf by the writer.
    /// So `spec.rows` must be `0` and `spec.partition` `None`.
    ///
    /// # Errors
    ///
    /// [`Error::Invalid`] when `spec.rows` isn't `0` or `spec.partition` isn't `None`, and the
    /// edition, name and dtype errors of [`Self::push_table`], all before anything is staged. As the row
    /// count is unknown, `name` must be valid both bare and as the first leaf `name/0000`, so a
    /// name within five bytes of the length limit is refused even for a table that would end as
    /// one block. A later leaf's longer name (`name/10000` on) is checked when that leaf is handed
    /// over, and refusing it abandons the table.
    pub fn open_table(&self, name: impl Into<String>, spec: TableSpec) -> Result<OpenTable> {
        self.open_table_in(Lane::Arrays, name.into(), spec, BLOCK_ROWS as u64)
    }

    /// [`Self::open_table`] on `lane` (usually [`Lane::table`] of the table's role): the leaves are
    /// committed in order among `lane`'s jobs, independently of the other lanes, so tables open on
    /// different lanes stream at once without one waiting for another, and are sealed in lane
    /// order (see [`Lane`]). The errors are those of `open_table`.
    pub fn open_table_on(
        &self,
        lane: Lane,
        name: impl Into<String>,
        spec: TableSpec,
    ) -> Result<OpenTable> {
        self.open_table_in(lane, name.into(), spec, BLOCK_ROWS as u64)
    }

    /// [`Self::open_table`] partitioned at `block_rows`: the test seam of
    /// [`Self::push_table_with_block_rows`].
    #[doc(hidden)]
    pub fn open_table_with_block_rows(
        &self,
        name: impl Into<String>,
        spec: TableSpec,
        block_rows: u64,
    ) -> Result<OpenTable> {
        self.open_table_in(Lane::Arrays, name.into(), spec, block_rows)
    }

    fn open_table_in(
        &self,
        lane: Lane,
        name: String,
        spec: TableSpec,
        block_rows: u64,
    ) -> Result<OpenTable> {
        if spec.rows != 0 {
            return Err(Error::Invalid(format!(
                "open_table '{name}': spec.rows must be 0, the row count is the rows pushed before close"
            )));
        }
        OpenTable::new(self, lane, name, spec, block_rows, None)
    }

    /// Hold every [`Self::push_table`] job before it reads its staged input, until the returned
    /// [`ReadHold`] is dropped: a test seam (ADR-0064 §8's barrier test, #714's heap model) that
    /// pins the `W` dispatched blocks on disk so the staged-input bound can be observed.
    #[doc(hidden)]
    pub fn hold_table_reads(&self) -> ReadHold {
        self.window.hold_reads(true);
        ReadHold(Arc::clone(&self.window))
    }

    /// Submit `job` on `lane`. `&self`, so the [`SequencedTable`]s of several lanes hand their
    /// leaves over from their own threads; a lane's jobs are submitted from one thread at a time
    /// (its `&mut` caller, or its sequenced input's release lock).
    fn submit(&self, lane: &Lane, job: Job, free_worker: bool) -> Result<()> {
        let tx = self
            .input
            .as_ref()
            .ok_or_else(|| Error::Container("stream writer already finished".into()))?;
        let lanes = || self.lanes.lock().unwrap_or_else(PoisonError::into_inner);
        let id = {
            let mut lanes = lanes();
            match lanes.iter().position(|(l, _)| l == lane) {
                Some(id) => id,
                None => {
                    lanes.push((lane.clone(), 0));
                    lanes.len() - 1
                }
            }
        };
        // Wait for the credit without the lock, so one lane's wait never holds another's submit.
        self.window.acquire(id, free_worker)?;
        // Take the seq and send under the lock: a lane's seqs reach the FIFO queue in order, so
        // its `next` is always queued (the window's deadlock freedom rests on it).
        let mut lanes = lanes();
        let seq = lanes[id].1;
        // The window admitted the job; the send never blocks, and fails only if every worker is gone.
        tx.send((Slot { lane: id, seq }, lane.clone(), job))
            .map_err(|_| self.window.refusal().unwrap_or_else(closed_error))?;
        lanes[id].1 += 1;
        Ok(())
    }

    /// Drain the pipeline, then compact + seal to `out`. Returns the sealed manifest (identical to a
    /// batch seal of the same blocks by lane, then in push order within a lane).
    ///
    /// # Errors
    ///
    /// Every thread is joined first, whatever the outcome. Then, in this order: the first refusal
    /// of a [`SequencedTable`] of this writer (ADR-0065 §7.2: it poisons the writer, so nothing is
    /// sealed; the [`Error::StreamedTable`] its own call returned); a panicked encode worker or
    /// committer; the pipeline's encode and commit errors;
    /// [`StreamedTableError::NotClosed`] for a sequenced table opened on this writer and not
    /// closed (still open, or dropped, so its rows aren't all committed); the seal's errors.
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
        // A refusal poisoned the writer: its pipeline was closed, so the committer's own error is
        // a consequence, and the refusal is the cause. (Should the pipeline die in the same
        // instant as a refusal, before its close reaches the gate, the refusal is reported in
        // place of the pipeline's error: a mislabelled cause, never a seal.)
        if let Some(e) = self.order.poisoned() {
            return Err(e);
        }
        if let Some(e) = first_error {
            return Err(e);
        }
        let session = committed?;
        if let Some(e) = self.order.unclosed() {
            return Err(e);
        }
        let stats = self.window.stats();
        Ok((session.seal(out)?, stats))
    }
}

/// The single committer: receives encoded blocks, commits each lane's to the session on that lane
/// in its `seq` order, independently of the other lanes (a reorder buffer keyed by `(lane, seq)`
/// holds completions that arrive early), releases each job's credit once it is committed, and
/// returns the session for sealing. The window bounds the reorder buffer to `max_in_flight`
/// entries.
fn commit_loop(
    mut session: WriteSession,
    out_rx: Receiver<Encoded>,
    window: &CreditWindow,
) -> Result<WriteSession> {
    let mut pending: BTreeMap<Slot, (Lane, Held, JobOutput)> = BTreeMap::new();
    // Per lane (by index), the `seq` committed next.
    let mut next: Vec<usize> = Vec::new();
    // Commit `lane`'s ready jobs: the data block then its additive companions (ADR-0028 §5
    // sidecars), in order, then give the job's credit back. A staged data block streams from its
    // file, which is deleted.
    let commit_ready = |session: &mut WriteSession,
                        pending: &mut BTreeMap<Slot, (Lane, Held, JobOutput)>,
                        next: &mut Vec<usize>,
                        lane: usize|
     -> Result<()> {
        if next.len() <= lane {
            next.resize(lane + 1, 0);
        }
        while let Some((name, held, out)) = pending.remove(&Slot {
            lane,
            seq: next[lane],
        }) {
            match out.data {
                DataBlock::InMemory((bref, payload)) => {
                    session.append_block_on(name.clone(), bref, &payload.bytes)?;
                }
                DataBlock::Staged((bref, staged)) => {
                    session.append_staged_on(name.clone(), bref, staged)?;
                }
            }
            for (cref, cpayload) in out.companions {
                session.append_block_on(name.clone(), cref, &cpayload.bytes)?;
            }
            window.release(lane, held);
            next[lane] += 1;
        }
        Ok(())
    };
    while let Ok((slot, lane, held, res)) = out_rx.recv() {
        pending.insert(slot, (lane, held, res?));
        commit_ready(&mut session, &mut pending, &mut next, slot.lane)?;
    }
    // Channel closed: all workers exited → every pushed block has arrived → drain each lane's
    // contiguous rest (nothing should be left: each arrival drained its own lane).
    let lanes: Vec<usize> = pending.keys().map(|s| s.lane).collect();
    for lane in lanes {
        commit_ready(&mut session, &mut pending, &mut next, lane)?;
    }
    // Every pushed job must have committed. Workers exit only after the queue drained or on a
    // panic (which `finish` reports first), so a shortfall here is a pipeline bug: fail loudly
    // rather than seal a truncated product.
    let pushed = window.stats().jobs_pushed;
    let committed: usize = next.iter().sum();
    if !pending.is_empty() || committed != pushed {
        return Err(Error::Container(format!(
            "stream pipeline lost jobs: {committed} of {pushed} committed, {} stranded",
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

    pub(super) const TS: &str = "2024-01-01T00:00:00Z";

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
    pub(super) mod listmode {
        use crate::table::{ColumnData, TableData, ROWS_PER_GROUP};
        use tessera_core::block::table::{Column, TableSpec};

        fn col(name: &str, dtype: &str) -> Column {
            Column {
                name: name.into(),
                dtype: dtype.into(),
                ..Default::default()
            }
        }

        pub(in crate::stream) fn spec(rows: u64) -> TableSpec {
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
                edition: None,
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
        pub(in crate::stream) fn rows(start: u64, n: usize) -> TableData {
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

    pub(super) const RPG: u64 = ROWS_PER_GROUP as u64;

    /// The listmode table in groups of `size` rows (≤ ROWS_PER_GROUP), off the canonical grid
    /// unless `size` is ROWS_PER_GROUP.
    fn groups_of(total: u64, size: u64) -> impl Iterator<Item = TableData> {
        (0..total)
            .step_by(size as usize)
            .map(move |start| listmode::rows(start, (total - start).min(size) as usize))
    }

    /// A manifest without what ADR-0064 §6 lets differ between two writers of the same rows:
    /// `producer`, `timestamp` and what follows from them.
    pub(super) fn canonical(m: &Manifest) -> serde_json::Value {
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

    // ---- ADR-0065 §7.3 / #806: open_table ----------------------------------------------------

    /// `open_table` of the listmode table at `block_rows` with `workers` workers: the leaves and
    /// the sealed manifest.
    fn open_table_product(
        dir: &Path,
        tag: &str,
        workers: usize,
        groups: impl Iterator<Item = TableData>,
    ) -> (u64, Manifest) {
        let stage = dir.join(tag);
        let ws = WriteSession::create(&stage, "recon", "p", "d", TS).unwrap();
        let mut sw = StreamWriter::new(ws, workers, 4);
        let mut table = sw
            .open_table_with_block_rows("events", listmode::spec(0), RPG)
            .unwrap();
        for group in groups {
            table.push(&mut sw, group).unwrap();
        }
        let leaves = table.close(&mut sw).unwrap();
        let m = sw.finish(&dir.join(format!("{tag}.tsra"))).unwrap();
        assert!(
            staged_files(&stage).is_empty(),
            "{tag}: staged file left behind"
        );
        (leaves, m)
    }

    /// A table whose row count is known only at close seals `TableMultiBlockSink`'s canonical
    /// manifest and `content_hash` at one leaf (empty, partial, exactly full), two leaves (the
    /// second full) and many leaves, for every worker count and on and off the row-group grid.
    #[test]
    fn open_table_seals_the_multi_block_sink_manifest_at_every_leaf_count() {
        let dir = tempfile::tempdir().unwrap();
        for (total, leaves) in [(0, 1), (99, 1), (RPG, 1), (2 * RPG, 2), (5 * RPG + 99, 6)] {
            let reference = sink_product(&dir.path().join(format!("sink{total}")), total, RPG);
            assert_eq!(reference.blocks.len() as u64, leaves, "sink at {total}");
            for (workers, size) in [(1, RPG), (2, 9999), (4, RPG)] {
                let tag = format!("r{total}w{workers}");
                let (n, m) = open_table_product(dir.path(), &tag, workers, groups_of(total, size));
                assert_eq!(n, leaves, "{tag}");
                assert_eq!(canonical(&m), canonical(&reference), "{tag}");
                assert_eq!(m.content_hash, reference.content_hash, "{tag}");
            }
        }
    }

    /// One leaf of lookahead: a full leaf is handed over once a row follows it, and before the
    /// next leaf's staging file is created, so only the last leaf waits for close. With all `W`
    /// workers held, the producer hands over `W` leaves, fills leaf `W`, and stalls on its
    /// hand-over at the next row with exactly `W + 1` leaves of input staged (a writer that
    /// created the next file first would stage `W + 2`; one that dispatched at close, as
    /// `TableMultiBlockSink` does, would hand over nothing and stage every leaf).
    #[test]
    fn open_table_hands_over_each_full_leaf_once_a_row_follows_it() {
        let leaves = 8u64;
        let leaf = |i: u64| listmode::rows(i * RPG, RPG as usize);
        for workers in [1usize, 2] {
            let w = workers as u64;
            let dir = tempfile::tempdir().unwrap();
            let stage = dir.path().join("stage");
            let ws = WriteSession::create(&stage, "recon", "p", "d", TS).unwrap();
            let mut sw = StreamWriter::new(ws, workers, 4);
            let window = Arc::clone(&sw.window);
            let hold = sw.hold_table_reads();
            let out = dir.path().join("p.tsra");
            let producer = thread::spawn(move || {
                let mut table = sw
                    .open_table_with_block_rows("events", listmode::spec(0), RPG)
                    .unwrap();
                table.push(&mut sw, leaf(0)).unwrap();
                assert_eq!(
                    sw.stats().jobs_pushed,
                    0,
                    "a full leaf may be the terminal one"
                );
                table.push(&mut sw, listmode::rows(RPG, 1)).unwrap();
                assert_eq!(
                    sw.stats().jobs_pushed,
                    1,
                    "a row after it makes it non-terminal"
                );
                table
                    .push(&mut sw, listmode::rows(RPG + 1, RPG as usize - 1))
                    .unwrap();
                for i in 2..leaves {
                    table.push(&mut sw, leaf(i)).unwrap(); // stalls at leaf W + 1, held
                }
                assert_eq!(table.rows(), leaves * RPG);
                assert_eq!(sw.stats().jobs_pushed as u64, leaves - 1);
                assert_eq!(table.close(&mut sw).unwrap(), leaves);
                sw.finish_with_stats(&out).unwrap()
            });
            // Until the producer stalls on the held workers (or fails, which the join reports).
            while window.lock().stats.producer_stalls == 0 && !producer.is_finished() {
                thread::yield_now();
            }
            let st = hold.stats();
            assert_eq!(st.jobs_pushed as u64, w, "w{workers}: leaves handed over");
            assert_eq!(st.peak_staged_input_blocks, workers + 1, "w{workers}");
            drop(hold);
            let (m, st) = producer.join().unwrap();
            assert_eq!(m.blocks.len() as u64, leaves);
            assert_eq!(st.peak_staged_input_blocks, workers + 1, "w{workers}");
            assert!(staged_files(&stage).is_empty(), "staged file left behind");
        }
    }

    /// Every way an open table can end without a close deletes every staging file: a refused
    /// group or a malformed one (at once, the handle still alive), a dropped handle, a close on
    /// another writer, and a dead pipeline. Handed-over leaves commit, the terminal one never does.
    #[test]
    fn an_abandoned_open_table_leaves_no_staging_file() {
        // Leaves of two row-groups: leaf 0 handed over, leaf 1 one row-group staged.
        let open = |dir: &Path, fail_first: bool| {
            let ws = WriteSession::create(&dir.join("stage"), "recon", "p", "d", TS).unwrap();
            let mut sw = StreamWriter::new(ws, 1, 4);
            if fail_first {
                sw.push(Box::new(|| Err(Error::Codec("boom".into()))))
                    .unwrap();
                spin_until(&sw.window, |st| st.closed);
            }
            let table = sw
                .open_table_with_block_rows("events", listmode::spec(0), 2 * RPG)
                .unwrap();
            (sw, table)
        };
        let fill = |sw: &mut StreamWriter, table: &mut OpenTable, groups: u64| {
            (0..groups).try_for_each(|i| table.push(sw, listmode::rows(i * RPG, RPG as usize)))
        };
        // Within leaf 0, nothing handed over: the refused group's leaf is deleted at once.
        let dir = tempfile::tempdir().unwrap();
        let stage = dir.path().join("stage");
        let (mut sw, mut table) = open(dir.path(), false);
        fill(&mut sw, &mut table, 1).unwrap();
        assert_eq!(staged_files(&stage).len(), 1);
        let err = table
            .push(&mut sw, listmode::rows(RPG, RPG as usize + 1))
            .unwrap_err();
        assert!(
            matches!(
                err,
                Error::StreamedTable {
                    reason: StreamedTableError::OversizedGroup { .. },
                    ..
                }
            ),
            "{err}"
        );
        assert!(staged_files(&stage).is_empty(), "refused: staged file left");
        let err = table.push(&mut sw, listmode::rows(0, 1)).unwrap_err();
        assert!(err.to_string().contains("abandoned"), "{err}");
        assert!(table.close(&mut sw).is_err());
        assert_eq!(
            sw.finish(&dir.path().join("p.tsra")).unwrap().blocks.len(),
            0
        );

        // Past leaf 0: the leaf being filled is deleted, and the seal refuses the partition
        // without its terminal leaf.
        for case in ["malformed", "dropped", "closed elsewhere"] {
            let dir = tempfile::tempdir().unwrap();
            let stage = dir.path().join("stage");
            let ws = WriteSession::create(&dir.path().join("other"), "recon", "p", "d", TS);
            let mut other = StreamWriter::new(ws.unwrap(), 1, 4);
            let (mut sw, mut table) = open(dir.path(), false);
            fill(&mut sw, &mut table, 3).unwrap();
            let alive = match case {
                "malformed" => {
                    let mut group = listmode::rows(3 * RPG, 1);
                    group.pop();
                    assert!(table.push(&mut sw, group).is_err());
                    Some(table)
                }
                "dropped" => {
                    drop(table);
                    None
                }
                _ => {
                    assert!(table.close(&mut other).is_err());
                    None
                }
            };
            let sealed = sw.finish(&dir.path().join("p.tsra"));
            assert!(
                matches!(sealed, Err(Error::LogicalTable { .. })),
                "{sealed:?}"
            );
            assert!(staged_files(&stage).is_empty(), "{case}: staged file left");
            drop(alive);
            other.finish(&dir.path().join("o.tsra")).unwrap();
        }

        // A dead pipeline refuses the hand-over of leaf 0; the table is abandoned with it.
        let dir = tempfile::tempdir().unwrap();
        let stage = dir.path().join("stage");
        let (mut sw, mut table) = open(dir.path(), true);
        fill(&mut sw, &mut table, 2).unwrap();
        let err = table.push(&mut sw, listmode::rows(2 * RPG, 1)).unwrap_err();
        assert!(err.to_string().contains("pipeline closed"), "{err}");
        assert!(
            staged_files(&stage).is_empty(),
            "dead pipeline: staged file left"
        );
        let err = sw.finish(&dir.path().join("p.tsra")).unwrap_err();
        assert!(err.to_string().contains("boom"), "{err}");
    }

    /// The handle is checked against the writer it was opened on, and its spec must leave `rows`
    /// to the writer.
    #[test]
    fn open_table_refuses_another_writer_and_a_declared_row_count() {
        let dir = tempfile::tempdir().unwrap();
        let ws = WriteSession::create(&dir.path().join("a"), "recon", "p", "d", TS).unwrap();
        let mut sw = StreamWriter::new(ws, 1, 4);
        let ws = WriteSession::create(&dir.path().join("b"), "recon", "p", "d", TS).unwrap();
        let mut other = StreamWriter::new(ws, 1, 4);
        let err = sw.open_table("events", listmode::spec(5)).err().unwrap();
        assert!(matches!(err, Error::Invalid(_)), "{err}");
        let mut table = sw.open_table("events", listmode::spec(0)).unwrap();
        let err = table.push(&mut other, listmode::rows(0, 3)).unwrap_err();
        assert!(err.to_string().contains("another StreamWriter"), "{err}");
        table.push(&mut sw, listmode::rows(0, 3)).unwrap();
        assert_eq!(table.close(&mut sw).unwrap(), 1);
        let m = sw.finish(&dir.path().join("p.tsra")).unwrap();
        assert_eq!(m.blocks[0].name.as_str(), "events");
        other.finish(&dir.path().join("o.tsra")).unwrap();
    }

    /// A seeded merge of per-lane sequences: each lane's items keep their order, the lanes'
    /// interleaving follows `seed` (xorshift64, so the test needs no RNG dependency).
    fn interleave<T>(seed: u64, mut lanes: Vec<Vec<T>>) -> Vec<(usize, T)> {
        let mut x = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
        lanes.iter_mut().for_each(|l| l.reverse());
        let mut out = Vec::new();
        loop {
            let live: Vec<usize> = (0..lanes.len()).filter(|&l| !lanes[l].is_empty()).collect();
            if live.is_empty() {
                return out;
            }
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let lane = live[(x % live.len() as u64) as usize];
            out.push((lane, lanes[lane].pop().expect("live")));
        }
    }

    /// #832: three lanes (two table lanes and the arrays lane) fed in randomised interleavings,
    /// worker counts and windows seal one product, byte for byte, and it is the batch
    /// `ProductBuilder` product of the same blocks in lane order: the table lanes by role name,
    /// then the arrays lane, each in push order — never in commit order.
    #[test]
    fn lanes_seal_the_batch_product_in_lane_order_whatever_the_interleaving() {
        let dir = tempfile::tempdir().unwrap();
        // Pushed lane by lane in this order, the arrays lane first, so commit order != lane order.
        let lanes = [Lane::Arrays, Lane::table("pethits"), Lane::table("events")];
        let named = |tag: &str, n: usize| -> Vec<_> {
            (0..n)
                .map(|i| ablock(&format!("{tag}{i:03}"), (i * 7) as i16))
                .collect()
        };
        let per_lane = vec![named("a", 3), named("p", 6), named("e", 5)];
        let mut batch = ProductBuilder::new("recon", "p", "d", TS);
        for lane in [2, 1, 0] {
            for (r, _, _) in &per_lane[lane] {
                batch.add_block_ref(r.clone());
            }
        }
        let batch = batch.seal().unwrap();
        let mut sealed_bytes: Option<Vec<u8>> = None;
        for (seed, workers, jobs) in [(1, 1, 2), (2, 4, 3), (3, 2, 16), (4, 4, 2), (5, 3, 5)] {
            let tag = format!("s{seed}w{workers}j{jobs}");
            let ws = WriteSession::create(&dir.path().join(&tag), "recon", "p", "d", TS).unwrap();
            let window = StreamWindow::new(jobs, u64::MAX);
            let mut sw = StreamWriter::with_window(ws, workers, window);
            for (lane, (r, spec, data)) in interleave(seed, per_lane.clone()) {
                sw.push_on(lanes[lane].clone(), array_job(r.name.as_str(), spec, data))
                    .unwrap();
            }
            let out = dir.path().join(format!("{tag}.tsra"));
            let m = sw.finish(&out).unwrap();
            assert_eq!(m.manifest_hash, batch.manifest_hash, "{tag}");
            assert_eq!(m.content_hash, batch.content_hash, "{tag}");
            let bytes = std::fs::read(&out).unwrap();
            match &sealed_bytes {
                None => sealed_bytes = Some(bytes),
                Some(first) => assert!(*first == bytes, "{tag}: sealed bytes differ"),
            }
        }
    }

    /// #832: two partitioned tables streamed at once on their own lanes, row-groups interleaved
    /// at random, seal the bytes of the same tables written one after the other in lane order
    /// on one lane (which `open_table_seals_the_multi_block_sink_manifest_at_every_leaf_count`
    /// holds to `TableMultiBlockSink`).
    #[test]
    fn open_tables_on_two_lanes_seal_the_sequential_product() {
        let dir = tempfile::tempdir().unwrap();
        let totals = [3 * RPG + 17, 2 * RPG];
        let sequential = {
            let ws = WriteSession::create(&dir.path().join("seq"), "recon", "p", "d", TS).unwrap();
            let mut sw = StreamWriter::new(ws, 2, 4);
            for (name, total) in
                ["events", "f", "pethits"]
                    .into_iter()
                    .zip([totals[0], 2 * RPG + 5, totals[1]])
            {
                let mut t = sw
                    .open_table_with_block_rows(name, listmode::spec(0), RPG)
                    .unwrap();
                for group in groups_of(total, RPG) {
                    t.push(&mut sw, group).unwrap();
                }
                t.close(&mut sw).unwrap();
            }
            let out = dir.path().join("seq.tsra");
            sw.finish(&out).unwrap();
            std::fs::read(&out).unwrap()
        };
        for (seed, workers) in [(7, 1), (8, 2), (9, 4)] {
            let tag = format!("s{seed}w{workers}");
            let ws = WriteSession::create(&dir.path().join(&tag), "recon", "p", "d", TS).unwrap();
            let mut sw = StreamWriter::new(ws, workers, 4);
            // Opened pethits first: lane order, not open or push order, decides the seal.
            let mut tables: Vec<OpenTable> = ["pethits", "events"]
                .into_iter()
                .map(|name| {
                    let lane = Lane::table(name);
                    sw.open_table_in(lane, name.into(), listmode::spec(0), RPG)
                        .unwrap()
                })
                .collect();
            let groups = vec![
                groups_of(totals[1], 9999).collect::<Vec<_>>(),
                groups_of(totals[0], 9999).collect(),
            ];
            for (t, group) in interleave(seed, groups) {
                tables[t].push(&mut sw, group).unwrap();
            }
            for t in tables {
                assert!(t.close(&mut sw).unwrap() > 1, "{tag}: partitioned");
            }
            // A table known up front, on a lane of its own, sealed between them in lane order.
            let total = 2 * RPG + 5;
            let spec = listmode::spec(total);
            sw.push_table_in(
                Lane::table("f"),
                "f".into(),
                spec,
                groups_of(total, RPG),
                RPG,
            )
            .unwrap();
            let out = dir.path().join(format!("{tag}.tsra"));
            sw.finish(&out).unwrap();
            assert!(std::fs::read(&out).unwrap() == sequential, "{tag}");
        }
    }

    /// #832: a product written on one lane, whichever it is, is the product of today's
    /// lane-less calls (the arrays lane) for the same jobs, byte for byte.
    #[test]
    fn a_single_lane_product_is_byte_identical_to_the_lane_less_product() {
        let dir = tempfile::tempdir().unwrap();
        let blocks = blocks(8);
        let (reference, reference_bytes) = unthrottled(dir.path(), &blocks);
        for lane in [Lane::table("events"), Lane::table("arrays")] {
            let tag = format!("{lane:?}");
            let ws = WriteSession::create(&dir.path().join(&tag), "recon", "p", "d", TS).unwrap();
            let mut sw = StreamWriter::new(ws, 3, 2);
            for (i, (_, spec, data)) in blocks.iter().enumerate() {
                let job = array_job(format!("b{i:03}"), spec.clone(), data.clone());
                sw.push_on(lane.clone(), job).unwrap();
            }
            let out = dir.path().join(format!("{tag}.tsra"));
            let m = sw.finish(&out).unwrap();
            assert_eq!(m.manifest_hash, reference.manifest_hash, "{tag}");
            assert!(std::fs::read(&out).unwrap() == reference_bytes, "{tag}");
        }
    }

    /// #832: the encode gate's no-stall rule is per lane. Lane `a`'s job 0 is held and its job 1
    /// holds the whole 1-byte budget, uncommitted behind it; lane `b`'s job 0 is the job the
    /// committer waits on in `b`, so it must encode and commit at once. A rule keyed to one
    /// lane's next would gate it behind lane `a`'s held job.
    #[test]
    fn each_lanes_next_job_encodes_and_commits_past_a_held_lane() {
        let dir = tempfile::tempdir().unwrap();
        let blocks = blocks(3);
        let ws = WriteSession::create(&dir.path().join("stage"), "recon", "p", "d", TS).unwrap();
        let mut sw = StreamWriter::with_window(ws, 2, StreamWindow::new(3, 1));
        let window = Arc::clone(&sw.window);
        let (gate0_tx, gate0_rx) = channel();
        let (gate1_tx, gate1_rx) = channel();
        let (done_tx, done_rx) = channel();
        let [(_, s0, d0), (_, s1, d1), (_, s2, d2)] = <[_; 3]>::try_from(blocks).unwrap();
        // Nothing has finished encoding yet (both workers hold lane a's jobs), so all three are
        // admitted without waiting.
        sw.push_on(Lane::table("a"), held_job(0, s0, d0, gate0_rx))
            .unwrap();
        sw.push_on(Lane::table("a"), held_job(1, s1, d1, gate1_rx))
            .unwrap();
        sw.push_on(Lane::table("b"), reporting_job(2, s2, d2, done_tx))
            .unwrap();
        gate1_tx.send(()).unwrap();
        // Lane a's job 1 now holds the budget; its worker takes lane b's job 0.
        let done = std::cell::Cell::new(false);
        spin_until(&window, |st| {
            done.set(done.get() || done_rx.try_recv().is_ok());
            (done.get() && st.stats.jobs_committed == 1) || st.gated_encodes == 1
        });
        assert_eq!(
            window.lock().gated_encodes,
            0,
            "lane b's next job was gated"
        );
        assert!(done.get());
        gate0_tx.send(()).unwrap();
        let m = sw.finish(&dir.path().join("p.tsra")).unwrap();
        let names: Vec<_> = m.blocks.iter().map(|b| b.name.to_string()).collect();
        assert_eq!(names, ["b000", "b001", "b002"]);
    }
}
