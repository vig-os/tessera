# Spike — the C ABI bridge, prototyped end to end (#800)

> **Status:** evidence for ADR-0065 (`docs/800-capi-bridge-adr`, Proposed). The prototype is
> `tessera/crates/tessera-capi` on `spike/800-capi-prototype` (PR #821). It answers design questions and
> is not the shipped API: the ADR decides that, and its sub-issues #808–#817 build it. Code references are
> at `origin/dev` `6a30780` unless they say otherwise.

## Verdicts

1. **The bridge works end to end, and it adds no byte of its own.** A C++ program, built with nanoarrow
   against the installed `tessera.h` and `tessera-capi.pc`, writes a listmode-like table that
   `tessera verify` accepts and that the Rust reader returns value for value. Its `content_hash` equals the
   one native `StreamWriter::push_table` seals from the same rows (§2).
2. **#802 is solved by an explicit dense sequence key.** With the key, N ∈ {1, 4, 16} C++ producer threads
   under shuffled schedules seal one hash (9 of 9 runs). The same hash comes out of the Rust test and the C++
   program. With the key taken in arrival order, 9 runs give 9 distinct hashes. A mutation that ignores the key
   fails the test (§3). **The prototype's blocking window holds memory bounded only under a precondition:**
   keys claimed in increasing order and pushed in increasing order per thread, which is G4TaskRunManager's
   shape. An arbitrary assignment can deadlock it, and that is the evidence for ADR-0065 §7.2's spill (§3.1).
3. **The import is zero-copy and the bridge copies once.** `from_ffi` wraps the producer's buffers (a test
   compares the pointers). Converting them to `ColumnData` is the one copy the bridge adds, because the
   writer's input type owns a `Vec` per column. Everything after that is the writer's own path, the same as
   for a Rust producer (§4).
4. **The bridge costs no throughput that matters.** Over 50 M rows on 8 dedicated ksb cores with 4 encode
   workers, staging on `/dev/shm`, native `push_table` runs at a median of 8.89 M rows/s. C++ through the C ABI
   runs at 8.25 M rows/s with 1 producer thread (−7 %), 8.52 M with 4, and 9.23 M with 16 (+4 %). Each
   configuration spreads 6–17 % run to run. All 12 runs seal one `content_hash` (§5).
5. **A C++ consumer links nothing of HDF5 or DICOM.** The prototype depends on `tessera-io` only. A static
   link needs `libtessera_capi.a` plus `-lgcc_s -lutil -lrt -lpthread -lm -ldl -lc`, and no libstdc++
   (§6). **The static archive must be symbol-localised before it ships:** as built, it exports about 90,000
   global symbols, among them zstd's 273. A consumer that links its own `libzstd.a` fails with
   `multiple definition of 'FSE_versionNumber'`. After `ld -r` and `objcopy --keep-global-symbols`, only the
   8 `tsra_*` symbols remain, and the same consumer links and seals the same hash (§6.2).
6. **Two of ADR-0065's choices need a decision the evidence sharpens** (§7): where hazard H4's NaN
   canonicalisation lives, and whether the bridge depends on `tessera-ingest` before #454 lands.

## §1 — What was built

| Piece | Where | What it shows |
| --- | --- | --- |
| C ABI | `src/lib.rs` | session → table (declared by an `ArrowSchema`) → `push(seq, ArrowArray)` → end → seal |
| Error model | `src/error.rs` | `catch_unwind` at every entry point, a `TsraStatus`, a thread-local last error |
| Type map | `src/arrow.rs` | the clean lane only; nullability from the declaration |
| Ordering | `src/order.rs` | a reorder buffer over the dense key, bounded by a batch-count window |
| Header | `include/tessera.h` | cbindgen output, with the Arrow C structs behind `ARROW_C_DATA_INTERFACE` |
| C++ programs | `cpp/` | `capi_smoke`, `capi_threads`, `capi_bench`; CMake + pkg-config + nanoarrow |
| Rust tests | `tests/` | 18 tests that call the `extern "C"` functions as C does |
| Nix | `flake.nix` | `packages.capi`, `packages.nanoarrow`, the `capi-smoke` check |

