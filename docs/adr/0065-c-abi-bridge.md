# ADR-0065 — C ABI bridge: C++ producers write through tessera's writer (`tessera-capi`)

**Status:** Accepted (2026-11-13), by the owner, as written; proposed 2026-11-11. The acceptance settles the ADR's
two owner decision points: NaN payloads (hazard H4) are canonicalised **at the bridge**, which calls
`tessera-ingest`'s `canonicalise_floats` and seals the `IngestTransform` exactly as ingest does (§1, §9); and the
stall deadline is **mandatory**, with no "no stall deadline" opt-in (§7.2). Toward epic
[#799](https://github.com/vig-os/tessera/issues/799); this is the design of
[#800](https://github.com/vig-os/tessera/issues/800), and it fixes how the bridge expresses
[#801](https://github.com/vig-os/tessera/issues/801) (a registered schema per product type) and
[#802](https://github.com/vig-os/tessera/issues/802) (sealed bytes independent of producer-thread scheduling).
[#803](https://github.com/vig-os/tessera/issues/803) (porting the readers) is out of scope. Code references are at
`origin/dev` `6a30780`, under `tessera/crates/`, unless they say otherwise. MorePET references are pinned:
gPET-sim `7fa1ad38`, gPET-sim2listmode `3d6d5660`, geddf-analysis `d6cd7857`, g4h5 `a548054f`. The owner's two consumer
surveys are still running. Points they would settle are marked **pending surveys**, and are not guessed here.
**Owner steer (2026-11-11), folded in:** "a C interface that takes Arrow batches from many threads and streams them
to StreamWriter". The architect's refinement of it is the design this ADR decides: the Arrow C Data Interface as the
wire; one session per product; a thread-safe `push(…, seq, ArrowArray*, ArrowSchema*)`; commit in `seq` order behind a
credit window that back-pressures a thread running too far ahead; and the schema declared at open and checked on
every batch. One refinement goes beyond the steer's sketch. The surveyed products have several tables
(`gpet-sim:g4ring-output`: `events` and `pethits`; `geddf:coincidences`: seven tables and an array), so a session
opens one **table lane** per table role, each with its own `seq` stream, and seals them together. `push` therefore
takes the lane, not the session (§3, §7.3).
**Reconciled with the merged #801 gap analysis** (`docs/spikes/801-consumer-gap-analysis.md`, on `dev` at `ecc55be`)
and its draft schemas (`docs/spikes/801-consumer-schemas/`). The schema shape (§8), the sequence key and block size
(§7.2) and the plan (§15) follow it. Where this ADR differs, it says so and says which wins.

## Context

### What the C++ producers do today

None of the MorePET producers mentions tessera, Arrow or Parquet. Each one writes HDF5 compound datasets with a
`units` attribute per member and a `schema_name` / `schema_version` root attribute.

- **gPET-sim** (the Geant4 app under `g4ring/`) is C++17 (`g4ring/CMakeLists.txt:10`). Its CMake build finds Boost,
  nlohmann_json, HDF5 (CXX), ZLIB and Geant4 11.3 (`:65-117`), and it vendors the `g4h5` HDF5 writer as a
  subdirectory (`:87`). It runs `G4TaskRunManager` (`g4ring/src/EventAction.cpp:15`). The manager hands each task a
  block of consecutive Geant4 event ids (`EventAction.cpp:62-63`). Output is **one HDF5 file per worker thread**, an
  `std::array<std::unique_ptr<g4h5::H5Writer>, kMaxThreads>` (`g4ring/include/SimulationManager.h:294`). The files are
  created under a mutex because HDF5 initialisation is not thread-safe (`SimulationManager.h:283`,
  `g4ring/src/SimulationManager.cpp:269-277`). Shards are never merged.
- **The write path:** `EventAction::EndOfEventAction` (`EventAction.cpp:189`) calls `FlushPETHits()` and then
  `SaveEvent()` (`:225`). `FlushPETHits` appends one row per aggregated hit to the thread's writer
  (`SimulationManager.cpp:524-556`, `writer->addTyped("pethits", pethit)`). It iterates a
  `std::map<std::pair<unsigned, unsigned>, PETHit>` (`SimulationManager.h:310`), so the order of the rows within
  an event is deterministic. `SaveEvent` appends one `events` row per kept event (`SimulationManager.h:657-678`).
- **The record types:** `Event` (`event_id u64, n_pethits u16, source_{x,y,z}_mm f32, annih_{x,y,z}_mm f32`,
  `SimulationManager.h:103-129`) and `PETHit` (`event_id u64, hit_type u16, crystal_id u32, edep_keV f32, time_ps f64,
  nHits u16, n_compton u8, n_rayleigh u8, pos_* f32, first_* f32`, `:141-175`). The authoritative schema is
  `g4ring/schema/g4ring-output.yaml` (`gpet-sim/g4ring-output 3.0`). With `saveAllEvents = 0`, only events with hits get
  an `events` row (`g4ring-output.yaml:17`).
- **The order key** is `event_id = Geant4 eventID + eventIdOffset` (`SimulationManager.h:701-703`). Within one run it
  is dense: every event id in `[offset, offset + numberOfEvents)` passes through `EndOfEventAction`, whether or not
  the event is saved.
- **gPET-sim2listmode** is a C++17 batch binary. It reads the shard set, writes `/listmode_singles` (one packed
  compound dataset, 64 B per row, ordered by non-decreasing `absolute_time_ps`), and serialises every HDF5 call
  behind one global mutex (`h5par::Hdf5Mutex()`).
- **The Stage 2 pairwise path** (`geddf_analysis/pairwise.py`) is 100 % Python (h5py and numpy). It writes a JSON
  record (`geddf-analysis/pairwise-record 3.0`), not a table. Its coincidence tables
  (`geddf_analysis/schema/coincidences.yaml`) are written by Python too. **So Stage 2 is a `tessera-py` consumer,
  not a C ABI one** (§13.3).

Two consequences shape the design. First, per-thread shards are not reproducible products. Which events land in
which shard depends on how the task manager scheduled the blocks, so the same seed gives different shard bytes on
different runs. The bridge has to fix this by construction (#802), not copy it. Second, every producer already has
its records aggregated in C++ structs at end of event, and it has a dense order key. The ABI can therefore take
whole batches with a key, not single rows.

### What the writer offers today

- **`WriteSession`** (`tessera-io/src/write.rs:258`) is the crash-tolerant journal under every streamed product.
  `create` (`:594`) and the header setters `with_study`, `with_field`, `with_extra`, `add_source`, `with_producer` and
  `with_generation` (`:638-682`) persist a header. Blocks are appended with `append_block`, `append_block_from` and
  `append_staged` (`:715-764`), and `seal` (`:1065`) builds the manifest through the same `ProductBuilder` a batch
  write uses, then streams the fragments into the zip (`pack_streaming`). The seal does **not** validate the
  manifest against a product schema. `ProductBuilder::seal` (`tessera-core/src/product.rs:175-215`) only embeds the
  built-in registry's schema for a known product (`:192-195`). Validation runs in the ingest engine, after the seal
  (`tessera-ingest/src/engine.rs:204-210`).
- **`StreamWriter`** (`tessera-io/src/stream.rs:730`) is a bounded-memory pipeline: `W` encode workers and one
  committer over a credit window. "Blocks are committed in call order" (`:842`), so the manifest order is the push
  order. `push_table` (`:927`) partitions a table above `BLOCK_ROWS` into the ADR-0064 leaves as it reads, but it
  needs `spec.rows` before the first row (`:861-863`). `finish` (`:1106`) joins every thread and seals.
- **`TableMultiBlockSink`** (`tessera-io/src/accumulate.rs:416`) takes arbitrary batches without a row count and
  stages durable row-group fragments. It dispatches every leaf at `finish` (`:586`), because the
  small-stays-single rule needs the total row count before it can name the first block (`:411-414`). It holds a
  `&'a mut StreamWriter` for its whole life (`:426`), so a session can stream only one table at a time through it.
  `TableStreamWriter` (`:231`) is its single-block sibling.
- **The byte-level invariants the bridge rides on:** the rows are re-chunked to the `ROWS_PER_GROUP` grid, so the
  bytes don't depend on batch boundaries (`accumulate.rs:12-15`). The seal doesn't depend on the worker count
  (`accumulate.rs:1029`, `stream.rs:1851`, `stream.rs:2113`). Masked slots are normalised on encode (`table.rs:115-121`).
- **The canonicalisation boundary** (ADR-0056 §2/§5, ADR-0057 §1) is `tessera-ingest`. `arrow_table::canonicalise_batches`
  (`tessera-ingest/src/arrow_table.rs:119`) maps an Arrow `RecordBatch` to `ColumnData`.
  `canonical::canonicalise_floats` (`canonical.rs:251`, hazard H4, NaN payloads) and `normalise_null_slots` (`:289`,
  H5) are recorded as `IngestTransform`s (`tessera-core/src/provenance.rs:150`) in the sealed `ingest_transform`
  list (`tessera-core/src/manifest.rs:86`). `tessera-ingest` links libhdf5 unconditionally today
  (`hdf5-metno` and `hdf5-metno-sys` at `tessera-ingest/Cargo.toml:19-22`); gating it is #454. `tessera-io` has no HDF5 in its graph
  (`tessera-io/Cargo.toml`, `[dependencies]`).
- **Schemas:** `ProductSchema` (`tessera-core/src/schema.rs:228`) declares metadata `fields` and `blocks`, where a
  `BlockRequirement` is only `{role, kind, min_count, description}` (`:209-217`). There are **no column declarations**, so a
  schema can't say what a table's columns are. `SchemaRegistry::builtin()` (`:349`) is compiled in. The embedded
  copy in a sealed manifest is authoritative for readers (`validate_manifest`, `:333`).
- **The aux members** (ADR-0042) are written after the seal: `stamp_ingest_provenance` (`tessera-io/src/provenance.rs:86`)
  and `add_aux_members` (`tessera-io/src/container.rs:728`). The ADR-0047 identity envelope is moving into the
  HDF5-free `tessera-identity` crate (PR #786, open: `encrypt_identity`, `AUX_IDENTITY_NAME`).
- **The FFI precedents:** `tessera-py` (`tessera-py/src/lib.rs`) maps every error to one exception through
  `err` (`:37`). Its `Builder` (`:281`) is a batch writer that keeps every encoded payload in RAM (`:283`).
  `tessera-wasm` (`tessera-wasm/src/lib.rs:21`) returns `false` instead of throwing. Neither of them streams, so the
  C ABI is the first streaming FFI surface.

## Decision

1. **A new crate, `tessera-capi`** (cdylib and staticlib, library name `tessera`, symbol prefix `tsra_`), wraps
   `StreamWriter`/`WriteSession` behind opaque handles. It contains no format logic: every byte it seals comes from
   `tessera-io`, through the same calls a Rust producer makes (§1).
2. **Tables cross the boundary as the Arrow C Data Interface.** Each push is one `struct ArrowArray` (a struct
   array) with its borrowed `struct ArrowSchema`. The bridge checks the batch's schema against the declaration
   before it imports anything, imports zero-copy with `arrow::ffi`, and converts through `tessera-ingest`'s arrow
   lane. The bridge canonicalises NaN payloads (hazard H4) with `canonicalise_floats` and seals the transform, as
   ingest does (owner ruling 2026-11-13, §1). Vortex never appears in the C API, so the
   storage engine can change without breaking a C++ caller. **Arrays (volumes) cross as a plain C buffer** plus a
   numpy dtype code and a shape (§2).
3. **One session per product, with one table lane per table role, each fed by `tsra_push(table, seq, batch,
   schema)` from any number of threads at once.**
   - Every batch carries a dense `uint64` sequence number, its lane's own.
   - The bridge streams into `StreamWriter`: each lane releases its batches in `seq` order into its table's leaves.
     The leaves encode in parallel and commit in their lane's order, and the seal orders the lanes by role. So no
     table waits for another.
   - A credit window back-pressures (blocks) a thread that runs too far ahead.
   - A duplicate, a gap at the seal, a `seq` past a hard horizon and a lost `seq` (through `tsra_session_abort` or a
     finite stall deadline) are typed errors, never hangs. The stall deadline is mandatory: no value disables it
     (owner ruling 2026-11-13, §7.2).
   - Many products means many independent sessions.

   This is how the bridge expresses #802 (§7).
4. **The bridge refuses to write without a product schema,** declared at session open (from the registry, #801),
   in the #801 drafts' shape. It declares tables keyed by role, each with its columns, `sequence_key`, `keys` and
   `references`, plus the arrays keyed by role. The bridge validates the open, every
   batch (names, types, nullability) and the seal against it, with an error that names the column and the `seq`,
   and it embeds the schema in the seal. This is how the bridge expresses #801 (§8).
5. **The error model:** an `int32_t` status code from every call, plus a thread-local last-error string.
   `catch_unwind` guards every entry point. A caught panic poisons the handle (§5).
6. **Every call is safe from any thread.** The sealed bytes depend only on the `seq → batch` mapping, never on
   which thread calls or when (§6, §7).
7. **The ABI has its own version, separate from the crate's.** The soname is `libtessera.so.<ABI major>`. Option
   structs carry their size. The header is generated by cbindgen and checked for drift. An ABI diff gate runs
   against the last release (§10).
8. **Packaging:** pkg-config and CMake config files, a nix package with a CMake consumer check, and cargo-dist
   library archives. The bridge links no HDF5, so the `static-hdf5` question doesn't arise for it (§11).

## §1 — Where the bridge sits

```text
C/C++ producer ── tessera.h (cbindgen) ──▶ tessera-capi   handles, errors, ordering (§7), schema checks (§8)
                                              │
                    ┌─────────────────────────┼───────────────────────────┐
                    ▼                         ▼                           ▼
   tessera-ingest [arrow lane only]      tessera-io                 tessera-identity (PR #786)
   ArrowArray → ColumnData, H4           StreamWriter, WriteSession, seal, aux/
                    └──────────────▶ tessera-core (schema, manifest, ids) ◀───────┘
```

`tessera-capi` depends on `tessera-core`, `tessera-io`, `tessera-ingest` with `default-features = false,
features = ["arrow"]`, `tessera-identity`, and `arrow-array` (for `arrow_array::ffi`). It is a library only. It ships
no binary, and it is not a dependency of the CLI, the wheel or the wasm crate.

**Why `tessera-ingest` and not a second Arrow map.** The bridge has the same job as the ingest table lane: a foreign
type system becomes `ColumnData` upstream of the waist (ADR-0057 §1). A second map in the bridge would be a second
place where a NaN payload or a masked slot could reach `content_hash`. Hazard H4 is real for this consumer: x86-64
and AArch64 produce NaNs of opposite sign for the same `0.0/0.0`, the same one-bit divergence that #472 traced. As the
owner ruled on 2026-11-13 (below), the bridge calls `canonicalise_floats` and records its transform exactly as
ingest does. It does not call `normalise_null_slots` (H5): the encoder already normalises masked slots
(`tessera-io/src/table.rs:115-121`), so calling it would only add a receipt that a native write of the same rows
doesn't carry. ADR-0057 §2 pre-authorised a `tessera-arrow` crate for "an obvious third-party audience ('I have a
`RecordBatch`, I want a `.tsra`')", and named trigger 2 (a consumer outside `tessera-cli`) as the one to watch.
The bridge is that consumer. It doesn't need the crate yet, though, because ADR-0057 §3's feature gating delivers
the same property once #454 makes `hdf5` and `dicom` optional features. **#454 is therefore a prerequisite** (§15).
A `capi-deps` flake check, like PR #786's `identity-deps`, asserts that `cargo tree -p tessera-capi` contains no
`hdf5*` crate and no `dicom*` crate, and that it does contain `arrow-array`, so an empty tree can't pass. If the
check can't be made green after #454 lands, the arrow lane moves into `tessera-arrow` as ADR-0057 §2 allows. That
is not this ADR's decision to make now. The prototype measured the fallback (spike §7.1): the clean-lane type map
alone is about 40 lines and needs no `tessera-ingest`, because a clean-lane value crosses bit for bit. What the
bridge needs from ingest is the canonicalisation, not the map, so a bridge with its own map would still have to
duplicate H4. That's why #454 stays first, and the 40-line map is the fallback only if #454 slips past #813.

**H4 lives in the bridge (owner ruling, 2026-11-13).** The ADR recommended the bridge, and the owner's acceptance
made that the decision: the bridge calls `tessera-ingest`'s `canonicalise_floats` and records the `IngestTransform`
in the seal exactly as ingest does (§9). The prototype showed the consequence (spike §7.2,
`nan_payloads_are_sealed_bit_for_bit`): the native writer seals NaN payloads bit for bit (S13), so a C++ producer
whose NaNs the bridge canonicalises and a Rust producer that pushes the same rows through `StreamWriter` seal
**different products whenever a NaN is present**. The options that were weighed:

- **In the bridge** (decided). The bridge is a door into the format, like ingest, and ADR-0056 §5 put H4 at the
  doors. A simulation's NaNs are arithmetic accidents whose sign depends on the CPU that ran it, so without this the
  same seed seals different bytes on x86-64 and AArch64. The transform is sealed, so the two products differ by a
  recorded receipt, not silently. Bridge-written equals native-written for every NaN-free table, and #813's
  equivalence test is stated that way.
- **In `tessera-io`'s encoder, for every producer.** Every producer then agrees, but it changes what the waist
  seals, which is a format decision (a corpus event for float columns that hold non-canonical NaNs, and S13's
  bit-exactness narrows). It belongs in its own ADR, not in this one. Rejected for this ADR.
- **Nowhere.** The producer owns its NaN bits. That's consistent, but it gives up cross-arch reproducibility for
  any producer that computes a NaN. Rejected.

The choice was ABI-neutral: no function or code differs between the three, only whether the bridge calls
`canonicalise_floats`. It does.

**Feature snapshots.** `arrow-array`'s `ffi` feature is already on in today's graph, through `vortex-array`
(`tessera/tests/feature-snapshots/arrow-array.txt:2`), so importing `arrow_array::ffi` turns no new feature on in
the shared arrow tree. A new crate with a direct `arrow-array` edge still changes the Gate B snapshot. The PR
regenerates it and shows that every added line is a `tessera-capi` edge.

## §2 — What crosses the boundary

### §2.1 Tables: the Arrow C Data Interface

Each table role's columns are declared by the product schema given at `tsra_session_open` (§8). The producer builds
the matching Arrow schema for each lane: a struct type whose children are the columns, in the declared order. Each push takes one
`struct ArrowArray`, a struct array of that type carrying the rows of one sequence number, **together with the
borrowed `struct ArrowSchema` that describes it**, as the prototype does.
The schema is required at every push because an `ArrowArray` carries no type. The importer works out every
buffer's length from the type it is given, not from the producer's allocation (arrow-array 58.4.1 `src/ffi.rs:450`,
`buffer_len`), and it asserts the struct's child count (`:362`). So importing a batch against the *declared* type
(`from_ffi_and_data_type`, `:292`) can't detect a batch built for another type. An `i32` buffer imported as `i64` is
sized twice its allocation, `validate_full` passes because the lengths match by construction, and the copy reads
out of bounds. A child-count mismatch panics instead. With the batch's own schema, the bridge does this in order:

1. It compares the pushed schema with the declared one (child count, names, format strings, nullability flags),
   reading only the schema, and refuses a mismatch with `TSRA_E_SCHEMA` before touching the array.
2. It imports with `arrow_array::ffi::from_ffi(array, &schema)` (`:276`), so every length comes from the producer's
   own description of its buffers.
3. It runs `ArrayData::validate_full` (offsets, UTF-8, null counts) before reading a value.

What no consumer can check is a producer whose schema misdescribes its own buffers. That is the C Data Interface's
precondition on every producer, and the header states it. Re-importing the schema per push costs a few small
allocations, which is noise against a 65,536-row batch. The spike didn't isolate it.

Only the clean lane of ADR-0056 §2 is accepted, matched exactly against the declared dtype:

| Arrow format | `ColumnData` / dtype code |
| --- | --- |
| `c` `s` `i` `l` | `i1` `i2` `i4` `i8` |
| `C` `S` `I` `L` | `u1` `u2` `u4` `u8` |
| `f` `g` | `f4` `f8` |
| `b` | `b1` |
| `u` `U` `vu` | `str` |

Any other type, including the types ADR-0056 §2 maps with a recorded transform (timestamps, decimals,
dictionaries, `FixedSizeList`, structs nested inside the batch), is refused with `TSRA_E_SCHEMA` at the
first push that carries it. A producer declares the physical dtype it already has and records the unit and the scale in the
schema (§8). For example, gPET-sim's `time_ps` is `f8` with unit `ps`, not an Arrow `Timestamp`. A recorded
transform is a receipt for a conversion that ingest had to make from someone else's file. A producer writing its own
data has nothing to convert, so accepting those types would only add a second spelling of the same column.

**Nullability comes from the declaration, not from the data.** `arrow_table.rs:36-43` decides nullability by the
data, which is right for a whole file read at once. A stream can't do that: whether a later batch carries a null is
unknown when the first leaf is encoded. So a column the schema declares nullable is always sealed as
`ColumnData::Nullable`: the bridge canonicalises each batch through `canonicalise_batch` and then wraps a column
that came back without a mask in an all-valid one. The same rows ingested from an Arrow IPC file with no nulls
would seal as a plain column, so a bridge product and an ingested one agree byte for byte only where the declared
nullability matches the data. That's the price of deciding before the data is seen, and it's stated in the
schema, which is sealed. A null in a column the schema declares non-nullable is refused (`TSRA_E_SCHEMA`), with the
column and the sequence key in the error.

**Why Arrow C Data, and not the alternatives:**

- **arrow-cpp as a dependency of ours.** It isn't one. The C Data Interface is a pair of C structs with an ABI that
  Arrow declares stable, so the bridge fixes the structs and not a library. A producer may
  fill them with arrow-cpp, with nanoarrow, or by hand. We recommend **nanoarrow**: it can be vendored as two files,
  it doesn't drag a large C++ dependency into a Geant4 build, and its builders (§12) are what turn the existing
  `PETHit` structs into a batch.
- **Per-product C structs** (a `struct PETHit` in the header, pushed as an array of records). That is an ABI per
  schema: every new column of every producer would be a header change and an ABI bump, and the bridge would
  re-implement a transpose that Arrow's columnar layout already is. Rejected.
- **Raw per-column structs** (`{name, dtype, data, len, validity}`). These would be a private re-spelling of what
  Arrow C Data already specifies: validity bitmaps, string offsets, the release protocol. We would document, test
  and version all of it ourselves, and it would buy nothing a producer can't get from nanoarrow's builders.
- **Vortex at the boundary.** Our table path already encodes Arrow-shaped columns into Vortex, so Arrow at the
  boundary adds no conversion, and keeping Vortex out of the C API keeps the storage engine replaceable behind it.
- **An Arrow IPC stream over a pipe into a `tessera` process.** This gives process isolation (no shared address
  space, no ABI at all), but every batch is serialised and parsed again, the producer must link an IPC writer, and the
  job gains a second process whose lifecycle, failure and exit code it must manage. It's the *later* channel for
  producers in another process (a `tessera ingest arrow-stream -` verb needs no ABI), and it doesn't conflict with
  this one. It isn't
  the primary path because it moves the cost to exactly the hot loop the owner wants cheap.
- **Arrow C *Stream* Interface** (`ArrowArrayStream`, pull-based). Here the consumer pulls batches from a producer
  callback. Geant4 pushes from many worker threads at end of event, and inverting that into one pull callback would
  put a queue and its locking in every producer. Push with a sequence number keeps the queue on our side (§7).

### §2.2 Arrays: a plain C buffer, a dtype code and a shape

A volume (an image, a sinogram, a lookup map like `/config/lyso_map`) is written with `tsra_array_write` from a
`tsra_array_view`: the dtype as a numpy code (`i1` … `u8`, `f2` `f4` `f8`, `b1`, the ladder `ArrayData::from_le_bytes`
accepts, `tessera-io/src/array.rs:300`), `ndim`, a `shape`, a C-contiguous little-endian buffer (host order on every
supported target) and its byte length, plus optional axes, a unit and a codec. The call names the array's role
in the schema. The bridge writes the buffer to a raw file in the session's staging directory before it returns, so
the caller may free it straight away and no array waits in RAM. At `finish`, each array is read back and encoded on
a worker (`stream::array_job`, `stream.rs:174`) on the arrays lane, in role order, which seals after the table
lanes (§7.3).

**Why not Arrow for arrays.** The C Data Interface has no N-D array type. Its `fixed_shape_tensor` canonical
extension is a *column of* tensors, one per row. **Why not DLPack.** `DLManagedTensor` is a good fit for ML
frameworks, but no current producer has one, and its device and stride generality would be accepted only to
refuse everything but host memory in C order. Strides are refused: a producer with a strided view copies it first,
and the copy is then its own, visible cost.

An array is whole in RAM while it encodes, as in every array path today (`array_job` takes an `ArrayData`). Whether a C++ producer
writes volumes larger than RAM is **pending surveys**. If one does, a chunked array writer is a `tessera-io` feature
first and a bridge call second.

## §3 — API surface and lifecycle

The header is generated (§11). This sketch fixes the shape, not the spelling:

```c
/* versions (§10) */
#define TSRA_ABI_MAJOR 0
#define TSRA_ABI_MINOR 1
#define TSRA_ABI_CHECK() /* run time: aborts unless the library's major == TSRA_ABI_MAJOR, minor >= ours */
uint32_t    tsra_abi_version(void);                  /* (major << 16) | minor */
const char *tsra_version_string(void);               /* crate version, e.g. "0.1.0-alpha.6" */

/* errors (§5) */
enum { TSRA_OK = 0, TSRA_E_ARG = -1, TSRA_E_SCHEMA = -2, TSRA_E_ORDER = -3, TSRA_E_STATE = -4,
       TSRA_E_IO = -5, TSRA_E_INTEGRITY = -6, TSRA_E_FORMAT = -7, TSRA_E_PANIC = -8, TSRA_E_STALL = -9 };
const char *tsra_last_error(void);                   /* thread-local; valid until this thread's next tsra_ call */

/* session = one product: one lane per table role, plus arrays */
typedef struct tsra_session tsra_session;            /* opaque */
typedef struct {
    size_t      struct_size;                         /* sizeof(tsra_session_options), for ABI growth (§10) */
    const char *stage_dir;                           /* WriteSession directory: absent or empty, disk-backed */
    const char *schema_json;                         /* the product schema (§8), required: tables keyed by role */
    const char *product, *name, *description, *timestamp;   /* identity inputs, RFC-3339 timestamp */
    uint32_t    workers;                             /* encode workers; 0 = WriteConfig::for_system() */
    uint64_t    ram_budget_bytes;                    /* 0 = DEFAULT_RAM_BUDGET */
    uint32_t    window;                              /* per lane: batches admitted ahead of next; 0 = 64 */
    uint32_t    max_ahead;                           /* per lane hard horizon, >= window + threads; 0 = 16 * window */
    uint32_t    refuse_instead_of_block;             /* 1: never block, refuse instead (§7.2); 0 = block */
    uint32_t    stall_timeout_ms;                    /* no advance of a lane's next while blocked; 0 = 600000,
                                                        at most 86400000 (24 h), above = TSRA_E_ARG */
} tsra_session_options;
int32_t tsra_session_open(const tsra_session_options *, tsra_session **out);

/* sealed metadata and provenance: allowed until finish, held by the bridge, applied at the seal */
typedef struct {
    size_t      struct_size;
    const char *tool, *version;                      /* required */
    const char *git_commit, *git_repo;               /* nullable */
    int32_t     dirty;                               /* -1 unknown, 0 clean, 1 dirty */
} tsra_producer;
int32_t tsra_session_set_study(tsra_session *, const char *study);
int32_t tsra_session_set_field(tsra_session *, const char *id, const char *value_json);
int32_t tsra_session_set_extra(tsra_session *, const char *key, const char *value_json);
int32_t tsra_session_add_source(tsra_session *, const char *relation, const char *reference,
                                const char *content_hash /* nullable */);
int32_t tsra_session_set_producer(tsra_session *, const tsra_producer *);   /* ADR-0058 §1 */
int32_t tsra_session_set_generation(tsra_session *, const char *config_json,
                                    const char *config_ref /* nullable */); /* ADR-0058 §2 */
/* unsealed aux/ (ADR-0042, ADR-0047) */
int32_t tsra_session_set_identity(tsra_session *, const char *identity_json,
                                  const char *const *age_recipients, size_t n_recipients);

/* table lanes (§7, §8): one per table role; push from any thread, any number of threads at once */
typedef struct tsra_table tsra_table;                /* opaque, owned by its session */
typedef struct {
    size_t             struct_size;
    uint64_t           first_seq;                    /* the lane's first sequence number (§7.2) */
    uint64_t           seq_block;                    /* B: key values per seq, pinned; 0 = no block cut (§7.2) */
    uint64_t           seq_key_base;                 /* key value of seq 0 (gPET-sim events: eventIdOffset) */
    const char *const *optional_columns;             /* the schema's optional columns this lane writes */
    size_t             n_optional_columns;
} tsra_table_options;
int32_t tsra_table_open(tsra_session *, const char *role, const tsra_table_options *, tsra_table **out);
int32_t tsra_push(tsra_table *, uint64_t seq, struct ArrowArray *batch /* always moved */,
                  const struct ArrowSchema *batch_schema /* borrowed; describes batch (§2.1) */);
int32_t tsra_mark_done(tsra_table *, uint64_t seq);  /* seq carries no rows; same as pushing an empty batch */
int32_t tsra_table_close(tsra_table *, int64_t expected_count /* < 0 = unchecked */);  /* after its pushes */
int32_t tsra_session_abort(tsra_session *, const char *reason); /* any thread, any time: poison, wake waiters */

/* arrays (§2.2) */
typedef struct {
    size_t             struct_size;
    const char        *dtype;                        /* numpy code: "i2", "f4", "b1", ... */
    size_t             ndim;
    const uint64_t    *shape;
    const void        *data;                         /* C-contiguous, little-endian, borrowed for the call */
    size_t             nbytes;
    const char *const *axes;                         /* nullable, else ndim names */
    const char        *unit, *codec;                 /* nullable; codec "pcodec" (default), "zstd", "auto" */
} tsra_array_view;
int32_t tsra_array_write(tsra_session *, const char *role, const tsra_array_view *);

/* seal: after every table is closed */
int32_t tsra_session_finish(tsra_session *, const char *out_path, char **manifest_json_out /* nullable */);
void    tsra_session_free(tsra_session *);           /* also after a failed or skipped finish; frees its tables */
void    tsra_string_free(char *);
```

**The lifecycle:** `open` → metadata calls, `table_open` per role → `push` / `mark_done` on the lanes and
`array_write`, in any interleaving and from any threads → `table_close` per lane → `finish` → `free`.

- **One session is one product, with all its tables.** gPET-sim's run is one `gpet-sim:g4ring-output` product with
  an `events` lane and a `pethits` lane. geddf's coincidences would be one product with seven lanes and an array.
  Many products means many independent sessions.
- A product written by several *processes* at once is out of scope. It needs an exploded layout, which is a later
  design.
- `tsra_table_close` refuses (`TSRA_E_STATE`) while a push on its lane is in progress, so the producer joins the
  threads that push to it first. It refuses with `TSRA_E_ORDER` if a sequence number below the largest one pushed
  is missing, or if `expected_count >= 0` and exactly that many weren't pushed (§7.2). Only then does it hand the
  lane's terminal leaf over.
- `finish` refuses (`TSRA_E_STATE`) while a lane is open, and refuses while a block the schema requires is missing.

On success, `finish` seals through `StreamWriter::finish_with` (#831), then writes `aux/provenance.json`
(`stamp_ingest_provenance`, with the tessera version as the unsealed tool stamp) and, if one was set, the identity
envelope (`aux/identity/identity.age`). It returns the sealed manifest as JSON if asked.

`free` without a successful `finish` abandons the product: it closes the pipeline, joins every worker and the
committer, deletes the staging directory and leaves no output file. `StreamWriter` has no such path today (it has no
`Drop` and no abort; only `finish` joins its threads, `stream.rs:1112-1133`), so #831 adds `StreamWriter::abort`.

**`stage_dir` must be absent or empty.** `WriteSession::create` doesn't refuse a directory a crashed session left
behind: it rewrites the header, truncates the journal (`create_file`, `write.rs:390-392`, called at `:620`) and
resets `staging/` (`reset_staging`, `:453-459`). Writing into such a directory would silently destroy whatever it
held, so `tsra_session_open` refuses a non-empty `stage_dir` with `TSRA_E_STATE`.

**Metadata may be set at any point before `finish`.** It can't go onto the `WriteSession` as it arrives:
`StreamWriter::new` takes the session by value (`stream.rs:746`, `:757`) and its committer thread owns it until the
seal (`commit_loop`, `:1144`), so no header setter is reachable once streaming has started. The bridge therefore
holds the sealed metadata itself (study, fields, extras, sources, producer, generation, the schema and the
canonicalisation transforms), validating each value when it is set, and hands it over at `finish`. The
`tessera-io` half is two issues. #831 adds a `StreamWriter::finish_with(out, header)` that applies a header patch to
the session after the last commit and before the seal, through the same setters (`write.rs:638-682`). #810 adds the
`with_schema` and `with_ingest_transform` setters the session lacks. A value set late is then exactly as sealed as one set early, and a
producer can record `events_run` or a completion flag at the end of a run without a second pass.

**Crash recovery isn't exposed in this ABI version.** `WriteSession::recover` (`write.rs:987`) replays committed
blocks, but the table commits its leaves only as they're handed to the `StreamWriter` (§7.3). A crash
mid-run therefore recovers a prefix of whole leaves, not the run. That's not a resumable product, and the bridge
doesn't pretend otherwise. A simulation re-runs from its seed. Exposing `recover` is worth doing once a producer
appears whose run can't be repeated (a DAQ): the re-open trigger is a survey answer naming one.

## §4 — Ownership

- **`struct ArrowArray *batch` is always moved.** On every return path, `tsra_push` takes ownership. It moves
  the struct out (`FFI_ArrowArray::from_raw`, arrow-data 58.4.1 `src/ffi.rs:224-234`, which sets the producer's
  `release` to `NULL`) before it does anything that can fail. The import is zero-copy: arrow-rs wraps the producer's
  buffers (spike §4, `the_import_is_zero_copy`). The bridge then copies the rows once, into `ColumnData`, which owns
  a `Vec` per column (`tessera-io/src/table.rs:82`), and drops the import, which calls the producer's `release`
  exactly once (`ffi.rs:68-71`), **before `tsra_push` returns**, even when the push then waits for the window
  (§7.2). The prototype counted 23 releases for 23 pushes, refused ones included (spike §1). So a producer never has
  an Arrow buffer held in the writer's staging. The producer never releases a pushed batch and never touches it
  again, whatever the status. The rule is unconditional because a conditional one ("released unless the call failed with X") is the classic double
  release or leak in C callers. It is also what nanoarrow's `ArrowArrayMove` expects.
- **Every `const struct ArrowSchema *` is borrowed,** at every `tsra_push`. The bridge
  reads it during the call and never keeps it, so the producer may reuse one schema for every batch and release it
  when it likes.
- **`release` runs on the pushing thread, inside the call.** Because the copy happens before `push` returns, the
  producer's callback never runs on an encode worker. That is a promise of the ABI, so an optimisation that keeps
  the producer's buffers past the call (spike §4's borrowed `ColumnData`) would be a major ABI change.
- **Buffers in `tsra_array_view` are borrowed for the call only.** They are copied before it returns.
- **Strings in are borrowed. Strings out** (`manifest_json_out`) are owned by the caller and freed with
  `tsra_string_free`, never with `free()`, because the allocator is Rust's.
- **Handles.** A session is created by `tsra_session_open` and destroyed by `tsra_session_free`. A table lane
  belongs to its session: it stays valid until `tsra_session_free`, which frees it, and has no free of its own, so
  there is no order in which two frees can go wrong. Using a handle after its session's `free`, or freeing a session
  while another thread is still inside a call on it, is undefined behaviour, as for any C API. Those are the rules
  the bridge can't check.

## §5 — Error model

- **Every fallible call returns an `int32_t` status.** `TSRA_OK = 0`. The codes are negative (the prototype's
  were positive; the ABI fixes one sign, and this is it), stable, and only ever
  added to:

  | code | meaning | from |
  | --- | --- | --- |
  | `TSRA_E_ARG` | null pointer, bad UTF-8, `struct_size` too small, malformed JSON | the bridge |
  | `TSRA_E_SCHEMA` | the schema refuses a column, a batch, a null or the seal | §8, `Error::Invalid` |
  | `TSRA_E_ORDER` | a sequence key out of order or outside its batch's block, a `seq` below `first_seq` or already pushed, one past the horizon, a push the window would block in refuse mode, a gap or a count mismatch at `tsra_table_close` | §7.2 |
  | `TSRA_E_STALL` | a lane's `next` didn't advance for `stall_timeout_ms` while pushes were blocked; the error names the lane and the missing `seq` | §7.2 |
  | `TSRA_E_STATE` | wrong lifecycle state: a call after `finish`, a poisoned or aborted session, a close during a push on its lane, `finish` with a lane open | the bridge |
  | `TSRA_E_IO` | filesystem errors, including a full disk in `stage_dir` | `Error::Io` |
  | `TSRA_E_INTEGRITY` | a digest or seal self-check failed | `Error::Integrity`, `BlockIntegrity` |
  | `TSRA_E_FORMAT` | any other writer refusal: names, logical tables, codecs | the other `Error` variants |
  | `TSRA_E_PANIC` | a bug in tessera, caught at the boundary | `catch_unwind` |

  `tessera_core::Error` is `#[non_exhaustive]` (`tessera-core/src/error.rs:13`), so the bridge's mapping ends in a
  `_ => TSRA_E_FORMAT` arm. A new variant then degrades to a correct, coarser code, not a build break. A unit test
  pins the mapping of every variant that exists.
- **`tsra_last_error()`** returns the `Display` text of the last failure on the calling thread, including the
  source chain. It is valid until that thread's next `tsra_` call, and it is `""` after a success. It is
  thread-local because an error from one Geant4 worker must not be overwritten by another's success before the first
  thread reads it.
- **No unwinding crosses the boundary.** Every `extern "C"` function body runs inside `std::panic::catch_unwind`. A
  caught panic returns `TSRA_E_PANIC`, with the panic message as the last error, and **poisons** the handle it was
  called on: every later call on that handle returns `TSRA_E_STATE`, and only `free` works. The invariants of a
  handle that panicked mid-call are unknown, and sealing from it could attest to bytes no one checked. Panics on the
  encode workers are already contained: `StreamWriter::finish` joins every thread and reports "encode worker
  panicked" (`stream.rs:1112-1133`, the message at `:1121`), which surfaces as `TSRA_E_FORMAT` from `finish`.
- **`panic = "abort"` is refused for this crate.** `catch_unwind` needs unwinding. The workspace profiles don't set
  `panic` (`tessera/Cargo.toml`, `[profile.dist]` inherits `release`), and a check in the crate's `build.rs` fails
  the build if `CARGO_CFG_PANIC` is `abort`, so a downstream profile can't silently turn a reported bug into a
  killed Geant4 job.
- **An ordering or schema refusal poisons the session.** Every later call returns `TSRA_E_STATE` and the session
  can only be freed. Blocked pushes wake and return the same error. A product whose table the bridge refused part of
  would attest to rows it doesn't hold.
- **A failure inside the pipeline poisons the session, not just the call.** The `StreamWriter` refuses every push
  after a failed encode or commit (`stream.rs:845`), and the `WriteSession` poisons itself on a failed journal write
  (`write.rs` module docs). The bridge reports the first such error from every later call on the session, so a
  producer that checks only `finish` still sees the cause.

## §6 — Thread-safety contract

- **Every call may come from any thread, concurrently.** The session serialises its metadata calls behind one
  mutex. `tsra_push` is the hot path, and any number of threads may be inside it at once, on one lane or on
  several. The schema check, the
  import and the copy into `ColumnData` run on the pushing thread with no lock held, so they scale with the
  producers (spike §5: the bridge was level with native at 4 threads and faster at 16). Only the admission to the
  window and the release of the due run are serialised (§7.2).
- **Determinism doesn't depend on the calling thread.** Which thread pushes a `seq`, how many threads there are and
  how they're scheduled never reach the bytes (§7). That's the contract #802 asks for, and it's what makes "any
  thread, any time" safe to promise.
- **Back-pressure blocks the calling thread, by design,** in exactly two places. A push that runs too far ahead
  waits at its lane's ordering gate until the window moves (§7.2). The due push is never held there. But the push
  that releases a contiguous run, the due one included, can fill a leaf, and handing that leaf to the encoder can
  wait while the encode window is full (`stream.rs:840-845`). That wait always ends, because the committer and the workers don't depend on the producer.
  A Geant4 worker blocked in either simply asks for its next event later. When the pipeline dies, the session is
  aborted, or the stall deadline passes, every blocked push wakes with an error (§7.2), so no thread waits forever.
- **No thread-local state but the last error.** The bridge keeps no per-thread writer. The "one HDF5 file per
  thread" pattern and its initialisation mutex (`SimulationManager.cpp:269-277`) disappear.
- **`fork()` while a session is open is unsupported.** Its worker threads don't survive into the child.

## §7 — Ordering: sealed bytes independent of producer scheduling (#802)

### §7.1 The invariant

**The sealed bytes depend only on the `seq → batch` mapping.** For a fixed schema, metadata and mapping, the sealed
`.tsra` is the same whatever the number of producer threads, their scheduling, which thread pushed which `seq`, the
arrival order, the encode worker count and the RAM budget. That covers every byte outside the unsealed `aux/`
members: the wall-clock `aux/provenance.json` and the randomised `age` envelope. This extends the worker-count
invariance that #677's test (c) established (`stream.rs:2113`) to producer threads.

**Determinism comes from `seq`, not from threads.** Geant4 MT assigns events to threads dynamically, but its event
seeds are per event, so an event's rows don't depend on the thread that simulated it. The producer derives `seq` from
the event, under a block size `B` that the producer pins and declares (next subsection). What the bridge can't fix is the producer's own determinism: the rows inside a batch are
sealed in the order given. gPET-sim's are (`std::map`, §Context). A producer that iterates an `unordered_map` must
sort first.

### §7.2 Dense sequence numbers, the block size and the window

Each table lane has its own `seq` stream, its own `next` and its own window. Everything below is per lane, except
the byte credit, which the session's lanes share.

- **Dense.** A lane's sequence numbers are exactly `first_seq, first_seq + 1, …`, each pushed **exactly once**. A
  `seq` with no rows is pushed as an empty batch or with `tsra_mark_done(seq)`, which is the same thing. That's what
  lets the bridge release a `seq` as soon as every smaller one has arrived, without waiting for every thread.
- **The block size `B` is declared and pinned, because Geant4's own cut isn't.**
  - **Geant4's cut follows the thread count.** `G4TaskRunManager` sets `eventModulo = min(√E, E / grainSize)` for a run of `E` events, with
    `grainSize` the thread-pool size (Geant4 `source/run/src/G4TaskRunManager.cc:293-333` at `f3d5293`, gap analysis
    §4.1). So once there are more threads than `√E`, "the event-block index" depends on the thread count.
    `SetEventModulo` doesn't pin it unconditionally. Geant4 still caps the value at `E / grainSize`, with only a
    `JustWarning` (`:311-320`), and `G4FORCE_EVENTS_PER_TASK` overrides it (`:323-327`).
  - **So the key is the sequence key, and `B` cuts it, per lane.** A table's sequence key and its `order` are part
    of its schema, as in the #801 drafts: `sequence_key: ["event_id"]` for `events`, `["event_id", "hit_ordinal"]` for
    `pethits`, and `"order": "producer"` with an empty key for the single-producer tables (§8). Each lane declares, at
    `tsra_table_open`, `seq_block = B` and `seq_key_base` (gPET-sim's `eventIdOffset`). With `B ≥ 1`, **batch `seq`
    covers the rows whose first key component lies in `[base + seq · B, base + (seq + 1) · B)`**, which is the gap
    analysis's `seq = event_id / B` (§4.4 item 1). A block cut needs an integer first key component, so `B ≥ 1` on a
    lane whose first key component isn't an integer column is refused at open with `TSRA_E_ARG`. One session holds
    lanes with unrelated key domains (`time_markers` by `tm_ms`, `coin_size_dist` by `size`), and lanes whose first
    component is a string (`source_files` by `(source, file_index)`), which is why `B` is per lane. `B = 0` means no
    block cut: `seq` is the batch index, and only the order check below applies.
  - **The producer pins `B`.** The first way is one batch per event (`B = 1`, `seq = Geant4 event id`), independent
    of how Geant4 cuts its tasks; gPET-sim's `EndOfEventAction` flush has this shape (§12). The second is one batch
    per task, with `SetEventModulo(B)`, `B ≤ E / pool size`, and the effective task size read back
    (`GetNumberOfEventsPerTask()`). The run is refused unless that size `T` satisfies `T % B == 0` (gap analysis §4.4
    item 2). gPET-sim's Stage 2 sets the modulo (`g4ring/g4ring.cpp:614-626`) to a value that is itself capped at
    `nEvents / nThreads` (`g4ring/include/PhaseSpaceTasks.h:32-46`). So it isn't thread-count independent today, and
    the bridge's checks below refuse it rather than seal it.
  - **Recorded.** Each lane's `B`, key base and `first_seq` are sealed in the manifest's `extra` as `capi_seq`, keyed
    by role. `B` is a run parameter of the producer's, not part of the product contract, so it is a lane option and
    not a schema field. The sequence *key* is part of the contract, so it lives in the schema. A fixed `B` gives the
    same bytes, and a different `B` is a different, recorded recipe. The thread count is never sealed: gPET-sim's
    `numberOfThreads` belongs in `aux/provenance.json`, or the bytes would follow it by construction (gap analysis
    §4.4 item 5).
- **What the bridge checks on every batch** (#833): the lane's sequence key must be **lexicographically increasing**
  across the batch: strictly when the sequence key is also one of the table's `keys` (the drafts' `pethits`
  `(event_id, hit_ordinal)`), which proves the within-batch order and the key's uniqueness; non-decreasing when it
  isn't (a multi-row table keyed by `event_id` alone). Every row's
  first key component must lie in the batch's block when the lane has `B ≥ 1`. Otherwise the push is refused with `TSRA_E_ORDER`, naming the
  lane, the `seq`, the column and the key. That refuses any cut that disagrees with `B`, a thread-count-dependent
  Geant4 cut included. A misaligned task (`T % B != 0`) shows up as two pushes of one `seq`, a duplicate: an error,
  never a merge. A table with `"order": "producer"` and an empty `sequence_key` (gPET-sim2listmode's and geddf's
  single-producer tables) has no key check, and its `seq` is the push index. For such a
  table, the outcomes without a key are producer errors, and they're documented:
  - **A batch still holds a contiguous key range in key order** (only `B` is wrong). The sealed rows are the same,
    because the rows are re-chunked to the row-group grid and the bytes don't depend on batch boundaries
    (`accumulate.rs:12-15`). But the dense and `expected_count` checks count the wrong units.
  - **A batch isn't a contiguous range** (say a thread accumulates the events of several tasks into one batch). Then
    the row order follows the schedule and **the bytes differ** between thread counts.
- **The window.** `next` is the lane's smallest `seq` not yet pushed. A push of `seq`:
  - `seq == next`: **never held at the ordering gate.** It and every waiting batch that follows it contiguously are
    released into the lane's table (§7.3). This is the rule `StreamWriter`'s own credit gate keeps for the job its
    committer waits on (`stream.rs:495`). The only wait the due push can meet is the encode window, when its release
    fills a leaf (§6), and that wait depends on the encoders alone, never on another producer.
  - `next < seq < next + window`, while the batches waiting ahead of `next` hold less than the byte credit: admitted.
    It waits in the buffer, already converted (§6).
  - `seq >= next + window`, or the byte credit is held: the push **blocks** until `next` advances far enough. This
    is the back-pressure that bounds the memory held ahead of a slow thread: at most `window` batches per lane, and
    at most the shared byte credit (half of `ram_budget_bytes`; the other half is the `StreamWriter`'s encode window).
    With `refuse_instead_of_block = 1` the same condition, window *or* credit, is refused at once with `TSRA_E_ORDER`.
    That refusal is terminal, so that mode is fail-fast and never blocks on order.
  - `seq >= next + max_ahead`: **refused at once**, `TSRA_E_ORDER` ("too far ahead"). Under the claiming contract
    (below), N threads can hold sequence numbers up to `next + window - 1 + N`, so `max_ahead` must be at least
    `window + N`, or a well-formed run is refused. The default of 16 × `window` covers any realistic thread count,
    and the header documents the bound.
  - `seq < next`, or a `seq` already entered: **refused**, `TSRA_E_ORDER` (duplicate). A push reserves its `seq`
    under the gate's lock on entry, before it can block, so two pushes of the same `seq` are caught even while both
    would wait.
  - At `tsra_table_close`, any `seq` missing below the largest one pushed is **refused** with `TSRA_E_ORDER`, and the
    error names the first one. With `expected_count >= 0`, close also refuses unless exactly that many were pushed. The
    count is a close argument, not an open option, because a producer may learn it only at the end (a paused run,
    §12). gPET-sim passes the number of event blocks of the run and gets a sealed product whose completeness is
    proven, not assumed.

  Every refusal poisons the session (§5), and every blocked push then wakes with the error. The same happens when the
  pipeline dies (an encode or commit failure, a panicking worker): the window is closed and every waiter wakes, as
  `CreditWindow::close` does for the `StreamWriter`'s own waiters (`stream.rs:567`).
- **A `seq` that never comes.** A missing `seq` is the one fault the bridge can't see as a call: threads past it
  block, and the close is never reached because the workers never return. Two things turn it into an error:
  - `tsra_session_abort(session, reason)`, callable from any thread while pushes are in progress, poisons the
    session and wakes every blocked push with `TSRA_E_STATE`. A producer calls it from the error path that lost the
    `seq` (gPET-sim: a worker's exception handler).
  - **The stall deadline is finite.** `stall_timeout_ms` (default 10 minutes; at most 24 hours, and a larger value is
    refused at open with `TSRA_E_ARG`; `0` = the default)
    bounds how long a lane's `next` may stay where it is while a push on that lane is blocked. It measures the
    absence of progress, not one push's wait, so a slow run whose `next` keeps advancing never trips it. When it
    passes, the session is poisoned with its own typed status, `TSRA_E_STALL`, naming the lane and the missing
    `seq`, and every waiter wakes. There is no value that disables it, because the owner ruled out a hang. A run
    whose single event takes longer than 24 hours raises the deadline toward the cap, or uses refuse mode, where
    nothing waits.

  **The deadline is mandatory (owner ruling, 2026-11-13).** The proposal left open whether an explicit, documented
  "no stall deadline" opt-in should exist for producers whose single events can exceed 24 hours. The owner ruled
  that it doesn't: the finite deadline (default 10 minutes, cap 24 hours, `0` = the default, expiry
  `TSRA_E_STALL`) applies to every session, and refuse mode is the path for a producer that can't live within it.

  So every ordering fault has a typed error outcome: a duplicate, a gap at close, a `seq` too far ahead, a key out of
  its block, a lost `seq` and a dead pipeline.
- **Why the back-pressure can't hang a well-formed producer.** A push blocks at the gate only for a `seq` other than
  `next`, and the push of `next` is never held there. So a lane can stall only if no running thread will ever push
  its `next`. That can't happen under the **claiming contract**:
  - sequence numbers are claimed in increasing order across threads (a shared counter, `G4TaskRunManager`'s
    consecutive event blocks, `EventAction.cpp:62-63`, or one batch per event under that manager), **and**
  - each thread pushes its own in increasing order, on every lane it writes, in the same order across lanes.

  Then `next` was claimed before every larger `seq`, and the thread holding it has no smaller `seq` of its own left to
  push on that lane. So it isn't blocked: it is computing and will push. The byte credit doesn't change this, because
  the due push ignores it.

  Across lanes, gPET-sim pushes `pethits(e)` before `events(e)` for every event `e`. When `next_events` is `e`,
  every event below `e` has pushed both, so `next_pethits ≥ e`. The thread holding `events(e)` has therefore either
  already pushed `pethits(e)` or holds the due `pethits` push, and it is never blocked on `pethits`. A producer that interleaves
  lanes in a different order on different threads breaks the contract, and the header says so.

  The spike exercised the single-lane case (claimed keys through a window of 4, 200 batches, N ∈ {1, 4, 16}, one
  hash; spike §3.1). It also showed the converse. Under an *arbitrary* assignment every thread can hold a `seq` past
  the window while `next` is unclaimed, and a pure blocking window then deadlocks. That is why the claiming contract
  is part of the ABI, stated in the header next to `tsra_push`, and why refuse mode exists for producers that can't
  promise it.
- **Why not spill instead of blocking.** Spilling waiting batches to disk would remove the precondition, at the cost
  of unbounded staging disk behind a slow thread. The owner chose back-pressure. It bounds memory *and* disk, and the
  precondition holds for every surveyed producer.
- **Why dense numbers, not ordered lanes with sparse keys.** The alternative is a lane per producer *thread* with
  strictly increasing, sparse keys and a k-way merge. That releases a key only once *every* lane has passed it, so an
  idle lane stalls every other one, and it needs a watermark call. Dense numbers need none of that, and every
  surveyed producer has one. (This ADR's table lanes are per *table*, not per thread.)
- **Why not sort by a key column at the seal.** That orders rows by their *values*. It needs a total order over every
  row and a full external sort of a table that is larger than RAM by design. The sequence key orders and checks rows
  inside a batch, and the sequence number orders batches. Neither one sorts.

### §7.3 Into the `StreamWriter`: what exists, and the minimal extensions

**What the `StreamWriter` does today, verified.** It accepts jobs only in push order. `submit` assigns the sequence
itself (`self.seq`, `stream.rs:1092-1101`), and `push`/`push_staged` take `&mut self` (`:845`, `:852`), so neither an
explicit sequence nor concurrent pushers are possible. It has one sequence for the whole product, so two tables
streamed at once would interleave in commit order. What it already has is the machinery #802 needs, at block
granularity:

- an ordered committer whose reorder buffer (a `BTreeMap` of early completions and a `next` counter) commits strictly
  in sequence order (`commit_loop`, `:1144-1172`);
- a credit window (`CreditWindow`, `:414`) that bounds jobs and bytes in flight, never stalls the job the committer
  waits on (`:495`), and wakes every waiter when the pipeline dies (`close`, `:567`).

Three extensions, all in `tessera-io`, with no second scheduler, pool or ordering mechanism:

1. **Lanes (#832).** A job is submitted on a lane: one per table role, plus one for the arrays. Each lane has its own
   sequence counter. The committer commits each lane's jobs in that lane's order, independently of the other lanes
   (the same reorder buffer, keyed by `(lane, seq)`). The journal records the lane, so `recover` keeps it. At the
   seal, the manifest orders blocks by lane, with the table lanes by role name and the arrays lane last, and by
   commit order within a lane: never by cross-lane timing.
   - The format allows this. `check_partitions` groups leaves by name, so leaves of different tables may sit in any
     relative order. The lane order is the bridge's determinism policy, not a format constraint.
   - The seal's integrity cross-check becomes per lane: each lane's running Merkle accumulator must equal the root
     over that lane's digest subsequence of the sealed manifest.
   - A single-lane product is byte-identical to today's.
   - No table waits for another, and nothing is staged beyond each lane's own few leaves.
2. **The sequenced table input (#812),** one per lane: the same machinery one stage earlier, because a batch can't
   simply be a job. A job is a block, and one block per event would be millions of leaves, off ADR-0064's
   `BLOCK_ROWS` grid.
   - **Its admission gate is a second `CreditWindow`,** the same type, with limits of `window` batches and the
     shared byte credit, whose no-stall rule is keyed to the lane's next due `seq` instead of the next commit. The
     `max_ahead`, duplicate and key checks (#833) sit in front of it.
   - **Its release is the committer's pattern applied to batches:** a `BTreeMap` of waiting batches and a `next`. The
     push that completes a contiguous run drains it, on its own thread, under one lock per lane that orders the
     appends. That is O(m) for the m batches it releases.
   - Released batches go into #806's partition writer, which re-chunks them to the `ROWS_PER_GROUP` grid
     (`accumulate.rs:12-15`). So the leaves don't depend on how many rows each `seq` carried, and `B` drops out of
     the row bytes (gap analysis §4.4 item 3, second option).
   - **A full leaf becomes an ordinary staged job** on the lane. The existing workers encode leaves in parallel, and
     the existing committer commits them in lane order.
3. **The open-ended partitioned table (#806),** dispatched with one leaf of lookahead. Once a table has more than
   `BLOCK_ROWS` rows, it is partitioned, and leaf `i` is the non-terminal leaf `<name>/{i:04}` (`table.rs:886-892`) as
   soon as leaf `i + 1` holds a row. So only the last leaf waits for close.
   - `push_table` needs a row count up front (`stream.rs:861-863`), and `TableMultiBlockSink` stages the whole table
     before the first encode (`accumulate.rs:411-414`; measured as byte-identical in spike §7.3).
   - #806 is the gap analysis's issue (G19). It is built **byte-identical to `TableMultiBlockSink` for the same
     rows**, as `push_table` already is (`stream.rs:861-862`).

The prototype's `order.rs` (a `Mutex`, a `Condvar` and a `BTreeMap` in the bridge) is the shape of the gate. #812
moves it behind `CreditWindow`, so the bridge and every Rust producer share one implementation.

### §7.4 The proof

The #834 suite runs 1, 4 and 16 producer threads over the same events. The setup:

- A Geant4 mock hands out task blocks of `T = min(√E, E / threads)` events for a run of `E` events (`E = 64`: `T` = 8, 8,
  4 at 1, 4 and 16 threads), with `T % B == 0`.
- The producer derives `seq` from the event ids under a pinned `B` (`seq = event_id / B`), and pushes two lanes
  (`events`, `pethits`) in the claiming-contract order.
- The `seq → thread` assignment is shuffled across threads by a seed, and each thread sleeps a seeded random time
  between pushes.
- This is crossed with two worker counts and two windows, one small enough to block.

It asserts:

- one sealed `manifest_hash`, one `content_hash` and identical block bytes across every run, equal to a
  single-threaded in-order push;
- the window bound: the peak number of batches admitted ahead of `next` is at most `window` per lane, and the peak
  bytes are at most the credit, read from the input's stats;
- that blocking actually happened in the small-window runs, so the bound isn't vacuous;
- **the negatives that show `B` is load-bearing** (gap analysis §4.4 item 7):
  - `seq` taken from Geant4's thread-count-dependent task index is refused by the key checks;
  - a misaligned `B` (`B = 8` with `T = 4` at 16 threads) is refused as a duplicate `seq`;
  - with an empty `sequence_key` and non-contiguous batches, the bytes differ across thread counts, asserted as the
    documented producer error;
- that a gap at close is `TSRA_E_ORDER` and writes no file, and that each of these turns every blocked push into an
  error rather than a hang:
  - a duplicate, including two blocked pushes of one `seq`;
  - a `seq` past `max_ahead`;
  - a lost `seq` with pushers blocked behind it, by `tsra_session_abort` and by the stall deadline (set short in the
    test, `TSRA_E_STALL`);
  - a pipeline failure;
- that a within-thread shuffle under `refuse_instead_of_block = 1` yields refusals, never a hang.

The suite runs in Rust against `tessera-io`'s sequenced input. #813 runs the 16-thread case against `tessera-capi`'s
`extern "C"` functions, and a C++ program compiled against `tessera.h` runs it through the real ABI (§11), so the
contract is proven where producers meet it. The spike already showed the core of it: 9 of 9 shuffled-schedule runs
sealed one hash, and keying by arrival gave 9 distinct ones (§16).

## §8 — Schemas (#801)

This section follows the merged #801 gap analysis (`docs/spikes/801-consumer-gap-analysis.md` §3.1, §5) and its
drafts (`docs/spikes/801-consumer-schemas/*.tables.json`). Where they differ from an earlier revision of this ADR,
**the drafts win**.

- **The bridge writes only schema'd products, and the schema is declared at session open.**
  `tsra_session_options.schema_json` is a `ProductSchema` as JSON (from the registry, below). Its `product` must
  equal the session's `product`. If the built-in registry knows the product, the supplied schema must equal the
  built-in one (otherwise `TSRA_E_SCHEMA`), so a producer can't silently fork a registered schema. `tsra_table_open`
  and `tsra_array_write` accept only roles the schema declares.
- **The column contract, in the drafts' shape (G1, #809).** This is #801's engine change.
  - `ProductSchema` gains `tables`, a map **keyed by role**. Per role it holds:
    - an ordered `columns` list, reusing `Column` (`tessera-core/src/block/table.rs:12`): name, dtype, nullable,
      unit, scale and description;
    - `sequence_key`, the ordered key columns that §7.2 checks;
    - `order`: absent means the rows are in `sequence_key` order, and `"producer"` means the producer's own order with
      an empty `sequence_key`, as the drafts mark the single-producer tables;
    - `keys`, a list of column-name lists, and `references` (#780). Each reference entry is the drafts' shape:
      `columns`, a target `to { scope, block, columns }` (`scope` is `"block"` or `"source(<role>)"`) and an optional
      free-text `note`.
  - It also gains `arrays`, a map keyed by role (dtype, rank, axes, unit). That map is this ADR's addition: no draft
    has one, and `geddf:coincidences` declares its `lyso_map` only as an array block requirement in its
    `*.schema.json`.
  - Both are `skip_serializing_if` empty, so every embedded schema sealed so far keeps its bytes and the conformance
    corpus doesn't move (the ADR-0058 §3 precedent, `schema.rs:236-242`).
  - **How the drafts map onto it.** Each product's draft is two files. `*.schema.json` is today's `ProductSchema`
    (fields and blocks), and `*.tables.json` holds the column contract, plus a top-level `product` (checked equal)
    and a `status` note, which is draft metadata, not contract. There are six schema files and five tables files:
    `geddf:pairwise-record` is metadata only. #809's acceptance test merges each `*.schema.json` with its
    `*.tables.json` `tables` map and parses the result under #830's strict rules, so every draft key must be a
    contract key. That's why `order` is one.
- **Where this ADR's shape is the drafts'.** The sequence key, the keys and the references are in the schema, not in
  the session options, because they're the product's contract. Only the producer's run parameters are session
  options, and they are sealed as `capi_seq` (§7.2): `B`, the key base and `first_seq`. An earlier revision of this
  ADR had a `seq_key_column` session option. The schema's `sequence_key` replaces it.
- **G6: an older binary must not silently drop the contract.** `ProductSchema` has no `deny_unknown_fields`
  (`schema.rs:69-71,227-228`), so `ProductSchema::from_value` (`schema.rs:322`) in today's binaries deserialises a
  schema with `tables` and drops the contract without a word. The embedded schema itself survives in the seal,
  because `Manifest.schema` is a JSON value, so the archive is intact. But such a binary validates less than the file
  declares. The additive claim above holds for bytes, not for meaning, unless #830 closes this:
  - `deny_unknown_fields` on `ProductSchema`, `FieldSpec`, `BlockRequirement` and the new contract types;
  - a `schema_format` integer (absent = 1; a schema with `tables` or `arrays` sets 2), which a reader refuses with a
    typed error when it is higher than it knows.

  From #830 on, a reader that can't honour the contract says so. Binaries already released (up to alpha.5) can't be
  fixed after the fact. They validate only the `fields` and `blocks` of a format-2 schema, and the release notes and
  the schema docs say so. Their *data* reads are unaffected, because every sealed block's own `TableSpec` carries its
  columns. (#573 is the same class of defect for the manifest as a whole.)
- **G2: requirements match by role (#830).** Today `BlockRequirement::matches` checks only the kind
  (`schema.rs:219-222`), so one `events` table satisfies a required `pethits`. #830 matches by role, and the bridge
  records each block's role through its lane. In scope.
- **The optional-column flag (#809).** The drafts list `nHits`, `n_compton` and `n_rayleigh`, which can be absent from
  a g4ring file altogether (gap analysis §5). A column gains `optional: bool`. In scope at the bridge: a lane declares
  the optional columns it writes at `tsra_table_open` (`tsra_table_options.optional_columns`), and every batch on that
  lane must carry exactly the non-optional columns plus that set, so the column set can't vary with the thread
  schedule. Per-row "not recorded" stays a nullable column, as the drafts model it.
- **G13: categorical / enum columns are deferred to #836.** `hit_type`, `source_type`, `source_photon_id` and the
  coincidence labels need a code table that `Column` doesn't have (`block/table.rs:12-71`). Until #836 lands, which
  also waits on the gap analysis's survey question 5, they cross the ABI as their integer codes (clean lane), and an
  Arrow `Dictionary` type is refused with `TSRA_E_SCHEMA`.
- **Keys and references (#780).** The bridge enforces a `keys` entry only where it coincides with the lane's
  `sequence_key`: the lexicographic strict increase (§7.2) proves uniqueness. `references` (for example
  `pethits.event_id → events.event_id`, and `crystal_id` → the crystal map) are carried and sealed with the schema,
  but checked nowhere yet. That's #780's.
- **The bridge validates at three points.**
  - `tsra_session_open` checks the schema itself: clean-lane dtypes only (§2.1), known array dtypes, and a
    `schema_format` it knows.
  - Every `tsra_push` checks the batch's own Arrow schema against the lane's declared columns before importing it
    (names, order, types per §2.1, nullability flags), then the nulls and the sequence key after the import. A
    refusal names the lane, the column and the `seq`.
  - `tsra_session_finish` runs `ProductSchema::validate` on the manifest **before** the archive is written:
    required fields, role-matched block counts and `requires_generation` (`schema.rs:249`). That needs #810's
    validate-before-pack hook on `WriteSession::seal`, which validates today in no path (gap analysis G5).
- **The schema is sealed with the product (G4).** `WriteSession::seal` never calls `ProductBuilder::with_schema`
  (`product.rs:113`), and only a built-in schema is embedded today (`product.rs:192-195`). #810 adds a session
  `with_schema`, and the bridge passes the schema through `finish_with` (#831). So the archive carries its contract,
  and every reader validates what the file declares (`validate_manifest`, `schema.rs:333`).
- **Where a C++ product's schema lives is the owner's call, and pending surveys** (G3).
  - The options are tessera's built-in registry (reviewed and versioned with tessera, ADR-0050's "a domain registers
    its own") or the producer's repository, next to the YAML it maintains today (`g4ring/schema/g4ring-output.yaml`).
  - The gap analysis keeps its drafts out of the live registry until the surveys report (§5).
  - This ADR recommends the producer's repository, with the `ProductSchema` JSON generated from the producer's YAML
    as the drafts' `generate.py` does, so the two can't drift. The bridge API is the same either way.

## §9 — Metadata, provenance and the seal

- `set_field`, `set_extra`, `set_study` and `add_source` correspond one-to-one to the `WriteSession` setters
  (`write.rs:638-662`). The bridge holds them and applies them through `finish_with` (§3, #831, #835). JSON values are
  parsed once at the call, so a malformed value fails there, not at the seal.
- `set_producer` maps onto `with_producer` (ADR-0058 §1, `write.rs:672`, `Producer`,
  `tessera-core/src/provenance.rs:46-59`). gPET-sim seals its own name, version and git commit. tessera's version goes
  to the unsealed `aux/provenance.json`, as for every external producer.
- `set_generation` maps onto `with_generation` (ADR-0058 §2, `write.rs:682`). gPET-sim's run configuration (the JSON
  it already loads with nlohmann_json) is the natural `config`. A schema that sets `requires_generation` makes it
  mandatory at the seal (§8).
- **The canonicalisation transform** (NaN canonicalisation, which the owner placed in the bridge on 2026-11-13,
  §1) is collected by the bridge when it fires, deduplicated as ingest does, and sealed through `finish_with` (§3).
  `WriteSession` has no `ingest_transform` setter today, though `ProductBuilder::with_ingest_transform` exists
  (`product.rs:141`). #810 adds it to the session header, like `with_generation`.
- **`set_identity`** encrypts a JSON identity document to `age` recipients after the seal, as
  `aux/identity/identity.age`, through `tessera-identity` (ADR-0047, PR #786). Simulations carry no identity. The
  call exists for producers that write from patient data. Whether a C++ producer does is **pending surveys**: the
  only patient-data stage surveyed (Stage 2) is Python.
- **Signing isn't in the ABI.** `tessera sign` (ADR-0037) signs a sealed file after the fact, and a key in a Geant4
  process is a key on every cluster node. If a producer has to sign in-process, the `sign_tsra` call
  (`tessera-io/src/sign.rs`) can be wrapped later without touching anything here.

## §10 — ABI and version policy

- **The ABI version is separate from the crate version.** `TSRA_ABI_MAJOR` and `TSRA_ABI_MINOR` are macros in the
  header, and `tsra_abi_version()` reports the library's. A major bump is any change that breaks an existing caller:
  a removed or renamed symbol, a changed signature, a changed meaning, or a reordered enum. A minor bump is an
  addition: a new function, a new status code, a new trailing field in an option struct. The crate version follows
  ADR-0052, and a crate release can carry an unchanged ABI.
- **The soname is `libtessera.so.<ABI major>`** (`libtessera.<major>.dylib` on macOS). Pre-1.0 the ABI major starts at
  `0`, and every breaking change bumps it, even during the alpha. A consumer test campaign must never load a
  library that disagrees with the header it was compiled against.
- **The header carries a run-time check.** `TSRA_ABI_CHECK()` is a macro that expands to a call: it compares the
  header's `TSRA_ABI_MAJOR`/`TSRA_ABI_MINOR`, compiled into the caller, with `tsra_abi_version()` from the loaded
  library. It aborts with a message unless the majors match and the library's minor is at least the header's. There
  is no compile-time half, because the mismatch it catches (an old header against a new library, or the reverse)
  only exists once the program loads a library. gPET-sim calls it once at start-up.
- **Option structs start with `struct_size`.** The library reads only the fields that the caller's size covers and
  defaults the rest, so a minor bump can add a trailing field without breaking an older caller. Enums are only
  appended to. All handles are opaque, so no layout is ever part of the ABI.
- **Gates:** the generated header is committed, and a check regenerates it and fails on a diff (header drift). An
  `abidiff` check (libabigail, in nixpkgs) compares the built `.so` with the one from the last release and fails if
  the ABI changed without the matching version bump. The first release has nothing to compare against, so the gate
  starts at the second.
- **The format version is untouched.** The bridge writes through `tessera-io`, so `TESSERA_VERSION` (ADR-0042,
  `tessera-core/src/manifest.rs:20`) and the corpus are unaffected by any ABI version.

## §11 — Header, build and packaging

- **cbindgen** generates `tessera.h` from the `extern "C"` items, from a committed `cbindgen.toml` (C, include
  guard, `tsra_` prefix, `ArrowSchema`/`ArrowArray` taken from the Arrow C Data header's canonical definitions behind
  its `ARROW_C_DATA_INTERFACE` guard, so a producer that includes `nanoarrow.h` or arrow-cpp's `abi.h` first doesn't
  see a second definition). The header documents every function's ownership, threading and error behaviour, from the
  Rust doc comments.
- **Two artifacts.** `crate-type = ["cdylib", "staticlib"]`. The **shared library is the recommended one**: a Rust
  cdylib exports only the `#[no_mangle] extern "C"` symbols, so the C libraries linked inside (`zstd-sys` via zarrs,
  the BLAKE3 assembly) stay hidden. The **static library** exports all of them. A Geant4 app that also links its own
  zstd (ROOT does) can get a duplicate symbol or, worse, the wrong one silently. The static archive is therefore
  shipped only after a check proves that `nm -g` lists nothing but `tsra_*` symbols. The prototype
  measured both the problem and the fix (spike §6.2). As built, the archive exports about 90,400 global symbols,
  273 of them zstd's, and a consumer that also links its own `libzstd.a` fails with
  `multiple definition of 'FSE_versionNumber'`. A partial link (`ld -r --whole-archive`, 5.3 s) followed by
  `objcopy --keep-global-symbols=<the tsra_ list> --strip-debug` (3.9 s) leaves the 8 `tsra_*` globals, and the same
  consumer links and seals the same hash. The C symbols have default visibility, so `--localize-hidden` wouldn't
  touch them. #815 makes the localisation a build step and `nm -g` a check. The static archive ships only
  localised. A static link then needs the archive plus rustc's `--print native-static-libs` list
  (`-lgcc_s -lutil -lrt -lpthread -lm -ldl -lc` on x86-64 Linux) as `Libs.private`, and no libstdc++. Sizes
  (release, x86-64): the shared library is 49 MB (39 MB stripped), and the localised static archive is 151 MB.
- **pkg-config and CMake.** The package installs `lib/pkgconfig/tessera.pc` and
  `lib/cmake/Tessera/TesseraConfig.cmake` plus a version file, which defines the imported target `Tessera::tessera`.
  gPET-sim's CMake (`find_package(... CONFIG)` style, `g4ring/CMakeLists.txt:65`) then needs
  `find_package(Tessera 0.1 REQUIRED CONFIG)` and `target_link_libraries(${APP_NAME} PRIVATE Tessera::tessera)`.
- **nix.** A `packages.tessera-capi` output (crane, like `tessera-cli`, `flake.nix:284-287`, exported beside it in `packages`, `:424-429`) installs `lib/`,
  `include/tessera.h` and both config files. A flake check builds a small CMake C++ consumer against the package
  with nanoarrow, and runs the §7.4 C case and a schema-refusal case. That check is what proves the header, the
  pkg-config file, the CMake file and the ABI together.
- **cargo-dist.** The broad channel builds only `tessera-cli` today (`dist-workspace.toml:27-35`,
  `precise-builds = true`). Adding the bridge means adding `tessera-capi` to the dist build and packaging its
  libraries (cargo-dist's library packaging), with the header and the config files as included files. This is
  unverified against dist 0.32's behaviour. The packaging sub-issue starts with a probe, as ADR-0057 §4's did. One
  known obstacle goes into the probe: the `[dist]` table sets `features = ["static-hdf5"]` workspace-wide
  (`dist-workspace.toml:27`), and `tessera-capi` has no such feature, so a precise build of it would be refused.
  #816 scopes the feature to `tessera-cli` (per-package dist config) rather than adding an inert feature to the
  bridge. The
  dist targets are the four in `dist-workspace.toml:37-42`. The glibc floor of the Linux artifacts, and whether the
  cluster that runs gPET-sim uses nix, conda or modules, are **pending surveys**.
- **HDF5 and `static-hdf5`.** The bridge's graph has no HDF5 (§1, enforced by `capi-deps`), so neither the
  `static-hdf5` feature nor a system libhdf5 is involved in building or loading `libtessera`. That matters
  for these consumers in particular: gPET-sim links its own HDF5 C++ library (`g4ring/CMakeLists.txt:73`), and a
  second libhdf5 in the same process is a known source of crashes.

## §12 — C++ usage sketch: gPET-sim's multithreaded writer

The sketch replaces the per-thread `g4h5::H5Writer` shards with one `gpet-sim:g4ring-output` product per run: one
session with an `events` lane and a `pethits` lane, written from every worker. It's a sketch (its compiled form is
the §11 consumer check), and the names follow gPET-sim's classes.

```cpp
// SimulationManager.h: one session per run, shared by every worker. No per-thread writer, no h5writer_mutex.
#include <nanoarrow/nanoarrow.hpp>    // NANOARROW_THROW_NOT_OK
#include <tessera.h>

struct TsraRun { tsra_session* s = nullptr; tsra_table* pethits = nullptr; tsra_table* events = nullptr; };

static void check(int32_t rc) {                       // every tsra_ call; the message is thread-local (§5)
    if (rc != TSRA_OK) G4Exception("tessera", "TSRA", FatalException, tsra_last_error());
}

// Master thread, RunAction::BeginOfRunAction (before any worker starts its block of events).
void SimulationManager::OpenProduct(const G4Run*) {
    TSRA_ABI_CHECK();                                 // run time: header vs loaded library (§10)
    tsra_session_options o{sizeof o};
    o.stage_dir   = stageDir.c_str();                 // empty, disk-backed, never tmpfs (ADR-0026)
    o.schema_json = schemaJson.c_str();               // gpet-sim:g4ring-output: tables keyed by role (§8)
    o.product = "gpet-sim:g4ring-output";             // == the schema's "product"
    o.name = runName.c_str(); o.description = "gPET-sim g4ring run"; o.timestamp = runStartUtc.c_str();
    check(tsra_session_open(&o, &tsra.s));
    tsra_producer p{sizeof p, "gPET-sim", GPETSIM_VERSION, GPETSIM_GIT_COMMIT, "MorePET/gPET-sim", -1};
    check(tsra_session_set_producer(tsra.s, &p));
    check(tsra_session_set_generation(tsra.s, configJson.dump().c_str(), nullptr));

    const char* opt[] = {"nHits", "n_compton", "n_rayleigh"};   // the schema's optional columns this run writes
    // B = 1: one batch per event, whatever Geant4's task cut; stored event_id = Geant4 id + offset (§7.2)
    tsra_table_options t{sizeof t, /*first_seq*/ 0, /*B*/ 1, /*base*/ fEventIdOffset, opt,
                         recordScatterInfo ? 3u : 1u};
    check(tsra_table_open(tsra.s, "pethits", &t, &tsra.pethits));
    tsra_table_options e{sizeof e, /*first_seq*/ 0, /*B*/ 1, /*base*/ fEventIdOffset, nullptr, 0};
    check(tsra_table_open(tsra.s, "events", &e, &tsra.events));
}

// Worker thread, EventAction::EndOfEventAction, for EVERY event: pethits first, then events (the claiming
// contract's cross-lane order, §7.2). seq = Geant4 event id; an event with no rows still pushes its seq.
void SimulationManager::FlushEvent(const G4Event* ev) {
    const uint64_t seq = static_cast<uint64_t>(ev->GetEventID());
    ArrowArray batch;
    NANOARROW_THROW_NOT_OK(ArrowArrayInitFromSchema(&batch, PetHitSchema(), nullptr));
    NANOARROW_THROW_NOT_OK(ArrowArrayStartAppending(&batch));
    for (const auto& [key, h] : thread_prompt_hit_buffers[slot])   // std::map<(crystal, photon)>: a fixed order
        if (h.edep >= edepThreshold_keV) AppendPetHit(&batch, h);
    /* … the annihilation buffer the same way … */
    NANOARROW_THROW_NOT_OK(ArrowArrayFinishBuildingDefault(&batch, nullptr));
    check(tsra_push(tsra.pethits, seq, &batch, PetHitSchema()));   // batch moved, whatever rc says
    if (keep) check(tsra_push(tsra.events, seq, &eventBatch, EventSchema()));  // one row
    else      check(tsra_mark_done(tsra.events, seq));             // not kept: no row, seq still done
}
// A worker's exception handler that loses an event calls tsra_session_abort(tsra.s, what) (§7.2).

// Master thread, RunAction::EndOfRunAction (after every worker has finished its events, so no push is in flight).
void SimulationManager::CloseProduct(const G4Run*) {
    const int64_t n = paused ? eventsCompleted : numberOfEvents;    // close() proves no event was lost
    check(tsra_table_close(tsra.pethits, n));
    check(tsra_table_close(tsra.events, n));
    check(tsra_session_set_field(tsra.s, "events_run", std::to_string(eventsRun).c_str()));
    check(tsra_session_finish(tsra.s, (saveFileName + ".tsra").c_str(), nullptr));
    tsra_session_free(tsra.s);
}
```

What changes for gPET-sim:

- one output file per run instead of one per thread;
- no merge or re-sort for the readers;
- no HDF5 initialisation mutex;
- a product whose bytes are the same for the same seed, whatever the thread count.

The `pethits` sequence key `(event_id, hit_ordinal)` needs the `hit_ordinal` column the drafts propose (gap analysis
§4.3, survey question 4). Until g4ring writes it, the lane's key is `event_id` alone. That key isn't one of the
table's `keys`, so the check is non-decreasing (§7.2), and the within-event order rests on the `std::map` walk. That's a schema choice, and the survey settles it.

Pause and resume (`EventAction.cpp:62-70`) gives the completed ids `0 … k-1`. A paused run closes its lanes with
`expected_count = k`, and a resumed run is a second product with `first_seq = k`. Whether that pair should be one
collection (ADR-0049), or one product keyed `(run_id, event_id)` (gap analysis §4.3, survey question 3), is
**pending surveys**.

## §13 — Failure modes and limits

1. **A full disk in `stage_dir`** surfaces as `TSRA_E_IO` from the push, array write, close or finish that hit it,
   and poisons the session. The disk budget is the leaves being staged (a few per lane, with #806's lookahead, §7.3), plus the raw
   arrays waiting for `finish` (§2.2), plus the encoded leaves before their commit (the `push_table` model, `stream.rs:872-878` and `:911-916`). A tmpfs
   `stage_dir` turns all of it into unreclaimable RAM (ADR-0026's disk paragraph). The header's documentation of
   `stage_dir` says so. The bridge neither detects nor refuses tmpfs, because `/dev/shm` is the right place for a
   benchmark (ADR-0064 §8.1), and a warning channel that a success can't carry (§5) would be one more thing to
   ignore.
2. **A crashed producer leaves the staging directory behind.** It isn't resumable (§3). A later
   `tsra_session_open` on the same `stage_dir` refuses it as non-empty (§3), rather than letting
   `WriteSession::create` truncate it. The producer deletes the directory, or picks a new one per run.
3. **Stage 2 is Python.** The pairwise path and the coincidence writer are in geddf-analysis's Python. They need a
   *streaming* writer in `tessera-py`, which today has only the batch `Builder` (`tessera-py/src/lib.rs:281`). That
   is not this ABI's scope. It's a finding for #803 and the surveys, and the follow-up issue filed with this ADR
   tracks it.
4. **A batch is copied once by the bridge,** from the producer's Arrow buffers into `ColumnData`. After that it
   follows the writer's own path (staging, encode), as a Rust producer's does. The import itself is zero-copy.
   Removing the copy would need a borrowed `ColumnData` all the way to the staging write, and it would break §4's
   promise that `release` runs inside `push`. The prototype bounds the gain: with 4 or more producer threads the
   bridge was level with the native writer or faster, which is consistent with the copy running on the producers'
   threads in parallel, though the runs don't isolate the cause (§16).

## §14 — Alternatives rejected

- **Re-implementing the format in C++.** Explicitly ruled out by #799: a second writer is a second determinism
  surface that needs its own conformance proof.
- **One product per thread** (today's shard model, kept). Not reproducible (§Context), and it moves a merge into every
  reader, which is the work #803 is trying to remove.
- **A process boundary only** (Arrow IPC over a pipe, §2.1). Kept as a possible later channel, not the primary one.
- **Ordered lanes with sparse keys**, **spilling instead of back-pressure** and **sorting rows by value at the seal**
  (all §7.2).
- **One table per session** (an earlier revision of this ADR), with a multi-table product split into one product per
  table. The surveyed products are multi-table by contract (`gpet-sim:g4ring-output`, `geddf:coincidences`), and
  splitting them would move a join across products into every reader. Lanes (§7.3) give each table its own stream
  without one waiting for another.
- **One sequence for the whole product,** with tables ordered by schema declaration. A table waiting behind an
  earlier-declared one would be staged in full until that one closes. Lanes have no such cost.
- **Several processes writing one product.** Out of scope. It needs an exploded on-disk layout that processes can
  append to independently, and that's a later design.
- **A second scheduler in the bridge.** The ordering stage reuses `StreamWriter`'s credit window and its ordered
  commit pattern (§7.3), so there is one implementation of "bounded and in order".
- **A C++ wrapper library as the ABI.** A C++ ABI breaks across compilers and standard libraries. A header-only C++
  RAII wrapper over the C ABI (`tessera.hpp`) is a convenience that adds no ABI, and the docs sub-issue ships one.
- **Validation as an opt-in.** A schema-less bridge write would seal whatever a producer sent, with no contract. The
  owner's #801 makes validation the point.

## §15 — Implementation plan

Each item is a sub-issue of #799, each a PR of about 400 lines or less, landing in this order now that the owner
has accepted the ADR (2026-11-13). The prerequisites already have issues: #454 (gating `hdf5`/`dicom` in
`tessera-ingest`, so the arrow lane is HDF5-free) and PR #786 (`tessera-identity`). The estimates were redone for
this revision. The items that bundled several deliverables (#810, #812, #814) are split, and the open-ended table
writer is the gap analysis's #806 (#808, filed earlier with the same scope, is closed as its duplicate).

| # | Issue | What | Size | Depends on |
| --- | --- | --- | --- | --- |
| 1 | #806 | `tessera-io`: open-ended partitioned table, one leaf of lookahead, byte-identical to `TableMultiBlockSink` (§7.3) | ≤ 400 | — |
| 2 | #832 | `tessera-io`: lanes on `StreamWriter`/`WriteSession`, sealed in lane order, per-lane integrity check (§7.3) | ≤ 400 | — |
| 3 | #809 | `tessera-core`: the column contract keyed by role (`tables`/`arrays`, `optional`, `sequence_key`, `keys`, `references`), corpus-neutral, parsing the six drafts (§8, G1, G3) | ≤ 400 | — |
| 4 | #830 | `tessera-core`: strict schemas (`deny_unknown_fields`, `schema_format`) and role-matched requirements (§8, G2, G6) | ≤ 300 | #809 |
| 5 | #810 | `tessera-io`: validate-before-pack seal, session `with_schema` and `with_ingest_transform` (§8, §9, G4, G5) | ≤ 300 | #809 |
| 6 | #831 | `tessera-io`: `StreamWriter::finish_with` (header patch at the seal) and `abort` (§3) | ≤ 250 | #810 |
| 7 | #812 | `tessera-io`: the sequenced table input per lane: the `CreditWindow` gate, ordered release, window, horizon, duplicate, gap, abort, finite stall (§7.2, §7.3) | ≤ 400 | #806, #832 |
| 8 | #833 | `tessera-io`: sequence-key checks (lexicographic order, block range) (§7.2) | ≤ 250 | #809, #812 |
| 9 | #834 | `tessera-io` tests: the #802 proof suite with the Geant4-modulo mock and its negatives (§7.4) | ≤ 400 | #833 |
| 10 | #811 | `tessera-capi` skeleton: crate, handles, lifecycle, status codes, last error, `catch_unwind` and poisoning, the panic=abort guard, ABI version, cbindgen header and its drift check, the `capi-deps` check (§3, §5, §10) | ≤ 400 | #454 |
| 11 | #813 | table lanes over the ABI: `tsra_table_open`/`push`/`mark_done`/`close`, per-push schema check, Arrow import, clean lane, nullability, canonicalisation (§2.1, §7, §8) | ≤ 400 | #809, #810, #811, #812, #833 |
| 12 | #814 | arrays over the ABI, staged raw and encoded at `finish` in role order (§2.2) | ≤ 250 | #811, #813, #832 |
| 13 | #835 | metadata, producer, generation, `aux/provenance.json`, identity, `finish`/`free` (§3, §9) | ≤ 350 | #814, #831, PR #786 |
| 14 | #815 | packaging: `.pc` and CMake config, `packages.tessera-capi`, the CMake C++ consumer check with nanoarrow, the static archive's localisation and symbol check (§11) | ≤ 400 | #835 |
| 15 | #816 | cargo-dist library artifacts (probe first) and the `abidiff` gate from the second release on (§10, §11) | ≤ 300 | #815 |
| 16 | #817 | docs: a "Writing from C/C++" chapter as docs-as-tests, `tessera.hpp`, the FEATURE-MATRIX row (§12) | ≤ 400 | #815 |

**Merge order:** the table's `#` column.

- #806, #832 and #809 are independent of each other and can land in any order.
- #830 and #810 both follow #809.
- #811 waits for #454, and #835 for PR #786.
- #816 and #817 are last and independent of each other.

Every PR rebases onto `dev` and lands alone.

**Deferred, with issues:** #836 (categorical code tables, G13), #780 (checking `references`), #768 (native
fixed-size lists, G10/G11; the producers expand tuples to scalar columns meanwhile), #805 (a C read side for
gPET-sim2listmode, G26) and #818 (a streaming writer in `tessera-py` for the Python Stage 2 path, §13.3).

## §16 — Pending surveys and what is unmeasured

**Pending surveys** (the owner's two consumer surveys settle these; nothing above guesses them): where a C++ product's
schema lives (§8); whether any C++ producer writes volumes larger than RAM (§2.2) or carries patient identity (§9);
whether a producer exists with no dense order key or that can't claim sequence numbers in increasing order (§7.2),
or with a run that can't be repeated (§3); the glibc floor and the package channel on the clusters (§11); whether a
paused/resumed pair is one collection or one product keyed `(run_id, event_id)` (§12, gap analysis question 3);
whether g4ring writes `hit_ordinal` (§12, question 4); whether enums get code tables (#836, question 5); typical rows
per event and run sizes, which set the window and the staging disk budget (§7.2, §13.1, question 9).

**Prototype evidence.** A parallel spike built a throwaway end-to-end prototype (`spike/800-capi-prototype`, PR
#821, `docs/spikes/800-capi-prototype.md` at `3e6f0d5`). What it measured, and where this ADR uses it:

- **No bytes of its own.** A C++ program built with nanoarrow against the installed header and pkg-config file seals
  the `content_hash` that native `StreamWriter::push_table` seals from the same rows, and `tessera verify` and the
  Rust reader accept it value for value (spike §2). An unknown row count, staged through `TableMultiBlockSink`,
  seals the same bytes as a declared one.
- **#802.** With the dense key, N ∈ {1, 4, 16} C++ threads under shuffled schedules seal one hash in 9 of 9 runs, the
  same hash as the Rust test. Keyed by arrival, the same 9 runs give 9 distinct hashes, and a mutation of the
  reorder buffer that ignores the key fails the test (exit 101). A single thread that works out of order breaks it
  too, so one producer thread is no substitute for the key (spike §3).
- **The blocking window and its precondition.** The prototype's reorder buffer blocks a push more than a window
  ahead. That holds memory bounded and can't hang when sequence numbers are claimed in increasing order: claimed
  keys through a window of 4 sealed one hash for N ∈ {1, 4, 16}. Under an arbitrary assignment it deadlocks (spike
  §3.1). The owner's steer adopts the blocking window. §7.2 states the precondition as ABI contract, adds the
  `max_ahead` refusal, wake-on-failure, abort and a finite stall deadline (mandatory, by the owner's 2026-11-13
  ruling), so a fault is a typed error and not a hang, and lets a producer without the precondition turn blocking
  off (`refuse_instead_of_block = 1`).
- **Throughput.** 50 M rows on 8 dedicated ksb cores, 4 encode workers, staging on `/dev/shm`: native `push_table`
  ran at a median of 8.89 M rows/s, and the C ABI at 8.25 M (1 producer thread, −7 %, inside that configuration's own
  17 % spread), 8.52 M (4) and 9.23 M (16). All 12 runs sealed one `content_hash`, and the writer, not the bridge, is
  the bottleneck (spike §5). `catch_unwind` adds no work on the success path. It is in every measured call, though
  not isolated.
- **Copies.** The import is zero-copy, and the bridge adds exactly one copy, into `ColumnData` (§4, §13.4).
- **Linking.** No HDF5 or DICOM, and the static-archive symbol leak and its fix are measured (§11).

**Still unmeasured** (each one is an acceptance criterion of the sub-issue named): the window's peak batches and
bytes, and the time producers spend blocked, under real `G4TaskRunManager` block sizes with a lagging thread, which
sets the default `window` (#834); the staged disk use of a table without a row count, with #806's lookahead
(#806); the per-call FFI cost separated from the batch work at small batch sizes (#813); and the writer's own heap under the bridge, which
the bench's pre-built input hides (#815). The spike also found that a ksb job can't evaluate this flake, because
crane's `fetchGit` of the pinned vortex fork fails there. That's tooling friction, not a design point, so the
consumer check (#815) runs in CI.

## Consequences

- C++ producers get reproducible multi-table `.tsra` products, one per run, written by tessera's own writer from any
  number of threads and validated against a registered schema, with no HDF5 of ours in their process.
- `tessera-io` gains a table writer that streams leaves without a known row count, lanes, and a thread-safe sequenced
  input on the `StreamWriter` built from its own credit window and ordered commit. Streaming ingest and Rust producers can
  use both.
- `ProductSchema` grows from "which fields and how many blocks" into the #801 drafts' column contract, with no corpus
  regeneration, and becomes strict (#830), so no reader silently validates less than a file declares.
- The workspace gains a 7th published crate and a C ABI with its own version, which every later change has to
  respect. That's the price of the owner's "through our writer, not a re-implementation", and the §10 gates are
  what keep it payable.
