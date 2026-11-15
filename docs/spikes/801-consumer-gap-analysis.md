# Spike — what the #799 C++ consumers need from tessera's format and writer (#801, #802, #803)

> **Status:** gap analysis, for the design rounds of #800 (C ABI bridge), #801 (a schema per product), #802
> (thread-independent bytes) and #803 (reader ports). No product code changes here. The owner's two consumer surveys
> supply the firm facts and **supersede** anything below that they contradict. This note is the tessera side: what our
> format and writer must offer, measured against what the consumers write and read today.
>
> **Pins.** Tessera at `origin/dev` `6a30780`. Consumer repos at `main`: MorePET/gPET-sim `7fa1ad3`, MorePET/g4h5
> `a548054`, MorePET/gPET-sim2listmode `3d6d566`, MorePET/geddf-analysis `d6cd785`, MorePET/gPET-pipeline `cbced11`,
> MorePET/gPET-readout `00f5316`. Geant4 at `Geant4/geant4` `f3d5293` (the GitHub mirror's default branch; gPET-sim's
> own comments cite 11.3.1). A consumer citation `repo/path:L` is a line in that repo at that pin. A bare
> `tessera/…:L` or `docs/…:L` is in this repository.
>
> **Method.** The consumer files were read through `gh api` (no clone of the large repos). Line numbers were
> spot-checked against the raw files, and every Geant4 and ordering claim in §4 was re-read at its line. Facts marked
> *(inferred)* are deductions from the code, not something a file states.

## Contents