The API in one view (the full contract is in the header):

```c
TsraSessionOptions so = { sizeof so, "listmode", "run-42", "…", "2026-11-11T00:00:00Z", /*workers*/ 4 };
tsra_session_open("/scratch/stage", &so, &session);
TsraTableOptions to = { sizeof to, rows /* or TSRA_ROWS_UNKNOWN */, /*reorder_window*/ 0 };
tsra_table_begin(session, "events", &schema, &to, &table);
tsra_table_push(table, seq, &array, &schema);   /* any thread; takes ownership of `array` */
tsra_table_end(table);
tsra_session_seal(session, "run-42.tsra", hash, sizeof hash);
```

Ownership follows the C Data Interface's move rule. `tsra_table_push` moves the `ArrowArray` out on
**every** path, success or failure (the caller's struct is left with `release == NULL`), and the producer's
`release` runs exactly once, before the call returns. `capi_smoke` wraps every array's `release` in a
counter: 23 pushes, among them a refused mismatched batch and a refused duplicate, gave 23 calls. Schemas
are borrowed.

## §2 — Correctness

| Check | Result |
| --- | --- |
| `cargo test -p tessera-capi --release` | 18 passed, exit 0 (3 unit, 12 roundtrip, 3 threads) |
| `columns_read_back_are_the_columns_pushed` | 150,000 rows in batches of 10,007, off the row-group grid; every value equal, float bits included |
| `the_bytes_are_those_of_the_native_writer` | the C ABI hash equals native `push_table`'s on the same rows |
| `an_unknown_row_count_seals_the_same_bytes` | `TSRA_ROWS_UNKNOWN` (staged through `TableMultiBlockSink`) gives the same hash as a declared count |
| `capi_smoke` (shared and static link, known and unknown rows) | one hash over all 4 runs: `blake3:9c64bd23…` |
| `readback <product> <dumps>` | "200000 rows, 5 columns identical to what C++ pushed", exit 0, all 4 runs |
| `tessera verify` | exit 0 on each of the 4 products (locally; the `capi-smoke` flake check repeats it) |

The error paths are tested as statuses, never as crashes. Every case passes: a NULL argument
(`INVALID_ARGUMENT`), an options struct that is too short, an already-released array, an unsupported type
(Timestamp), a nullable `str` column, a non-struct schema (`SCHEMA`), a batch of the wrong shape or type, a NULL
in a non-nullable column (`SCHEMA`, and the array is still released), a duplicate key (`ORDER`), a gap at the
end (`ORDER`, and the session is poisoned, so the seal returns `STATE` and writes no file), too few or too many
rows (`WRITER`), a second table while one is open (`STATE`), and a hash buffer that is too small.

## §3 — Determinism across producer threads (#802)

Every batch carries a dense `u64` key: `0, 1, 2, …` over the whole table, each exactly once. The reorder
buffer releases key `k` to the writer once every key below it has been released. It admits the keys
`next .. next + window`, and a push of a later key blocks until the window moves (§3.1).

`capi_threads` (C++, `std::thread`, 48 batches of 3,001 rows): each thread takes batch indices from a seeded
`std::shuffle` of the batch list, and yields a random 0–3 times before each push. These runs set the window to
the batch count, so nothing blocks and the test isolates the ordering.

```text
reference (N=1, seed 0)  blake3:7a629f96d41018f8c6e3326dfc91912b521030b306a245c464d93de5c0e04367
explicit N=1  seed 1..3  7a629f96… == (×3)
explicit N=4  seed 1..3  7a629f96… == (×3)
explicit N=16 seed 1..3  7a629f96… == (×3)
arrival  N=1  seed 1..3  6f0748af… 1f55e678… d0f733aa…
arrival  N=4  seed 1..3  f446ab0b… 25d15090… ba28767e…
arrival  N=16 seed 1..3  13c402c2… 8ffb2fe8… 7b78c6d6…
arrival order: 9 distinct hashes over 9 runs
```

The Rust test `tests/threads.rs` does the same with Rust threads, and it seals the same reference hash,
`7a629f96…`. So the C++ ABI path and the Rust path agree on the bytes.

**What breaks without the key, shown on the test.** The run below mutated `Reorder::push` to key each batch by
its arrival (`let seq = st.next + st.pending.len()`), and nothing else:

```text
test the_hash_does_not_depend_on_producer_threads_or_their_scheduling ... FAILED
assertion `left == right` failed: a shuffled single thread equals an in-order write
  left: "blake3:8bbe0794…"   right: "blake3:7a629f96…"
test result: FAILED. 0 passed; 2 failed            (cargo test exit 101)
```

Even a single thread breaks it, once it processes its work out of order. So "one producer thread" is not a
substitute for the key.

### §3.1 The window, and the precondition it costs

A blocking window bounds the buffer to `window` batches, but it is safe only when **the key `next` is never
held by a thread that is itself blocked**. That holds when keys are claimed in increasing order (a shared counter,
or `G4TaskRunManager`'s consecutive event blocks) and each thread pushes its own keys in increasing order: the
thread that claimed `next` has nothing smaller still unpushed, so it isn't waiting. It does **not** hold for an
arbitrary assignment. Suppose every producer holds a key at least `window` past `next`, while `next` itself is
still unclaimed or held by one of them. Then every push waits and nothing ever releases them. A single thread
that pushes key `window` before key 0 deadlocks the same way. The window can't tell the two cases apart,
because it can't know how many producers exist. (An independent review caught this. The first revision's tests
used 48 batches under the default window of 64, so the window never engaged.)

The claimed runs exercise it. Threads claim keys from a shared counter, so 200 batches of 997 rows pass through a
window of 4. In Rust, `claimed_keys_pass_a_small_window_without_deadlock` asserts that pushes waited, so the
test isn't vacuous. In C++:

```text
claimed  reference (N=1)  blake3:98d880ed… (window 4, 200 batches)
claimed  N=4  seed 1..2   98d880ed… == (×2)
claimed  N=16 seed 1..2   98d880ed… == (×2)
```

**For the ADR:** this is the evidence for §7.2's RAM credit with spill. Spilling early batches instead of
blocking removes the precondition, at the cost of disk. If the bridge ships a blocking window instead, the
precondition is ABI contract, and it must be stated where producers read it.

## §4 — Copies

| Step | Copies | Evidence |
| --- | --- | --- |
| `ArrowArray` → `ArrayData` (`from_ffi`) | 0 | `the_import_is_zero_copy`: the imported buffer is the producer's pointer |
| `validate_full` | 0 (it reads) | O(1) per primitive column, O(n) UTF-8 validation for `str` |
| `ArrayData` → `ColumnData` | **1** | `values().to_vec()`: `ColumnData` owns a `Vec<T>` (`tessera-io/src/table.rs:82`) |
| `ColumnData` → staged row-group file | the writer's | `push_table`'s staging, the same for a Rust producer |
| staged file → encoder → Vortex `Buffer` | the writer's | `ColumnData::to_vortex` copies (`table.rs:578-588`) |

**So the boundary is zero-copy, and the bridge adds exactly one copy.** That copy is also where the
producer's memory is given back: the array is released before `push` returns, so a producer can't hold an
arrow buffer hostage in the writer's staging. Removing the copy would mean a borrowed `ColumnData` (an
`arrow_buffer::ScalarBuffer` variant) all the way to the staging write. That's a `tessera-io` change whose
gain §5 bounds.

