---
type: issue
state: open
created: 2026-11-10T10:46:07Z
updated: 2026-11-10T10:46:07Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/794
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-11T08:50:45.646Z
---

# [Issue 794]: [feat(io): live tap on StreamWriter commits + optional `tessera-live` aggregator crate](https://github.com/vig-os/tessera/issues/794)

Split-out of the DAQ discussion on #203. Two pieces, one API seam; writer stays the only
producer of truth.

## Context

A DAQ consumer (cyclotron control PC → NI cDAQ + OCR'd readouts → kit daemon) needs
**bounded, lossy** access to batches as `StreamWriter` commits them — strip-chart,
rolling histogram, alarm — without blocking the writer. The engine has `with_live_index`
(ADR-0028 §5) but no subscription surface; watchers must tail fragments on disk and
recompute stats the engine already has.

## Proposal

### A. `StreamWriter::tap(TapPolicy)` in `tessera-io`

Bounded, lossy subscription driven by the ordered committer; one event per committed
block / row-group:

```rust
pub enum DropPolicy { LatestWins, DropOldest }

pub struct TapPolicy {
    pub capacity: usize,
    pub drop: DropPolicy,
    pub include_stats: bool,   // requires with_live_index; opt-in, zero-cost off
}

pub struct TapEvent {
    pub seq: u64,
    pub block_ref: BlockRef,
    pub live_root: Option<String>,
    pub stats: Option<ChunkStats>,
    pub committed_at: Instant,
}

impl StreamWriter { pub fn tap(&self, policy: TapPolicy) -> Tap; }
impl Iterator for Tap { type Item = Result<TapEvent>; } // sync, ADR-0034 default
```

Events are emitted **after** the journal fsync, in commit order — only durable bytes get
announced. Drops are counted per-tap (`Arc<AtomicU64>` observer pattern from `cloud.rs`).
Multiple taps per writer allowed.

### B. Optional `tessera-live` crate (workspace member)

Fed by A on its own thread. Pure-Rust, no I/O, no transport. Reuses per-chunk stats from
the live index (#633):

- Decimation (min/max per pixel-column, every-Nth).
- Rolling histograms, fixed bins, bounded memory.
- Quantile sketches (t-digest / DDSketch).
- Windowed stats (count/mean/std/min/max) for alarms.
- Timer-driven emit at **10–60 Hz**.

Non-goal: **transport** (WebSocket / iroh / kit live lane). `tessera-live` hands rows;
the daemon owns the wire.

## Blockers

- **#633** — `with_live_index` folds identity stats where the batch path errors. Must
  land before `tessera-live` treats live stats as truth; until then `stats: None` is the
  only safe tap mode. #677 credit window is already in.

## Acceptance criteria

- [ ] `StreamWriter::tap(policy)` returns a `Tap`; multiple taps per writer allowed.
- [ ] A slow / parked tap never blocks `push` / `push_staged` / `push_table`. Regression
      test: a tap that `park()`s; assert `push` throughput unchanged, `peak_in_flight_jobs`
      ≤ `J`, per-tap drop counter > 0.
- [ ] Events emitted **after** the journal fsync, in commit order; `seq` matches the
      committed journal line.
- [ ] `include_stats = true` requires `with_live_index` and returns the same `ChunkStats`
      the sealed `.cidx` sidecar carries (reconciles by construction).
- [ ] Tap closes cleanly on `finish` / `seal` / writer drop / writer error; the iterator
      returns the terminal error, then `None`.
- [ ] No change to sealed bytes: `content_hash` identical with and without a tap attached,
      across worker counts and window sizes (property test).
- [ ] `tessera-live` lands in a follow-up PR with a histogram, a t-digest, and an alarm
      example, each < 300 LoC, no tokio.
- [ ] CLI `tessera watch <stage-dir>` deferred to a later issue; this one ships the
      library seam only.

## Out of scope

Transport / wire format (`tessera` never owns a socket). Async tap API (deferred; sync
per ADR-0034). A sealed "truncated" manifest state (tracked in the #203 open questions).