1. [Summary](#1-summary)
2. [Inventory](#2-inventory-what-each-consumer-writes-and-reads-today)
3. [Gap table](#3-gap-table)
4. [Ordering (#802)](#4-ordering-802)
5. [Draft product schemas (#801)](#5-draft-product-schemas-801)
6. [Reader ports (#803)](#6-reader-ports-803)
7. [Questions for the surveys](#7-questions-for-the-surveys)
8. [Issues filed by this spike](#8-issues-filed-by-this-spike)

## 1. Summary

- **Every consumer product is flat, columnar and fixed-width**, with no variable-length lists in any of them. Each
  maps onto today's `ColumnData` set (`tessera/crates/tessera-io/src/table.rs:82-125`) once fixed-size tuples are
  expanded to scalar columns and NaN or zero sentinels become NULLs. The dtype gaps are narrow: no native
  `FixedSizeList` in the writer (#768) and no categorical or enum descriptor on a column.
- **The schema registry can't express any consumer product's columns.** A `ProductSchema` declares metadata fields
  and block *roles*. It has no column contract, and its block check ignores the role altogether
  (`tessera/crates/tessera-core/src/schema.rs:219-222`). The streaming `WriteSession` can't embed a non-built-in
  schema, and nothing validates one at seal. This is the core of #801 (gaps G1-G6).
- **Ordering has a clean answer from Geant4 itself.** With the task run manager and
  `seedOncePerCommunication = 0`, event *i* is seeded with seed pair *i*, drawn in event-id order on the master, so
  an event's content does not depend on which worker ran it or how many workers exist. The on-disk interleaving does.
  The deterministic key is **`event_id`** within one run, and **`(run_id, event_id)`** for a product that spans
  batches, because gPET-sim stores the id as `uint32 + offset`. A `(thread, sequence)` pair isn't stable across thread
  counts (§4).
- **Tessera's writer is single-producer, and it needs the row count up front.** `StreamWriter` commits in push order
  from one `&mut self` caller (`tessera/crates/tessera-io/src/stream.rs:34-38`), and `push_table` refuses an unknown
  `spec.rows` (ADR-0064 §5.3). Geant4 workers neither push in order nor know their row counts. #802 needs the owner's
  dense `seq`, as `seq = event_id / B` for a fixed, recorded `B`: Geant4's own block size follows the thread count
  (§4.4). #806 needs open-ended tables.
- **One reader is C++** (gPET-sim2listmode). #800 is writer-only, so the port can't start without a read-side C ABI
  (#805). The Python readers need row-range reads (#807), and a correct nullable read: today's Python returns NULL
  columns with their validity mask appended (#804, found by this spike).
- **Port effort per repo:** gPET-sim2listmode **S** (once #805 exists), gPET-readout **S**, gPET-pipeline **M**,
  geddf-analysis **M** to read and **L** to also write. gPET-reco is an unsurveyed downstream reader of
  `coincidences.h5` (§6).

## 2. Inventory: what each consumer writes and reads today

### 2.1 Overview

| Consumer | Writes | Reads | Producer threads → order | Language |
| --- | --- | --- | --- | --- |
| gPET-sim (g4ring) | `output_thread<N>.h5` (`events`, `pethits`), `phase_space_thread<N>.h5` (Stage 1) | crystal map, GDML, phase space (Stage 2) | one file per Geant4 worker; event blocks interleave by scheduling | C++ (Geant4 11, g4h5) |
| gPET-sim2listmode | `listmode_singles.h5` | g4ring shards, activity maps | **one** writer thread, time-sorted, thread-count-independent | C++ + Python runner |
| geddf-analysis | `coincidences.h5`, pairwise JSON | `listmode_singles.h5`, crystal map (CSV) | `multiprocessing.Pool`, results consumed in task order | Python (h5py) |
| gPET-readout | nothing persistent | g4ring `data/pethits` | — | Python (h5py) |
| gPET-pipeline | stage manifests (JSON) | post-conditions on every stage's output | — (orchestrates subprocesses) | Python |

None of the six repos references tessera, `.tsra`, Arrow or Parquet today. MorePET/visionx-rs is the one consumer that
already writes `.tsra`, through `tessera-core`/`tessera-io` on `dev`: `ProductBuilder`, `blob_ref_streaming` and
`pack_streaming_verified` (`visionx-rs/crates/visionx-tsra/src/pack.rs:5-8,140-204`). It is the model for a Rust-side
writer.

### 2.2 gPET-sim / g4ring (Geant4 MT)

**Files.** In MT, each worker's `RunAction::BeginOfRunAction` creates `<base>_thread<N>.h5`, with `N` the worker's
`G4GetThreadId()` (`gPET-sim/g4ring/src/SimulationManager.cpp:254-297`). Nothing merges them in-process. The
separate `g4ring/scripts/merge_mt_h5files.py` concatenates them in thread-id order, **without sorting**
(`merge_mt_h5files.py:50-55,140-167`). Rows are packed HDF5 compounds (`g4ring/include/OutputSchema.h:79-109`), written
by g4h5 with shuffle + deflate (level 1 by default) in 1-D unlimited datasets, chunked and buffered at 100 000 rows
(`g4h5/include/g4h5/H5Writer.h:29-32`, `g4h5/src/H5Writer.cpp:78-86`, `g4ring/include/SimulationManager.h:324-325`).

**`/data/events`**: one row per kept event (every event when `saveAllEvents`), 34 B packed
(`gPET-sim/docs/schemas/g4ring_output_schema.md:73`).

| column | dtype | unit | null / sentinel |
| --- | --- | --- | --- |
| `event_id` | u64 | 1 | `uint32(g4 id) + eventIdOffset` (`SimulationManager.h:701-703`) |
| `n_pethits` | u16 | 1 | — |
| `source_{x,y,z}_mm` | f32 | mm | — |
| `annih_{x,y,z}_mm` | f32 | mm | **NaN** when the event has no annihilation (`g4ring/schema/g4ring-output.yaml:32-35`) |

**`/data/pethits`**: one row per (event, crystal, source photon) with `edep_keV ≥ edepThreshold_keV`, 54 B packed
(`g4ring-output.yaml:36-49`, `SimulationManager.h:165-184`).

| column | dtype | unit | null / sentinel |
| --- | --- | --- | --- |
| `event_id` | u64 | 1 | foreign key → `events.event_id` |
| `hit_type` | u16 | 1 | enum 0 prompt, 1/2 annihilation photon, 4 pre; 3 reserved (`SimulationManager.h:139-140`) |
| `crystal_id` | u32 | 1 | foreign key → crystal map (GDML copy number) |
| `edep_keV` | f32 | keV | — |
| `time_ps` | f64 | ps | earliest deposit |
| `nHits` | u16 | 1 | an optional member: absent from some files |
| `n_compton`, `n_rayleigh` | u8 | 1 | **0 = "not recorded"** unless `recordScatterInfo` (`g4ring-output.yaml:47-49`) |
| `pos_{x,y,z}_mm`, `first_{x,y,z}_mm` | f32 | mm | — |

**Stage 1** (`phase_space_thread<N>.h5`): `/data/phase_space_particles` holds 52 B rows of `event_id` u64,
`source_photon_id` i16 (an enum whose encoding is version-dependent,
`docs/schemas/phase_space_format_versions.md:13-15`), `pdg_code` i32, `pos_*` f32 mm, `dir_*` f32 (a unit vector),
`kinetic_energy_keV` f32, `time_ps` f64, and optional `n_compton`/`n_rayleigh` u8. Stage 1 also writes its own
`/data/events`: `event_id`, `n_particles` u32, `source_*` and `annih_*` (NaN), stored uncompressed
(`SimulationManager.h:226-271`, `SimulationManager.cpp:335-346`).

**Metadata.**

- Root attributes: `schema_name`, `schema_version` "3.0", `simulationMode`, `numberOfEvents` (the same in every
  shard, never summed), `events_run` (per shard, so it **is** summed), `saveAllEvents`, `edepThreshold_keV`,
  `scanner_axial_axis` (`OutputSchema.h:222-236`, `g4ring_output_schema.md:88-89`).
- `/metadata/general`: `seed`, `runId` (a user batch index), `numberOfThreads`, `useMultithreading`, plus echoes of
  every input and output path (`SimulationManager.cpp:1832-1850`).
- `/provenance`: producer, version, git commit and dirty flag, nix store path, Geant4 version
  (`OutputSchema.h:259-266`).
- `/provenance/parameters`: the effective configuration. `/provenance/inputs`: a sha256 per input file
  (`g4ring-output.yaml:96-130`).
- All of it is written at shard close through a plain `H5::H5File`, because g4h5 exposes only `int` file attributes
  (`SimulationManager.cpp:428-435`).

**Volumes.** Campaigns of 200 k, 1 M and 1 B events are on record
(`docs/records/g4_mt_vs_parallel_investigation.md:16,40-47`). At 34 B per event and 54 B per pethit, a 1 B-event
normal run holds up to 34 GB of `events` before compression (all of it with `saveAllEvents`), plus 54 B per pethit
*(inferred: the pethit count per event depends on the geometry and the source, and no file states it)*.

**Companions outside the shards:** `dose_map.h5`, written by the master and not migrated to 3.0
(`RunAction.cpp:105-115`); per-event RNG replay files; a checkpoint manifest. They are out of scope here until the
surveys ask for them.

**Input that's also a product: the crystal map** (HDF5 v2). It holds `crystal_id` u32 (or i32), `center_{x,y,z}` and
`dir_{x,y,z}` as f32 or f64, and `volume_name` (vlen str), with root attributes for crystal sizes, axial axis and
`n_crystals` (`docs/schemas/crystal_map_format.md:14-36`). It is the target of `pethits.crystal_id`. geddf-analysis
reads a CSV form of it.

### 2.3 gPET-sim2listmode

Paths below are under `ListMode-Libraries/DataManagers/` unless they name another directory.

**Reads** g4ring `events` + `pethits`, from either the 3.0 or the legacy member names (`event_number`, `volume_id`,
`edep`, `time`), sniffing the optional members per file
(`ListMode-Libraries/DataManagers/include/ListModeStructs.h:242-313`). It also reads root attributes and an optional 3-D
float64 activity map with `origin`/`spacing` (`src/ActivityMap.cpp:10-34`).

- Whole datasets are read into memory, one file at a time with the next one prefetched, through a parallel chunk
  inflater (`src/H5ParallelRead.cpp:94-303`, under a global `Hdf5Mutex` because libhdf5 isn't thread-safe).
- Rows are **re-sorted by `event_id` after reading**, because "merged multi-thread files are not sorted"
  (`src/SimulationDataManager.cpp:343-351`). Events and hits are then joined with a sorted cursor; orphans are counted.
- Shards are combined by a pre-pass that reads every `event_id` and makes the ids unique within a source:
  `offset[i+1] = offset[i] + max(event_id of file i) + 1` (`SimulationDataManager.cpp:156-278`, `README_DEV.md:53`).

**Writes** `listmode_singles.h5` (schema `gpet-sim2listmode/listmode-singles` 3.0). It is one 1-D resizable compound
dataset with one row per single, 64 B packed, or 88 B with both opt-in groups
(`docs/schemas/listmode_singles_schema.md:51-55`).

| column | dtype | unit | null / sentinel |
| --- | --- | --- | --- |
| `edep_keV` | f32 | keV | — |
| `crystal_id` | u32 | 1 | — |
| `hit_type` | u16 | 1 | enum |
| `nHits` | u16 | 1 | **0 = not recorded** |
| `absolute_time_ps`, `relative_time_ps` | f64 | ps | — |
| `source_event_id` | u64 | 1 | remapped per source |
| `source_type` | u16 | 1 | 0 intrinsic, 1..N tracer |
| `source_{x,y,z}_mm` | f32 | mm | — |
| `n_compton`, `n_rayleigh` | u8 | 1 | 0 = not recorded |
| `pos_{x,y,z}_mm` | f32 | mm | **0.0 = not recorded** (`has_hit_position` says which) |
| `annih_*`, `first_*` (opt-in) | f32 | mm | **NaN** |

Columns per `ListModeStructs.h:370-417`. Sentinel flags are per source (`schema/listmode-singles.yaml:93-96`).

- **Groups** (`src/ListModeDataManager.cpp:1121-1273`): `/metadata` (per-source counts, `has_*` flags, acceptance,
  `source_files/{paths, event_id_offsets}`, `decay_channels`), `/statistics`, and `/provenance` (git, nix path, the
  full effective `config_json`, inputs with sha256).
- **Chunking:** 100 k rows, deflate 6, and `H5Pset_obj_track_times(false)`, so identical runs are byte-identical
  (`ListModeDataManager.cpp:30,825-836`).
- **Order:** non-decreasing `absolute_time_ps`, enforced at flush (`ListModeDataManager.cpp:865-916`).
- **One writer thread.** The event loop is a sequential inhomogeneous Poisson process. Threads are used only to read
  inputs, so "the output is identical for every thread count" (`README_DEV.md:55`,
  `docs/parallelization_future_work.md:8-12`).

### 2.4 geddf-analysis (Stage 2.5: coincidences and the pairwise path)

**Reads** `listmode_singles.h5` through one chokepoint, `read_singles` (`geddf_analysis/listmode.py:166`), which maps
the 3.0 names back to pre-3.0 ones (`LISTMODE_RENAMES`, `listmode.py:38`). Other read sites:

- `process_sim_listmode.py:1231,1280,1445`: open the file. The first is a per-worker handle inside the pool.
- `process_sim_listmode.py:310`: `_chunk_row_ranges` picks `tm_ms`-aligned row ranges, one per worker.
- `coincidence_finder.py:302-319`: `load_singles` reads everything and re-sorts by smeared time.
- `pairwise.py:152` and `run_stats.py:29-98`: read metadata and attributes only.

**Writes** `coincidences.h5` (schema `geddf-analysis/coincidences` 3.0,
`geddf_analysis/schema/coincidences.yaml:21-109`). Every dataset is resizable, with one row per event, coincidence,
single or millisecond, and **fixed-shape tuples per row** (no variable lengths):

| dataset | row layout | notes |
| --- | --- | --- |
| `proc_data/events_2p` | `tm_ms` u32, `dt_clk` i64, `en_keV` f32×2, `ax`,`tx` u16×2, `vtx_mm` f32×3 | |
| `proc_data/events_3p` | as 2p with ×3 tuples, plus `lt_ns`, `lt_corr_ns` f32 | |
| `proc_data/coin_2p` / `coin_3p` | `tm_ms` u32, `en_keV` u16×n, `ax`,`tx` u16×n, `dt_clk` i64 (×2 for 3p) | |
| `proc_data/{coin,event}_labels_{2p,3p}` | u8 | row-aligned with `coin_*` / `events_*` |
| `proc_data/true_vtx_{2p,3p}_mm` | f32 (N,3) | **NaN** for non-TRUE events |
| `proc_data/hit_pos_2p_mm` | f32 (N,2,3) | iff `has_hit_pos`; NaN for a missing single |
| `proc_data/coin_size_dist` | i64 (M,2) | written once, at the end |
| `raw_data/time_markers` | `tm_ms` u32, `idx` u64, `length_ms` u8, five u32 counts | synthesised at the end |
| `raw_data/singles` (opt-in) | `tm_ms` u32, `en_keV`,`ax`,`tx` u16, `td_clk` i64 | |
| `config/lyso_map` | f64 (n_tx, n_ax, 4) | an array, not a table |

Writers: `_create_output_datasets` and `_append` (`process_sim_listmode.py:850-974`). Root attributes are
`has_hit_pos` and `has_singles`. `/config` carries 22 typed parameters and `/provenance` the inputs with their
sha256. HDF5 chunks are 8192 rows.

**Order.** The pool's results are consumed in task order (`_ordered_pool_results`, `process_sim_listmode.py:1237-1260`),
so the bytes don't depend on the worker count. Chunks are independent by construction: a `td = 65535` sentinel closes
each chunk's last single (`process_sim_listmode.py:382-383`).

**Pairwise record:** a single JSON object (`geddf_analysis/pairwise.py:198-199`, `docs/schemas/pairwise_record.md`). It
holds counts by class for doubles and triples, the windows and cuts, `n_singles`, `n_511`, `n_ng`, `singles_span_s`,
window diagnostics and provenance.

**Volumes:** 17-55 MB per coincidences run at 175 k-521 k singles
(`gPET-pipeline/docs/examples/03_water_block_listmode_bulk.md:31-39`).

### 2.5 gPET-readout and gPET-pipeline

- **gPET-readout** only reads: `read_pethits` over `data/pethits` (`src/gpet_readout/g4ring_io.py:116-133`), with
  call sites in `ring.py:244-249` and `composite.py:103`. Each stacks `pos_*` into an (N,3) array and sorts by
  `event_id`. It writes no per-run product.
- **gPET-pipeline** transforms no data. It runs the stage binaries and reopens their outputs for post-conditions:
  - `gpet_pipeline/stages/listmode.py:177-194`, `stages/coincidences.py:107-146` and `stages/reco.py:231-249`;
  - an HDF5 validator driven by `docs/schemas.md`;
  - `s3://` transfer through the `aws` CLI (`stages/storage.py:1-59`).
- Stage 4 (gPET-reco) reads `coincidences.h5` directly (`docs/stages/4_reconstruction.md:22-29`). Streaming and bulk
  produce the same `listmode_singles.h5`; the difference is an input mode (`docs/stages/2_listmode.md:60-75`).

## 3. Gap table

"exists" means the feature is on `dev` and covers the consumer's need. "partial" means part of it exists, or it exists
only on one path, for example ingest but not the writer. "needed" means nothing covers it. The issue column is where
the work belongs. Tessera citations are at `6a30780`.

### 3.1 Schema registry and validation (#801)

| # | Need | Status | Tessera today (file:line) | Issue |
| --- | --- | --- | --- | --- |
| G1 | A per-product **column contract** (names, dtypes, units, nullability per table role) | needed | `ProductSchema` has only `fields` + `blocks` (`tessera-core/src/schema.rs:225-243`); columns live only in each sealed block's `TableSpec` (`tessera-core/src/block/table.rs:139-149`) | #801 |
| G2 | Block requirements matched **by role** | needed | `BlockRequirement::matches` checks only `kind` (`schema.rs:220-222`), so one `events` table satisfies a required `pethits` | #801 |
| G3 | Consumer schemas as **data**, not engine code | partial | built-ins are Rust literals (`schema.rs:436-835`); a caller-supplied schema embeds through `ProductBuilder::with_schema` (`tessera-core/src/product.rs:113`), and an embedded schema wins at validation (`schema.rs:333-338`); no loadable registry file | #801 |
| G4 | The **streaming** writer embeds a non-built-in schema | needed | `WriteSession` has `with_field`, `with_producer` and `with_generation` but no `with_schema` (`tessera-io/src/write.rs:638-690`); `seal` embeds only a built-in (`product.rs:192-196`); Python has no `with_schema` either (#567) | #801, #567 |
| G5 | **Validation at the bridge** (every batch against the declared schema) and at seal | needed | `ProductBuilder::seal` checks the leaf and partition rules, never the schema (`product.rs:175-215`); validation runs on read (`validate_manifest`, `schema.rs:333`); the ingest engine validates after its seal, but against the **built-in** registry, so it ignores an embedded custom schema (`tessera-ingest/src/engine.rs:202-210`) | #801 |
| G6 | An unknown key in a schema is an **error**, not dropped | needed | no `deny_unknown_fields` on `ProductSchema` / `FieldSpec` (`schema.rs:69-71,227-228`): a schema that adds a column contract would deserialise in an older binary with the contract silently ignored | #801 |
| G7 | Namespaced product names (`gpet-sim:g4ring-output`) | exists | string names, open-world, `vendor:product` convention (`docs/adr/0003-schema-identification.md:15-21`) | — |
| G8 | Required / recommended metadata, units, PHI tier | exists | `FieldSpec` (`schema.rs:69-111`); simulation data is `public` throughout | — |

### 3.2 Dtypes and nesting

| # | Need | Status | Tessera today (file:line) | Issue |
| --- | --- | --- | --- | --- |
| G9 | `u8…u64`, `i8…i64`, `f32`, `f64`, `bool`, `str` columns | exists | `ColumnData` (`tessera-io/src/table.rs:82-100`); codes `i1…f8, b1, str` (`table.rs:155-169`) | — |
| G10 | **Fixed-size tuples** (`f32[3]` vertices, `u16[2]` channels) | partial | generic ingest expands `FixedSizeList<scalar, N ≤ 8>` into `name.0…` and records it (`tessera-ingest/src/arrow_table.rs:30,106-113`); the **writer** (`ColumnData`) has no list type, so the bridge must expand or #768 must land | #768, #800 |
| G11 | **Nested** tuples (`hit_pos_2p_mm` f32 (N,2,3)) | partial | a list of lists is outside the ingest map (`arrow_table.rs:304`, scalar child only); the producer must flatten to 6 columns | #768 |
| G12 | NULL instead of NaN / 0 sentinels | exists | `ColumnData::Nullable` + `Column.nullable` (`table.rs:101-125`, `block/table.rs:33-44`); whether to convert sentinels is the consumers' call (§7) | — |
| G13 | **Enum / categorical** columns (`hit_type`, `source_type`, `source_photon_id`, labels) | needed | `Column` has no vocabulary or code-table slot (`block/table.rs:12-71`); `table.rs:97-99` points at "a categorical descriptor — a schema choice" that doesn't exist; ingest materialises dictionaries to values (`arrow_table.rs:29,51-56`) | #801 |
| G14 | Struct columns | partial | ingest flattens to dotted names (`arrow_table.rs:222`); no consumer needs a struct once G10 is solved | — |
| G15 | Picosecond times as **f64** (`time_ps`, `absolute_time_ps`) | partial | storable as `f8` + `unit = "ps"`; ADR-0046 §2 says event timestamps are "integer ticks + a scale, never floats" (`docs/adr/0046-time-model.md:57`); the consumers' choice stands until the surveys decide (§7) | #801 |
| G16 | Dense N-D arrays (`lyso_map`, activity map) | exists | array block, ADR-0023 | — |
| G17 | Per-row variable-length lists | not needed | no consumer has one (§2) | — |

### 3.3 Writer, ordering and partitioning (#800, #802)

| # | Need | Status | Tessera today (file:line) | Issue |
| --- | --- | --- | --- | --- |
| G18 | Many producer threads → **one** product, bytes independent of scheduling | needed | `StreamWriter` commits in **push order** from one `&mut self` caller (`tessera-io/src/stream.rs:34-38,845-852`); worker-count invariance is proven (`stream.rs:1605-1650`, #677 test (c)), producer-thread invariance isn't | #802 |
| G19 | Tables whose **row count is known only at close** | needed | `push_table` needs `spec.rows` up front and refuses a short or long input (ADR-0064 §5.3, `stream.rs:927-960`) | #806 |
| G20 | Bounded memory for 10⁹-row tables | exists | `<table>/NNNN` leaves at `BLOCK_ROWS`, staged on disk, with the ADR-0064 §8 bound (`stream.rs:880-925`) | — |
| G21 | Partition by **batch / run** | exists | one product per batch inside a collection (ADR-0049); the key question is §4 | — |
| G22 | Row-aligned side tables (labels next to `coin_2p`) | partial | ADR-0062 `RowAlignment` / positional zip designed, not built (#667); the drafts fold labels into their table instead (§5) | #667 |
| G23 | Declared **keys and foreign keys** (`pethits.event_id → events`, `crystal_id → crystal map`) | needed | no keyed-reference vocabulary anywhere; `Source` edges are provenance (`tessera-core/src/provenance.rs:15`) | #780 |

### 3.4 Reading (#803)

| # | Need | Status | Tessera today (file:line) | Issue |
| --- | --- | --- | --- | --- |
| G24 | One logical name over the leaves | exists | `Reader::table_group` / `logical_table` (`tessera-io/src/container.rs:445`, `tessera-io/src/multiblock.rs:392`) | — |
| G25 | Column projection, `take`, `locate`, block pruning (Rust) | exists | `LogicalTableView` (`multiblock.rs:39-323`) | — |
| G26 | **C++** reader | needed | no read side in the C ABI; #800 is a writer | #805 |
| G27 | Python whole-table and whole-column reads | exists | `column`, `table_dict`, `table_arrow`, `table` (`tessera-py/python/tessera/__init__.py:123-152`) | — |
| G28 | Python **row-range / take** reads (geddf's per-worker ranges) | needed | none in Python | #807 |
| G29 | Python **nullable** read | needed (defect) | the buffer is `values ‖ mask` (`table.rs:320-324`); `_ndarray` doesn't split it (`__init__.py:43-69`) | #804 |
| G30 | Streaming bounded-memory query | partial | ADR-0062 Accepted, PR1 not built (#666); `tessera sql` materialises (`tessera-cli/src/sql.rs:131-161`) | #666 |
| G31 | `s3://` reads | exists | `open_url` (`tessera-io/src/cloud.rs:194`) | — |

### 3.5 Provenance, aux and identity

| # | Need | Status | Tessera today (file:line) | Issue |
| --- | --- | --- | --- | --- |
| G32 | Run configuration as a sealed recipe (`/provenance/parameters`, `config_json`) | exists | `Generation { config, config_ref }` (`tessera-core/src/provenance.rs:184-193`); `WriteSession::with_generation` (`write.rs:682`); `requires_generation` (`schema.rs:242`) | — |
| G33 | Producer identity (tool, version, git commit, nix path) | exists | `with_producer` (`write.rs:672`), ADR-0058 §1 | — |
| G34 | Input files with sha256 | exists | `Source` edges with `content_hash` (`write.rs:662`); a sha256 must become the edge's digest or a `generation.config` entry, since edges hash with blake3 | — |
| G35 | Facts that must **not** enter the seal (`numberOfThreads`, path echoes, wall-clock) | exists | `aux/` members outside the seal (`tessera-io/src/container.rs:28-40,728`, ADR-0042); see §4.4 for why the thread count belongs there | — |
| G36 | Identity inheritance (singles inherit the run's `seed`, `run_id`) | exists | `FieldSpec.inherit` + `inherit_identity` (`schema.rs:110`, ADR-0058 §5); the drafts don't set it (§7) | — |
| G37 | Encrypted identity envelope | not needed | ADR-0047 carries PHI on `aux/`; simulation products hold none | — |

ADR-0047 (which the brief names for aux and identity) is the crypto-shred envelope. The aux carrier it rides on is
ADR-0042, and seal-time identity is ADR-0058 §5. The rows above cite all three.

## 4. Ordering (#802)

### 4.1 How gPET-sim's workers emit events today

1. **Run manager.** In MT, gPET-sim creates the Geant4 task run manager, `G4RunManagerType::Tasking`, to avoid the
   `G4MTBarrier` hang of `G4MTRunManager` (`gPET-sim/g4ring/g4ring.cpp:298-302`,
   `docs/records/migration_g4taskrunmanager.md`). The thread count comes from the config or `G4NTHREADS`
   (`g4ring.cpp:304-325`).
2. **Event ids are handed out in blocks, under a mutex.** `G4TaskRunManager::SetUpNEvents` assigns a task the next
   `numberOfEventsPerTask` ids, starting at the shared counter `numberOfEventProcessed`, and advances the counter
   (`Geant4/geant4 source/run/src/G4TaskRunManager.cc:629-661`). The block size is `eventModulo`: `sqrt(nEvents)` by
   default, capped at `nEvents / grainSize`, where `grainSize` is the **thread-pool size**
   (`G4TaskRunManager.cc:293-333`). The cap also applies to a value set with `SetEventModulo`: it is reduced with only
   a `JustWarning` (`Run10035`, `:311-320`), and `G4FORCE_EVENTS_PER_TASK` overrides both (`:323-327`). gPET-sim's
   Stage 2 calls `SetEventModulo`, aligned to the Stage 1 tasks (`g4ring.cpp:614-626`), but with a value that is
   itself capped at `nEvents / nThreads`, or 0 to keep Geant4's default (`g4ring/include/PhaseSpaceTasks.h:32-46`). So
   the block size follows the thread count once `nThreads > sqrt(nEvents)`, in both stages (§4.4).
3. **Each worker appends its blocks to its own file**, in event order within a block. Which blocks a worker gets
   depends on who takes the mutex first. A shard holds non-contiguous blocks, and the merge script concatenates the
   shards in thread order, **unsorted** (`merge_mt_h5files.py:140-167`). That is why gPET-sim2listmode and
   gPET-readout re-sort by `event_id` (§2.3, §2.5).
4. **Within an event, hit order is fixed.** Hits are aggregated in a `std::map` keyed by `(crystal, source photon)`,
   one map for prompt and one for annihilation hits (`g4ring/include/SimulationManager.h:305-312`). An event runs on
   one thread from one seed pair, so its rows come out in a fixed order *(inferred: the flush walks the maps in key
   order)*.

### 4.2 Is the order stable across thread counts in Geant4 itself?

**The content per event is, and the interleaving is not.**

- **Seeding.** With `seedOncePerCommunication = 0` (the default, `source/run/src/G4MTRunManager.cc:53`), `SetUpNEvents`
  pushes **one seed pair per event** from the master's pre-filled `G4RNGHelper`. The pairs are taken in order as
  `GetSeed(nSeedsPerEvent * nSeedsUsed)` and `+1`, and `nSeedsUsed` is incremented once per event, under the same
  mutex that assigns the event ids (`G4TaskRunManager.cc:643-656`).
- **Event *i* gets seed pair *i***, whatever the block size and whichever worker runs it. The worker reseeds its
  engine from that queue before **every** event (`source/run/src/G4WorkerTaskRunManager.cc:233-273`). This assumes
  the application doesn't override the master's seeding (`InitializeSeeds`). g4ring doesn't: a code search of
  MorePET/gPET-sim finds neither `InitializeSeeds` nor `SetSeedOncePerCommunication`.
- **gPET-sim relies on this already.** Its MT resume skips `2 × next_event_id` master flats, so that "resumed event i
  gets the pair of event next_event_id + i", and it refuses to resume with any other `seedOncePerCommunication`
  (`g4ring.cpp:630-648`).
- **Modes 1 and 2 break it.** Under mode 1 (once per worker run) or mode 2 (once per `eventModulo` block), the seed
  follows the task partition, and `grainSize` follows the thread count (`G4TaskRunManager.cc:295,647`). gPET-sim checks
  the mode only on the resume path. A fresh run in mode 1 or 2 would produce per-event content that depends on the
  thread count *(inferred from the code above)*.
- **The interleaving** (which worker writes which block, and when) depends on scheduling and on `grainSize`, so the
  per-thread files and their concatenation differ between thread counts and between runs.

### 4.3 Which key gives a deterministic order

| candidate | deterministic across thread counts? | verdict |
| --- | --- | --- |
| `event_id` | yes, within one `/run/beamOn`: unique, contiguous `[0, N)` + offset, and the content per id is fixed (§4.2) | **the key within a run** |
| `(run_id, event_id)` | yes | **the key for a product that spans batches** |
| `(thread, sequence)` | no: the block-to-thread mapping is scheduling-dependent, and thread ids are pool slots reused across runs *(per `migration_g4taskrunmanager.md`)* | rejected |

**Why `event_id` alone isn't enough across batches.**

- The on-disk id is `static_cast<uint32_t>(g4EventId) + fEventIdOffset`, with an `unsigned int` offset
  (`SimulationManager.h:698-703`), and a Geant4 event id is a `G4int`. So one run holds at most 2³¹ events, and the
  stored id wraps at 2³² across offset batches.
- The streaming example plans for 10¹⁰+ events in batches
  (`gPET-pipeline/docs/examples/04_water_block_listmode_streaming.md:22,168`).
- gPET-sim already records `runId` as a user batch index (`SimulationManager.cpp:1834`). So a cross-batch product needs
  `(run_id, event_id)`, or a `u64` id that doesn't wrap.

**Rows per event.** `pethits` and `phase_space_particles` have several rows per event, so their total order is
`(event_id, ordinal within the event)`. g4ring writes no ordinal today. The drafts add a proposed `hit_ordinal` and
`particle_ordinal` (§5). The alternative is a stable sort on `event_id` alone, which is deterministic only if each
event's rows arrive in their fixed order (§4.1 item 4).

**sim2listmode's sorts are not stable.** It re-sorts the merged events and hits with `std::sort` on `event_id` alone
(`ListMode-Libraries/DataManagers/src/SimulationDataManager.cpp:344-351`), and the output buffer with `std::sort` on
`absolute_time_ps` alone (`ListModeDataManager.cpp:907`, `operator<` at `include/ListModeStructs.h:379-381`).
`std::sort` leaves the order of equal keys unspecified. The input is a merged MT file whose order follows scheduling
(§4.1 item 3), so the order of one event's hits, and of hits with equal times, can differ between runs of the same
simulation *(inferred: same rows, so it is visible only on ties)*. Its byte-identity claim (§2.3) holds for an
identical input file. A tessera port should use `std::stable_sort` with a full key, for example
`(absolute_time_ps, event_id, hit_ordinal)`.

**Single-producer consumers don't need a key.** gPET-sim2listmode (one writer thread, time-sorted) and geddf-analysis
(ordered pool) don't depend on their own thread count. sim2listmode's sorts still need a full key (above). For them the draft tables say `"order": "producer"`.

### 4.4 What #802 has to provide

The owner's steer on #802 (2026-11-11, full text on #800) fixes the writer's shape: each batch carries a dense,
producer-assigned `seq`. The bridge commits batches in `seq` order through StreamWriter, behind its bounded credit
window, and an empty block pushes an empty batch or calls `mark_done(seq)`. This section maps the analysis above onto
that shape. Items 2 and 7 are proposed **amendments** to the steer, which the owner decides.

1. **The key is `seq = event_id / B`.** It replaces round 1's segments keyed by a declared id range `[lo, hi)`, and
   the mapping is exact: the segment `[lo, hi)` is the batch `seq = lo / B` with `hi = lo + B`, and only the run's
   last batch is shorter.
   - `event_id` is Geant4's id within the run (`G4Event::GetEventID`), before g4ring adds its offset (§4.3).
   - Density is round 1's "an empty segment still has to be pushed". A block can have no kept `events` and no
     `pethits` rows, and it still pushes an empty batch or `mark_done`.
   - The in-order commit replaces round 1's reorder buffer keyed by `lo`, and the credit window is round 1's
     backpressure bound (item 4).
   - For a product that spans `/run/beamOn` batches (`(run_id, event_id)`, §4.3), #802 chooses between a per-run
     `seq` base and one session per run.
2. **Contradiction, and amendment to the steer's item 3: Geant4's block is not a fixed size.** The steer suggests
   `seq` = the event-block index. Geant4's block size is `T = eventModulo = min(sqrt(N), N / grainSize)`, where
   `grainSize` is the thread-pool size (`G4TaskRunManager.cc:293-333`). Once the thread count exceeds `sqrt(N)`, `T`
   depends on it. For `N = 64`, `T` is 8 at 1 and 4 threads and 4 at 16.
   - `SetEventModulo` does not pin `T` unconditionally. Geant4 still caps it at `N / grainSize` with only a
     `JustWarning` (`:311-320`), and `G4FORCE_EVENTS_PER_TASK` overrides it (`:323-327`).
   - g4ring's Stage 2 pins a value that depends on the thread count (§4.1 item 2, `PhaseSpaceTasks.h:32-46`).
   - So with `seq` = Geant4's block index, the steer's guarantee "same (seq → batch) mapping, same bytes" holds, but
     the mapping is not the same across thread counts.

   **Amendment.** `seq = event_id / B`, for a block size `B` that the producer fixes per product, independently of
   the thread count. `B` is recorded in the session metadata.
   - Recording `B` in the seal is safe because `B` is fixed. It does not bring the thread count into the bytes,
     unlike `numberOfThreads` (item 5).
   - The producer computes `seq` from the event id, never from Geant4's task index, and flushes its buffer whenever
     `seq` changes.
   - **Alignment.** A worker can push a whole batch only if one Geant4 task covers it. Tasks start at multiples of `T`
     (`SetUpNEvents` advances the counter by `T`, `:629-661`), so the condition is `T % B == 0`.
   - The producer pins it with `SetEventModulo(B)`, with `B ≤ N / pool size`. It reads back the effective size,
     `GetNumberOfEventsPerTask()` (`G4TaskRunManager.hh:100`), and refuses the run if `T % B != 0`. *(Inferred: where
     to read it back; the size is computed inside `InitializeEventLoop`, `G4TaskRunManager.cc:359`.)*
   - **If alignment breaks anyway**, two workers push parts of one `seq`. That is the steer's duplicate-`seq` case,
     so it must be an error, never a merge. Then a misaligned run fails loudly instead of sealing different bytes.
3. **Amendment to the steer's item 2: the partition.** Whether `B` reaches the bytes depends on how the bridge turns
   batches into leaves.
   - If each batch becomes its own leaves, every batch boundary is a leaf boundary. `B` is then part of the product's
     bytes, and item 2's fixed, recorded `B` is the whole guarantee.
   - If the bridge cuts leaves at `BLOCK_ROWS` over the `seq`-ordered rows, `B` drops out of the bytes, and only the
     row order remains. `push_table` already re-chunks its input "so the leaves don't depend on how the input was
     chunked" (`tessera/crates/tessera-io/src/stream.rs:865-869`, ADR-0064 §5.1). A producer could then change `B`
     without changing the product.
   - The second option needs #806, because `push_table` takes `spec.rows` up front. Density and alignment (item 2)
     are needed either way.
4. **The window.** Round 1's bound on out-of-order work is the steer's credit window, which blocks a thread that is
   too far ahead. One condition matters for Geant4: the due `seq` must always be admitted. Otherwise a window smaller
   than the pool deadlocks, with every slot held by later batches while the worker holding the due block waits.
   This is a new term in the ADR-0064 §8 bound.
5. **Keep the thread count out of the seal.** gPET-sim writes `numberOfThreads` into `/metadata/general`. In a
   tessera product it belongs in `aux/provenance.json` (ADR-0042), or the sealed bytes change with the thread count by
   construction. The same goes for the output and log paths (G35).
6. **Single-producer consumers.** gPET-sim2listmode and geddf-analysis push from one thread in their own order
   (`"order": "producer"`). For them `seq` is just the push index. sim2listmode's unstable sorts (§4.3) still need a
   full key before its order is independent of its input file.
7. **Amendment to the steer's item 5: the test that catches item 2.** The steer's test pushes a fixed (seq → batch)
   mapping in shuffled order from 1, 4 and 16 threads. Because the mapping is fixed, it cannot see a mapping that
   follows the thread count. The amended test runs the same `N` events at 1, 4 and 16 threads, with blocks handed
   out as Geant4's **default** modulo does. That means real Geant4, or a mock of `ComputeNumberOfTasks` and
   `SetUpNEvents` with `T = min(sqrt(N), N / threads)`. Take `N = 64`, so `T` is 8, 8 and 4. It asserts three things:
   - **`B` pinned** (`B = 4`, `seq = event_id / B`, `T` a multiple of `B`): an identical `content_hash` at 1, 4 and 16
     threads.
   - **`B` not pinned** (`seq` = Geant4's task index): under item 3's first option, the `content_hash` differs
     between 1 and 16 threads. This arm fails if the test is blind. Under the second option, equality is correct,
     and the misaligned arm carries the check.
   - **Misaligned** (`B = 8` with `T = 4` at 16 threads): rejected as a duplicate `seq`, never sealed.

   The steer's own assertions stay: peak in-flight ≤ the window, and a gap at seal is an error. The test is the
   producer-side twin of `content_hash_is_identical_across_workers_and_windows` (`stream.rs:1605`).

## 5. Draft product schemas (#801)

The drafts live in [`801-consumer-schemas/`](801-consumer-schemas/). They are **not** in the live registry: they wait
for the surveys. Each product has two files, both in shapes tessera already serialises:

- `<product>.schema.json` is a `tessera_core::schema::ProductSchema` exactly as serde writes it (`schema.rs:225-243`).
  It deserialises today and could be embedded through `ProductBuilder::with_schema`.
- `<product>.tables.json` is the **proposed** column contract that #801 adds (G1). For each table role it holds a list
  of `tessera_core::block::table::Column` objects (the existing shape, `block/table.rs:12-71`), plus three proposed
  keys: `sequence_key` (#802), and `keys` and `references` (#780).

[`generate.py`](801-consumer-schemas/generate.py) is the single source of the JSON, and
[`check.py`](801-consumer-schemas/check.py) checks it against the serde shapes. The check covers required keys, enum
values, the field dtypes `DType::as_str` + `string`/`coded`/`json`, the column codes, unique column names, and that
every key or reference names a declared column and role. It is a Python mirror of the Rust types, not a Rust
round-trip: the brief named no build slot.

| product | blocks (role: kind) | from |
| --- | --- | --- |
| `gpet-sim:g4ring-output` | `events`: table, `pethits`: table (min 0) | §2.2 |
| `gpet-sim:g4ring-phase-space` | `events`, `phase_space_particles`: table | §2.2 |
| `gpet-sim:crystal-map` | `crystals`: table | §2.2 |
| `gpet-sim2listmode:listmode-singles` | `listmode_singles`, `source_files`, `decay_channels` (min 0), `hit_type_counts` (min 0) | §2.3 |
| `geddf:coincidences` | `events_2p`, `events_3p`, `coin_2p`, `coin_3p`, `time_markers`, `coin_size_dist`, `singles` (min 0): table; `lyso_map`: array | §2.4 |
| `geddf:pairwise-record` | none (metadata only) | §2.4 |

**Modelling choices** (each one open to the surveys, §7):

- **Namespaced names**, not the built-in `sim` or `listmode`. `sim` has a single `output` role, and claiming
  `listmode` would require its `coincidence_mode` and patient fields (`schema.rs:617-680`).
- **Sentinels become NULLs.** NaN vertices, `0`-means-not-recorded counts and `0.0` positions with a `has_*` flag all
  become nullable columns (G12). The flags then become redundant.
- **Fixed tuples become scalar columns.** Vectors are named the way the consumers name them (`pos_x_mm`, …) and
  tuples the way ADR-0056 expands a list (`en_keV.0`, `en_keV.1`). The (N,2,3) `hit_pos_2p_mm` becomes
  `hit_pos.{0,1}.{x,y,z}_mm` (G10, G11).
- **Row-aligned side datasets fold into their table.** Labels, `true_vtx` and `hit_pos` become columns of
  `events_*` / `coin_*`, rather than waiting for `RowAlignment` (G22). This assumes the alignment that
  `coincidences.yaml:55-65` describes.
- **Per-run counts and the sealed recipe.** Per-run counts that today are HDF5 attributes are typed metadata fields.
  Free-form configuration is the sealed `generation.config`, and every draft but the crystal map sets
  `requires_generation: true` (G32).
- **The thread count and path echoes are absent** from the fields on purpose (§4.4 item 4).
- **Times stay `f8` ps**, as written today (G15).
- **Optional members are optional columns.** g4ring's `nHits`, `n_compton` and `n_rayleigh` can be absent from a file
  altogether (`g4ring-output.yaml:47-49`). That differs from a per-row NULL, and the column contract (G1) needs an
  "optional column" flag for it. The drafts list these columns, and model the per-row "not recorded" case as
  nullable.

## 6. Reader ports (#803)

| repo | read call sites today | tessera replacement | effort |
| --- | --- | --- | --- |
| gPET-sim2listmode (C++) | `SimulationDataManager.cpp:149,163,175` (pre-pass: open, `events`, `event_id` column); `:288,300,305,312,321` (open, read `events` + `pethits`); `ActivityMap.cpp:10-28`; `H5ParallelRead.cpp` (a decode accelerator, deleted by the port) | #805's C read side: `logical_table("events" / "pethits")`, `event_id` projection for the pre-pass, whole-table reads as Arrow arrays; array read for the activity map | **S** for the port itself (one file holds > 90 % of the surface, and `FileData` stays the seam); **blocked on #805** |
| gPET-readout (Python) | `g4ring_io.py:116-133` (`read_pethits`); callers `ring.py:244-249`, `composite.py:103` | `tessera.open(p).table_dict("pethits")`, or `column("pethits", "pos_x_mm")` ×3 | **S**: one module, read-only |
| geddf-analysis (Python), read | `listmode.py:166` (`read_singles`); `process_sim_listmode.py:1231,1280,1445` (opens), `:310` (row ranges); `coincidence_finder.py:319`; `pairwise.py:152`, `run_stats.py:29-98` (metadata) | `table` / `column` for whole reads; the manifest's `metadata` for facts; **row ranges need #807**; NULLs need #804 | **M** |
| geddf-analysis (Python), write | `process_sim_listmode.py:850-974,1500-1667`; `coincidences_file.py:64-152` | Python `Builder.add_table` per block; nullable writes, `with_schema` (#567) and an open-ended row count (#806) are missing from Python | **L** (ten datasets; the writer surface isn't there yet) |
| gPET-pipeline (Python) | `stages/listmode.py:177-194`, `stages/coincidences.py:107-146`, `stages/reco.py:231-249`; the HDF5 YAML validator (`docs/schemas.md`) | `tessera.open` + `manifest()`; the validator becomes `tessera schema` against the embedded schema (G3-G5) | **M**: the validator is new code |
| gPET-reco | reads `coincidences.h5` directly (`gPET-pipeline/docs/stages/4_reconstruction.md:22-29`) | `table("events_2p")` and friends | **unknown**: not inventoried, so it needs to be in the survey |

**Order of the ports.** A reader can't move before its producer does. gPET-sim → gPET-sim2listmode and gPET-readout
comes first, then gPET-sim2listmode → geddf-analysis, then geddf → gPET-reco and the pipeline post-conditions. Each
hand-off needs either both sides ported in one wave, or the producer writing both formats for one release. The
pipeline's chain contract (`gPET-pipeline/docs/chain-contract.md`) is where to record which, per stage.

## 7. Questions for the surveys

1. **Sentinels → NULL.** Do the consumers accept NULLs where they now write NaN, `0` or `0.0` plus a `has_*` flag? Does
   every reader cope with a masked column? Python needs #804 for that.
2. **f64 picoseconds or integer ticks.** ADR-0046 §2 prefers integer ticks plus a scale. Is f64 ps precision enough
   for the longest acquisitions, given that an f64 has 1 ps resolution up to about 9 × 10³ s?
3. **Batches.** Is one product per `/run/beamOn` (with a collection over the batches) the unit, or do the consumers
   want one product across batches, keyed `(run_id, event_id)` (§4.3)?
4. **A within-event ordinal.** Can g4ring write `hit_ordinal` / `particle_ordinal`? Or does the writer stable-sort
   rows on `event_id` alone, relying on the fixed per-event order (§4.3)?
5. **Enums.** Should `hit_type`, `source_type`, `source_photon_id` and the label codes get a code table in the schema
   (G13)? Is the phase-space encoding version a field or part of the product version?
6. **Inheritance.** Should `listmode-singles` and `coincidences` inherit `seed` / `run_id` from their parent (G36)?
7. **Companion products.** Are `dose_map`, the activity map and the CSV crystal map products in their own right?
8. **gPET-reco.** What does it read, and is it in #803's scope?
9. **Volumes.** What are the row counts and pethits per event of the largest planned campaign? They size the §4.4
   credit window and #806.

## 8. Issues filed by this spike

- #804: `bug(py)`: a nullable table column reads back with its validity mask appended to the values (G29).
- #805: `feature(capi)`: a read side for C++ consumers (G26).
- #806: `feature(io)`: stream a partitioned table whose row count is known only at close (G19).
- #807: `feature(py)`: row-range, take and per-leaf reads of a logical table (G28).

Every other gap maps to an open issue: #801 (G1-G6, G13, G15), #802 (G18), #768 (G10, G11), #780 (G23), #567 (G4,
Python), #666 (G30) and #667 (G22).