## §5 — Numbers

**Setup.** ksb job `tsra800-bench-1`: 8 dedicated cores (`--exclusive-cpus`, cpuset 4–11), 16 GiB, staging and
output on `/dev/shm` (tmpfs, 16 GiB). The workload is 50,000,000 rows of the listmode-like schema: 24.125 B per
row raw (8 + 4 + 4 + 4 + 4 plus a validity bit), 1.206 GB in all. The writer runs 4 encode workers and a
declared row count, which gives 12 leaves of `BLOCK_ROWS`. The binaries are release builds from `9854a32`.
**Every input is built before the clock starts:** `capi_bench` builds all 763 nanoarrow batches of 65,536
rows, and `native_bench` builds all `TableData` row-groups. So the timing covers only the bridge and the writer,
from the first push to the seal's return. There are 3 repetitions, interleaved native / 1 / 4 / 16 producers.

| Producer | Runs (s) | Median rows/s | vs native | Peak RSS |
| --- | --- | --- | --- | --- |
| native `push_table` | 5.411 · 5.623 · 5.725 | 8.89 M (214 MB/s raw) | 1.00 | 1,214 MB |
| C ABI, 1 thread | 6.228 · 6.063 · 5.295 | 8.25 M | 0.93 | 1,178 MB |
| C ABI, 4 threads | 5.871 · 6.097 · 5.855 | 8.52 M | 0.96 | 1,182 MB |
| C ABI, 16 threads | 5.820 · 5.361 · 5.415 | 9.23 M | 1.04 | 1,195 MB |

