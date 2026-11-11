---
type: issue
state: open
created: 2026-06-25T11:39:44Z
updated: 2026-11-10T10:46:08Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/203
comments: 4
labels: priority:high, epic, area:io
assignees: none
milestone: backlog / research
projects: none
parent: none
children: none
synced: 2026-11-11T08:50:47.958Z
---

# [Issue 203]: [[P3][epic] tessera-io streaming write engine](https://github.com/vig-os/tessera/issues/203)

**Phase P3 — epic.** ROADMAP: tessera/docs/ROADMAP.md

- S5 zarrs array backend (64³ cubic + pcodec).
- S17 streaming write: bounded RAM ring → rayon encode pool → durable fragment commits + incremental Merkle + registry watermark (M/E/C); spill on burst; **never encode on the DAQ hot path**.
- S3 chunk-Merkle integrity tree (hash-on-write).
- container writer; observability (`tracing` on watermarks).

**Done-gate:** acq→sealed roundtrip; crash-recovery resumes to last committed watermark; §D perf-SLA met. (FEATURE-MATRIX §E.)
---

# [Comment #1]() by [gerchowl]()

_Posted on June 25, 2026 at 10:19 PM_

**S5 array backend landed** (commits `4d9c86c`, `f75389f`):

- `tessera-io::array` — real Zarr v3 + **pcodec** encode/decode for the 8 pcodec-native dtypes (i16/i32/i64/u16/u32/u64/f32/f64), plus `decode_subset` (ROI / orthogonal slab — only intersecting chunks).
- Block payload = one **deterministic** serialized Zarr store (sorted-key framing); digest over the encoded bytes via `add_block_ref`.
- **Bit-exact** incl float NaN/±inf/−0.0/denormal (the S13 clinical gate, now in Rust); **writer-deterministic** (encode-twice → byte-identical).
- Conformance corpus array fixtures now carry real payloads; goldens regenerated; repack byte-identical. ADR-0023 + SPEC §5a written.
- `nix flake check` green (39 tests). `ArraySpec` default codec → pcodec.

Remaining under this epic: **Vortex table backend** (next), then the streaming write engine (fragment-append, hash-on-write incremental Merkle, crash-recovery).

---

# [Comment #2]() by [gerchowl]()

_Posted on June 25, 2026 at 11:26 PM_

**Vortex table backend landed** (commits `b2041e4`, `7fdd538`):

- `tessera-io::table` — real Vortex columnar encode/decode for 10 numpy dtypes (`i1/i2/i4/i8`, `u1/u2/u4/u8`, `f4/f8`); `CurrentThreadRuntime`+session (no tokio); digest over encoded bytes via `add_block_ref`.
- **Conformance corpus is now fully real** — both table fixtures (`listmode_events`, `multiblock_study` ROI) carry real Vortex payloads, no spec-JSON stubs. `listmode` rows shrunk 1e6→4096 (a conformance fixture tests determinism, not scale).
- **Load-bearing determinism fix**: Vortex's default compressor chooses **ALP** for float columns, and ALP's float-exponent search is build-profile-sensitive (opt-level/FMA) → different bytes across builds → broke content-addressing (caught by the hermetic conformance gate). Fixed by excluding ALP/ALPRD schemes (`BtrBlocksCompressorBuilder::exclude_schemes`); floats fall back to deterministic flat/Pco. Verified dev==release==hermetic. (ADR-0024, SPEC §5b.)
- `flake.nix` provides libclang+libstdc++ for the Vortex hermetic build. `nix flake check` green (43 tests, cross-environment conformance).

Remaining under this epic: the streaming write engine (fragment-append, hash-on-write incremental Merkle, crash-recovery to watermark).

---

# [Comment #3]() by [gerchowl]()

_Posted on June 26, 2026 at 12:11 PM_

**Advanced in f5cabff** — the bounded-memory, parallel-encode streaming core landed.

`StreamWriter` (tessera-io::stream): push → bounded sync_channel (backpressure = RAM cap) → N encode worker threads → ordered committer (reorder by seq → `append_block` in push order) → `WriteSession` → `finish` seals. std-only, no new deps.