- **One product.** All 12 runs seal `blake3:22d9783d…`. The product is 204.5 MB, 5.9× smaller than the raw
  input. The C ABI file is 41 B longer than the native one because its manifest carries a different product
  name and description, which is also why their `manifest_hash`es differ. The block bytes are identical.
- **Overhead.** With one producer the bridge is at most 7 % slower at the median, which is inside that
  configuration's own 17 % spread (5.30–6.23 s). With 4 or more producers, the copy into `ColumnData` (§4)
  runs on the producers' threads in parallel, and the bridge is level with native or faster. That fits the
  native baseline doing nothing in parallel before `push_table`, but the runs don't isolate the cause. The writer is the bottleneck either way: about
  210 MB/s of raw input into Vortex/pcodec with 4 workers.
- **RSS** is dominated by the pre-built input (1.206 GB) in both programs, so the writer's own heap (bounded by
  ADR-0064 §8, measured in `tests/staged_job_memory.rs`) isn't visible here. The bridge adds no buffer that
  shows up. It releases each batch before `push` returns, and the reorder window holds at most 64 batches
  (4.2 M rows) only when producers run ahead.
- **Per batch:** 763 batches in about 6 s is about 8 ms a batch, against the import, validation and copy of
  1.58 MB. The per-call FFI cost (`catch_unwind`, schema import, reorder lock) isn't separable from that at
  this batch size.

## §6 — Packaging and linking

### §6.1 What a consumer links

| | Shared | Static |
| --- | --- | --- |
| Link line | `pkg-config --libs tessera-capi` = `-ltessera_capi` | `libtessera_capi.a` + `-lgcc_s -lutil -lrt -lpthread -lm -ldl -lc` |
| Exported symbols | **8**, all `tsra_*` (`nm -D --defined-only`) | ~90,400 globals as built; **8** once localised (§6.2) |
| Size (release, x86-64) | 49 MB, 39 MB stripped | 174 MB, 151 MB localised and debug-stripped; `capi_smoke` 74 MB |
| HDF5 / DICOM | none | none |
| libstdc++ | not needed by the library | not needed by the library |

`Libs.private` is rustc's `--print native-static-libs`, verbatim. The `capi-smoke` check builds both link
modes with CMake, the way a Geant4 build would: `find_package(nanoarrow CONFIG)` and
`pkg_check_modules(tessera-capi)`. It also asserts that the static `capi_smoke` doesn't load
`libtessera_capi.so`.

**Static HDF5: not needed for this channel.** The bridge's graph has no HDF5. The question returns only if the
bridge depends on `tessera-ingest` before #454 makes `hdf5` optional (§7.1).

### §6.2 The static archive leaks its dependencies' C symbols

A Rust `staticlib` bundles every dependency's object files. Their symbols stay global: zstd (`ZSTD_*`, `HUF_*`,
`FSE_*`, the legacy `ZSTDv05`–`v07`), blake3's C/asm kernels, and the Rust-mangled rest. A Geant4 or HDF5
stack commonly links zstd itself, and here is what happens when both are in one link:

```text
$ g++ smoke.cpp libtessera_capi.a … -Wl,--whole-archive libzstd.a -Wl,--no-whole-archive
entropy_common.c: multiple definition of `FSE_versionNumber'; libtessera_capi.a(…entropy_common.o): first defined here
```

Without `--whole-archive`, the linker would quietly take whichever definition it saw first, and the two
zstd versions would then have to agree on every struct. **A fix that works, measured:**

```sh
ld -r --whole-archive libtessera_capi.a -o tessera_capi.o                         # 5.3 s
objcopy --keep-global-symbols=tsra.syms --strip-debug tessera_capi.o local.o       # 3.9 s
ar rcs libtessera_capi.a local.o                                                   # 8 globals left
```

The localised archive links beside the consumer's own `libzstd.a` and seals the same hash
(`blake3:9c64bd23…`). The shared library needs nothing: rustc already exports only the `#[no_mangle]`
symbols from a cdylib. This belongs in #815 ("the static archive's symbol check") as a build step plus a
check, not just a check.

### §6.3 The header

cbindgen 0.29 generates `tessera.h` from `src/lib.rs` alone, with no `cargo metadata`, so the drift check
runs offline in the sandbox (`rust-cbindgen` from nixpkgs). Two details have to be right, and the prototype
shows how:

- `FFI_ArrowArray` / `FFI_ArrowSchema` are renamed to `struct ArrowArray` / `struct ArrowSchema` (cbindgen
  `[export.rename]`).
- The spec's struct definitions are emitted behind the spec's own `ARROW_C_DATA_INTERFACE` guard. With that
  guard, `tessera.h` and `nanoarrow.h` can be included together in either order. All three C++ programs do it
  under `-Wall -Wextra -Werror`.

### §6.4 nanoarrow is sufficient

nanoarrow 0.8.0 covered everything the producer side needed. It builds the schema, and it builds batches
buffer-wise (`ArrowBufferAppendUnsafe`, `ArrowBitmapAppendUnsafe`) rather than one element at a time. It also
supplies the release protocol. Its CMake package exports `nanoarrow::nanoarrow`. It isn't in this flake's
nixpkgs pin, and a later nixpkgs' package pulls arrow-cpp, gtest and benchmark for its tests, so the flake
builds the core library itself (a 20-line derivation). arrow-cpp was never needed.

## §7 — Findings that bear on ADR-0065

### §7.1 Depending on `tessera-ingest` drags in libhdf5 until #454 lands

ADR-0065 §1 has the bridge call `tessera-ingest`'s arrow lane, so that one map serves both. Today
`tessera-ingest` links `hdf5-metno` and `dicom` unconditionally (`tessera-ingest/Cargo.toml:15-23`), so a C++
consumer would inherit libhdf5. The ADR already names #454 as a prerequisite. The prototype shows the
alternative's cost: the clean lane is about 40 lines in `src/arrow.rs` (`dtype_of` plus a
`values().to_vec()` per type). It needs no transform, because a clean-lane value crosses bit for bit. The
part of the ingest lane the bridge actually needs is its canonicalisation (§7.2), not its type map.

### §7.2 Where hazard H4 is fixed decides whether C++ and Rust producers agree