- **Byte-identical to batch** (commits in push order → same Merkle/seal) → does NOT touch the v1.0 frozen format or determinism gate.
- **Bounded RAM** under burst (full channel blocks push; peak in-flight ≤ ~capacity blocks).
- **Bench:** 6.2× throughput vs synchronous encode-on-hot-path (saturates ~4 workers), cap=2 holds RAM flat. (SPIKE-RESULTS #203, `examples/stream_write`.)

Done here: multi-block streaming + live block-level hash-on-write Merkle (FEATURE-MATRIX §E rows flipped to ✓).

**Remaining (kept open):** sub-block chunked compaction for a single >RAM block — the only byte-changing part, where >RAM file ingest and live *per-chunk* Merkle converge. Tracked in **ADR-0026**; gated on the cross-env determinism re-validation (#198) since it regenerates table goldens.

---

# [Comment #4]() by [gerchowl]()

_Posted on November 10, 2026 at 10:46 AM_

## Design comment — tessera owns the DAQ write path (+ a live tap, deferring aggregation)

Context for #203. A DAQ consumer is landing: a cyclotron control PC streaming **NI cDAQ**
samples + OCR'd device readouts into tessera, with nerdmachines/kit DAQ daemon as the
reader. Two record kinds:

- **Per-run products** — one `.tsra` per acquisition; receipt before, seal after.
- **A 24/7 machine log** — one rolling writer, sealed **hourly and/or daily**.

The engine already exists (`StreamWriter` + `WriteSession`: #677 credit window, ADR-0028 §5
fused pass, `with_live_index`, ADR-0026 revision "ingest is a throttled producer into the
one `WriteSession`"). What is missing is a *writer surface* with provenance fixed at open
and a bounded live view of commits. Three asks, each reusing what exists:

### 1. The writer, from the DAQ side

`tessera_io::stream::Writer::open(dir, schema, receipt)` as a thin DAQ-facing wrapper over
`WriteSession::create` + `StreamWriter::with_window`:

- **`receipt` is the existing `Producer` + `Generation` + `Source` edges** (ADR-0058 /
  ADR-0048), not a new field. They go through `WriteSession::with_producer` /
  `with_generation` / `add_source` **before the first append**, so the manifest is
  identity-stamped at open — who/what, input+config hashes, tool+commit.
- `append(RecordBatch)` → `push_table` / `push_staged`, pinned on
  `tessera_ingest::arrow_table::canonicalise_batch` for Arrow→`ColumnData` so hot-path
  code is encode-free (ADR-0026 revision, FEATURE-MATRIX §E).
- **`seal()`** uses the existing packer; `.tsra` layout and the Vortex pin (0.75,
  `608d8a1`) are unchanged (ADR-0022, ADR-0024). **`Writer::recover(dir)`** rides
  `WriteSession::recover` (torn-tail drop, fragment quarantine, journal rewrite) and lets
  the caller `resume` or `seal_truncated()` — but today `seal` just reseals what the
  journal names; representing **truncated** as a sealed state is an open question.
- Seal and fragment **by time OR size** (whichever first). Size uses the existing
  `BLOCK_ROWS = 2²²` / `ROWS_PER_GROUP = 2¹⁶` boundary; time is new config, no format
  change.

### 2. The one real format question: Arrow-IPC WAL tail vs fragment-as-WAL

The proposal says "durable **Arrow-IPC** stream WAL tail; Arrow tools can read the unsealed
tail directly." Today the WAL is the **journal (`journal.jsonl`) + the Vortex fragments
under `blocks/`**; the atomic commit point is the journal fsync, not a second log. That is
a load-bearing decision — one encoder, one commit point (ADR-0026 revision). An Arrow-IPC
tail *in addition* to the fragments would be a second write path. The right framing:

- **Keep fragment-as-WAL** for the sealed bytes (ADR-0054's "no non-canonical codec in the
  identity tier" argument applies to Arrow-IPC-as-WAL too).
- **Optional `--arrow-ipc-tail`** mirrors the latest in-progress row-groups as an
  **unsealed `aux/` member** (ADR-0042) for off-tessera tools to tail. A *derived cache*
  of the fragments, never the commit point, not in `content_hash`.

`fsync` policy per batch / per N ms is a `WriteSession` config knob; "crash loses ≤ one
flush interval" is already the journal's contract (#695).

### 3. The tap — in core-lib, aggregation in `tessera-live` (separate issue)

`StreamWriter::tap(TapPolicy)` returns a bounded, **lossy** subscription driven by the
ordered committer (one notification per committed block / row-group, carrying `BlockRef`,
`live_root()` and the `ChunkStats` from `with_live_index`). Writer never blocks on
viewers; `latest-wins` / `drop-oldest`; counters on drops (the `Arc<AtomicU64>` observer
pattern in `cloud.rs`). **Aggregators (decimation, rolling histograms, t-digest/DDSketch,
windowed mean/std for alarms, 10–60 Hz emit) live in an optional `tessera-live` crate**
fed by the tap on its own thread — new issue #794.

**Prerequisite**: aggregators that read per-chunk stats reuse the live-index fold
(`with_live_index`). **#633 is open — live-fold currently returns identity stats where the
batch path errors**; it must land before aggregators treat the live aggregate as truth.

### 4. Non-goal (explicit)

Transport (WebSocket / iroh / kit live lane) stays **out of tessera**. The tap hands
`BlockRef`s + live stats; the DAQ daemon owns the wire.

### Open questions

- **WAL format**: confirm fragment-as-WAL + optional `aux/` Arrow-IPC mirror; reject a
  second sealed log.
- **fsync defaults**: per-batch vs per-N-ms; default N for the machine log vs per-run.
- **`truncated` in the manifest**: a `Generation.outcome` enum (ADR-0058 shape), a sealed
  flag (corpus regen), or unsealed `aux/provenance.json` only?
- **Tap API**: sync `Iterator<Result<Event>>` vs an async mirror behind a feature. Default
  sync, per ADR-0034.
- **Time columns + seal cadence**: cadence is wall-clock (ADR-0046 UTC for stamps);
  listmode times are integer ticks + `time_ticks(period_s, start_s).with_epoch("…")`
  (ADR-0046 §2) — pin the epoch per record kind.