`nan_payloads_are_sealed_bit_for_bit`: two products that differ only in a NaN payload (`0x7fc00000` against
x86-64's `0xffc00000`) seal different hashes, through the bridge and, by `the_bytes_are_those_of_the_native_writer`,
through the native writer too. ADR-0065 §1 canonicalises NaNs *in the bridge*. That fixes the C++ producer's
cross-arch bytes, but it also makes **a C++ producer and a Rust producer that push the same rows seal
different products** whenever a NaN is present. That would break this spike's strongest equivalence
(verdict 1). There are two consistent options:

- canonicalise in `tessera-io`'s encode path, so every producer gets it (a corpus event only for float
  columns that hold non-canonical NaNs); or
- keep bit fidelity everywhere, as the writer does today (S13), and state that a producer owns its NaN bits.

H5 (masked slots) is already normalised in the encoder (`table.rs:115-121`), so only H4 is at issue. **The owner
decides, and the ADR records which.**

### §7.3 What the prototype confirms

- **`push_table` needs the row count up front** (`stream.rs:861-863`). An unknown count, through
  `TableMultiBlockSink`, seals the same bytes, but it stages the whole table on disk before the first encode.
  That is #808's case, measured as byte-identical here.
- **One table per session at a time.** Both `push_table` and the sink hold `&mut StreamWriter`, so the
  prototype refuses a second `tsra_table_begin` (`STATE`) until the first ends. #808 lifts that.
- **`StreamWriter` has no abort.** Dropping it detaches its threads (there is no `Drop` impl). So
  `tsra_session_free` can't join them, and a detached job whose staging is removed fails harmlessly on its
  own thread. That is #810's `StreamWriter::abort`.
- **The schema is re-sent with every batch** in the prototype, which lets the bridge check every batch's names
  and types. Importing it is a handful of small allocations for a 5-column schema, which is noise against a
  65,536-row batch, but §5 doesn't isolate it. ADR-0065 §2.1 imports against the declared type instead
  (`from_ffi_and_data_type`), which skips the check. Both work. The difference is an API choice.
- **`catch_unwind` adds no work on the success path** under Rust's table-based unwinding: it compiles to
  a landing pad that runs only on a panic. §5's numbers include it on every call, but they don't isolate it.
- **A window counted in batches, with no spill, is safe only under §3.1's precondition, and it bounds batch
  count, not bytes.** ADR-0065 §7.2's RAM credit with spill is the right production design. The claimed runs
  model consecutive claiming, but not a lagging thread that holds the window for everyone under real
  `G4TaskRunManager` block sizes. #812 carries that measurement.

## §8 — Open questions (for the ADR)

1. **Blocking window or spill** (§3.1): if a blocking window ships at all, its precondition is ABI contract.
2. **H4's home** (§7.2): in the bridge, in `tessera-io` for every producer, or nowhere.
3. **#454 first, or a ~40-line clean-lane map in the bridge** (§7.1). The evidence says either is safe for the
   bytes. Only the dependency graph differs.
4. **The static archive**: ship it localised (§6.2), or ship only the shared library plus a static one "at
   your own risk". The localisation is cheap, so the recommendation is to ship it localised.
5. **Status code values**: the prototype's are positive (`TSRA_STATUS_SCHEMA = 2`), the ADR's sketch's are
   negative. Either is fine. The ADR should fix one, because codes are ABI.
6. **ksb can't evaluate this flake as a job**: preparation fails at crane's `fetchGit` of the pinned vortex
   fork (`Failed to fetch git repository 'https://github.com/gerchowl/vortex'`). The `capi-smoke` check
   therefore runs in CI, and the bench shipped prebuilt binaries to ksb in a closure (§5). This is tooling
   friction, not a design point.

## §9 — Limits of the prototype

The prototype has no product schema (#801), metadata, producer or generation, aux or identity, arrays,
RAM-bounded window or spill, and no concurrent tables. Each of those is an ADR-0065 sub-issue (#808–#817). It
is a spike, larger than the 400-line PR guideline (about 2,700 lines with the tests, the C++ programs and the
generated header), because the questions needed a working end-to-end path to answer.
