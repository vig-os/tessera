# Design note — the `tessera-table-v2` codec profile (#915, ADR-0066 step 6)

> **Status:** Accepted (owner, 2026-11-19; decisions in §7). It gates #828 (ADR-0066 step 7) and sets that
> step's acceptance criteria (§5, §8). No product code, SPEC, corpus or golden changes here.
>
> **ADRs:** this note proposes no new ADR. It proposes one ADR amendment, to ADR-0024 (dated, naming
> `tessera-table-v2`), plus SPEC §5b text: the rule table of §2.1, the two invariants and the refusal points of
> §2.3. Both land in #828 with the code that makes them true, as ADR-0066's own amendments land with their
> implementation, and #828's review checks them against this note. ADR-0060 needs no amendment (§2.3).
> ADR-0066 step 6 links this note.
>
> **Evidence:** the per-column sweep in #828 (body and comments), the #822 fixtures and references, the
> #826 writer measurements, ADR-0024 (with its 2026-11-09, -15 and -16 amendments), ADR-0026, ADR-0060
> §2, ADR-0066 rules 10–13, the `tessera_io::table` module at `7b5cedda`, and the pinned Vortex fork
> `gerchowl/vortex@5c55cf7` (0.93.0). File references cite that fork (`vortex:`) or this repo. Numbers
> without a source are marked as estimates or listed as open measurements (§8); no bench ran for this
> note.

## 0. Decision summary

| # | Question | Decision |
|---|---|---|
| 1 | Profiles | **One writable profile, `tessera-table-v2`, a per-column codec chosen by the column's dtype:** ≥ 16-bit integers and floats → **Vortex Pco, level 8**; 8-bit integers → **Vortex Zstd, one frame per row-group, no dictionary** (O1); `b1` and `utf8` keep v1's codec set. Forced, never sampled. The same rule serves every product type: no per-product-type level table and no logical transforms (O0). |
| 2 | Naming | The existing sealed `TableSpec.codec_profile` field, value `"tessera-table-v2"`, set by the writer only. No per-column record. Readers never refuse a block for its profile; that holds because a profile may never change decoded values (§2.3). |
| 3 | Trade-off | Smaller (sweep: GE −18.6 %, Siemens ≈ −5 to −8 % against today's `.tsra`, §2.4) and no per-chunk scheme search. In exchange, random `take` and non-contiguous filters decode whole 65,536-row groups instead of O(1) kernels (§3), and full-scan decode is likely slower (M3). Projection and zone-map pruning are kept. |
| 4 | Determinism | Same gate as today (corpus goldens on x86_64 + aarch64, dev == release), plus a new `table_v2_dtypes` fixture, a workers 1 == N check, an aarch64 run on the #828 head before merge, and the pinned `pco` and libzstd versions in the profile's drift guard. |
| 5 | Acceptance | GE ≤ 3.25 B/ev, Siemens ≤ 7.10 B/ev; lossless on both #822 fixtures; whole-writer encode ≥ v1 per core; full-scan decode ≥ 500 MiB/s per core; take, projection and pruning floors (§5). |
| 6 | Rejected | Per-product-type level tables, logical transforms, sampled BtrBlocks with integer Pco, native 8-bit Pco under the current edition, a standalone codec frame outside Vortex, uniform zstd, a dense default (§6). |

The owner decided O0–O4 on 2026-11-19, each as recommended (§7): the dtype rule (O0), `vortex.zstd` for 8-bit
columns (O1), row-group-granular take with A4's floors (O2), no per-column codec record (O3), and a green
both-legs aarch64 run before #828 merges (O4).

## 1. Where v1 loses, and what v2 changes

`tessera-table-v1` (`tessera-io/src/table.rs:1076-1086`) is BtrBlocks, compact preset, with ALP, ALPRD,
**integer Pco**, Delta and OnPair excluded. Integers therefore never reach Pco. BtrBlocks samples each chunk
and picks among FoR, bit-packing, dictionary, RLE, sparse and constant. The best of those for a 40-distinct `u1`
column is about 6 bits/value, against an order-0 entropy of 5.3 bits. Summed over Siemens' nine `u1` columns,
bit-packing alone gives ≈ 6.4 B/ev, against a summed entropy of ≈ 5.7 B/ev (estimate from #828's per-column
entropies). That is close to the observed gap: `.tsra` 7.509 against uniform Pco 6.843 B/ev. Row-group size is
not the cause (+0.2–0.3 % for Pco at 65,536 rows, #828 finding 1). Zone maps and footer are ≤ 1 % of the file
(#828 comment of 2026-11-11). The per-column breakdown that proves this is open measurement M1.

v2 gives every column an entropy coder, which is what the sweep measured, and removes the per-chunk choice.

## 2. Decisions

### 2.1 Profiles

**The rule (the whole of `tessera-table-v2`):**

| Column dtype | Encoding | Parameters | Chosen by |
|---|---|---|---|
| `i2 i4 i8 u2 u4 u8 f4 f8` | `vortex.pco` | level 8 (`pco::DEFAULT_COMPRESSION_LEVEL`), 8,192 values per page, one Pco chunk per 65,536-row group | the dtype; no estimate, no sample |
| `i1 u1` | `vortex.zstd` | one level for every 8-bit column (M6 picks it from {1, 3, 9}; provisional 9), one frame per row-group (`values_per_frame = 0` on the row-group chunk), no dictionary (`ZstdData::from_primitive_without_dict`), single-threaded | the dtype; **O1** |
| any numeric, `nullable` | as its dtype; the validity mask is Vortex's own child | as above | the dtype |
| `b1`, `utf8` | v1's codec set (BtrBlocks compact, same exclusions, and the string/binary Zstd schemes **excluded**, so enabling zstd for integers does not move string bytes; #628 stays its own event) | unchanged | BtrBlocks, as v1 |

Layout constants stay frozen: `ROWS_PER_GROUP = 2^16`, `BLOCK_ROWS = 2^22`, zone maps every 8,192 rows, edition
`core2026.08.3`. `vortex.pco` and `vortex.zstd` are both core-edition members since `core2025.06`
(`vortex:vortex-edition/src/declarations/core/v2025_06.rs:22,24`), so v2 needs no new edition, and any
Vortex ≥ 0.85 reader that registers them decodes v2 blocks.

**Per product type:**

| Product type | Profile | Evidence (sweep, standalone codecs, 2^22-row blocks) |
|---|---|---|
| GE list-mode events (5 × `i2` + `t_ms` `u4`; no 8-bit column) | `tessera-table-v2` | uniform Pco-8: **3.182 B/ev** (v1 `.tsra` 3.907, HDF5 shuffle+zstd 3.742, GLEPL 3.364 on the full bed) |
| Siemens list-mode events (`u4`, `i2`, 9 × `u1`) | `tessera-table-v2` | Pco-8 on `ms`, `tof`; zstd on the 9 × `u1`: **≈ 6.9 B/ev** at 2^22-row frames (estimate from #828's fast profile, 6.891, which uses zstd-1/zstd-9 on 8 of the 9 and Pco on `prompt`); uniform Pco with `u1` widened: 6.843 (v1 7.509, HDF5 shuffle+zstd 7.034, shuffle+zstd-19 6.902) |
| Every other table (ROI, spectra, generic ingest, SQL results, sequenced tables) | `tessera-table-v2` | not in the sweep; covered by the corpus size table (§5, A6) |

The default is `tessera-table-v2`. It is also the only profile a writer can write, as v1 is today: a profile is
a fixed set of constants, not a run-time choice (ADR-0024, 2026-11-16 amendment). Pco-8 is the level of both of
#828's "uniform" rows. A dense profile for #796 (archival re-encode) gets its own id, chosen in #796 and not here
(§6, G).

**Implementation constraints for #828.**

- The numeric path calls `vortex_pco::Pco::from_primitive(…, 8, 8192, …)` and
  `vortex_zstd::ZstdData::from_primitive_without_dict(…, level, 0, …)` (wrapped by `Zstd::try_new`; the plain
  `Zstd::from_primitive` trains a dictionary) per row-group chunk, directly or through
  schemes with no estimate. Admitting `IntPcoScheme` into BtrBlocks would keep the sampled comparison against the
  other integer schemes (`vortex:vortex-btrblocks/src/schemes/integer/pco.rs:52`, `DeferredEstimate::Sample`):
  rejected alternative C. Stock `IntPcoScheme` hard-codes level 8 and 8,192-value pages (`…/integer/pco.rs:62-67`),
  so the forced path and the stock scheme agree on parameters.
- Decoding `vortex.zstd` needs `vortex-file`'s `zstd` feature (`vortex:vortex-file/src/lib.rs:177-178`), which
  brings `vortex-zstd` and `zstd` 0.14 beside zarrs' 0.13. That is a new direct feature and new crates: it trips
  `checks.*.feature-snapshots` and needs a `cargo-deny` pass. Required: O1 is decided (b).
- `Pco` measured in the sweep is pcodec 1.1.0 (Python); `Cargo.lock` pins `pco` 1.0.2. A1's margins assume the two
  compress alike; M1 confirms it.

### 2.2 Naming in the manifest

- **Field:** `blocks[].spec.codec_profile` (`TableSpec.codec_profile`, `tessera-core/src/block/table.rs`). It
  already exists (ADR-0024, 2026-11-16 amendment, #869). Its value becomes `"tessera-table-v2"` through
  `tessera_io::table::TABLE_CODEC_PROFILE`. `edition` stays `"core2026.08.3"`.
- **Who sets it:** unchanged. `table::sealed_spec` records the writer's constant on every table block,
  partition leaf and sequenced-table leaf, and refuses a caller spec that names another profile
  (`table.rs:791-838`).
- **What it binds:** unchanged. It is in the manifest, so `manifest_hash` covers it, and in no payload. The
  payload bytes move because the encoder changes, which moves the block digests, `content_hash` and
  `manifest_hash` of every table-bearing product (ADR-0066 Consequences, "Seals move, in steps").
- **No per-column codec record.** #828's requirement "each column's codec is recorded in the manifest" is
  met without a new field. Under v2 a column's codec is a pure function of (profile, dtype), stated in SPEC §5b.
  The encoding actually used is in the Vortex footer (wire IDs per array), which `tessera inspect` can show. A
  second sealed copy would be redundant data that could disagree with the payload (O3).
- **`Column.codec` on table columns.** The field exists (`table.rs:16-18`, builder `with_codec`), and no
  table writer reads it. A caller can seal `codec: "zstd"` over a Pco column today, so the field can be false;
  the corpus `listmode` fixture does exactly that (`tessera-io/src/conformance.rs:158-160`). #828 makes the table
  writers refuse a table column that sets it, with an error that points to `codec_profile`, as `sealed_spec`
  already does for the profile, and drops the three `Some("zstd")` from that fixture.

### 2.3 Refusal, and the interaction with ADR-0060

- **Readers do not refuse an unknown profile, and v2 keeps that sound.** SPEC §5b: "A reader MUST NOT refuse a
  table block because its profile is unknown or absent". That rule is safe only while decoding needs nothing
  but the Vortex wire IDs, which the edition governs. This note makes two invariants of every profile:
  1. **A profile may change which encodings are written, never what the decoded values are.** No tessera-level
     transform (column differences, delta, narrowing, widening into a different stored dtype) may sit behind a
     profile id. A future profile that needs one must express it as a Vortex encoding (decodable by wire ID) or
     as a new block kind.
  2. **A profile may only write arrays that every reader of its edition decodes once it registers the
     edition's encodings.** The edition is the decodability promise (SPEC §5b). An encoding a reader has not
     registered fails with a typed "encoding not found" error that names the wire ID (`vortex.zstd` is
     registered by the `vortex` crate's default features, but by `vortex-file` only behind its non-default
     `zstd` feature); that is a build choice of the reader, fixed by registering it. An array that a reader
     with every edition encoding registered still cannot decode (native 8-bit Pco: `unreachable!`, a panic)
     breaks the promise. That is why native 8-bit Pco is rejected under `core2026.08.3` (O1, §6 D′).
- **Where an unknown profile is refused:**
  1. a writer asked to record a profile it does not write (today's `sealed_spec` behaviour);
  2. any check that re-encodes rows to compare bytes with a sealed block (the #796 re-encode, a future
     `verify --reencode`): it reports "cannot check: profile `X` is not this build's" and exits non-zero. It
     never reports a match or a mismatch it cannot establish;
  3. a decode that hits a wire ID the reader lacks fails as today; #871 makes that error name the edition, and
     #828 extends it to name the profile too.
- **Informational note.** `tessera verify`/`inspect` print `note: table block <name>: codec profile <X> is
  not known to this reader; decoding by Vortex wire IDs` for a profile they do not know. Exit status is
  unaffected.
- **ADR-0060 newer-field rules.** `blocks[].spec` is an opaque map that the newer-field walk does not enter
  (ADR-0060 §2, "Opaque maps are never walked"), and values never count. So an unknown profile is neither a
  newer field nor a reason to block a re-seal. That is correct. A re-seal (`commit`, `publish`, the ingest
  engine) copies the payload bytes unchanged, so the recorded profile is still true of them. A path that
  **re-encodes** a table block writes its own profile through `sealed_spec` and never carries the old one. v2
  adds no key, so ADR-0060 needs no amendment for this.
- **"v1 tables still read"** (owner, #828, 2026-11-19). Decoding is by wire ID, so a v1 payload decodes under
  a v2 reader with no code path of its own. ADR-0066 rule 10 refuses pre-A′ products at the container. That
  leaves no product in the new format that carries v1 blocks, so #828 guards the property at the codec level:
  a frozen v1 payload (`tessera-table-v1`, `core2026.08.3`) must decode bit-exactly through
  `tessera_io::table::decode`.

### 2.4 Speed and size trade-off of the default

From the sweep (pcodec 1.1.0 and zstandard 0.25.0 via Python, one pinned core, shared host at load ≈ 4.7,
codec time only; speeds are indicative and comparable only within the sweep) and #826 (whole writer):

| Profile | GE B/ev | GE enc M ev/s | Siemens B/ev | Siemens enc M ev/s |
|---|---:|---:|---:|---:|
| v1 `.tsra` (bench writer) | 3.907 | 6.68 whole writer (#826) | 7.509 | 6.14 whole writer (#826) |
| HDF5 shuffle+zstd (bench writer) | 3.742 | 15.98 whole writer (#826) | 7.034 | 27.92 whole writer (#826) |
| **uniform Pco-8 (= v2 on GE)** | **3.182** | **22.3** codec | 6.843 (`u1` widened) | 14.1 codec |
| fast per-column + transforms (#828 proposal; nearest measured row to v2 on Siemens) | 3.154 | 25.2 codec | **6.891** | **49.2** codec |
| best per-column + transforms | 3.085 | 3.7 codec | 6.828 | 4.8 codec |
| uniform shuffle+zstd-3 | 3.742 | 52.9 codec | 7.037 | 89.6 codec |

What the default takes:

- **Size, GE:** −18.6 % against v1. Within 0.9 % of the fast profile and 3.1 % of the best measured
  per-column choice; below GE's closed GLEPL codec.
- **Size, Siemens:** the sweep compressed each column in one 2^22-row frame; v2 uses one zstd frame per
  65,536-row group, which cost zstd up to +2.9 % in #828's row-group check (Pco +0.3 %). With that worst case
  on the 8-bit columns, Siemens lands at ≈ 7.05 B/ev before zone maps and footer, ≈ 7.10 after: −5.4 % against
  v1, and about 1 % *above* HDF5 shuffle+zstd (7.034). Widened Pco would be ≈ 6.95 (§7, O1). The 2^22-frame
  sweep reached the summed order-0 entropy (6.83, #828 finding 3); the ≈ 3 % above it under O1(b) is
  per-frame cost, not modelling. Past the entropy, gains need joint modelling (§6, H).
- **Encode:** Pco-8 codec time is 3.3× (GE) and 2.3× (Siemens, widened) v1's *whole-writer* rate per core.
  The #828 fast profile, whose Siemens speed comes from zstd on the 8-bit columns, is 49.2 M ev/s codec-only. v1's
  codec share is not separated (#826 measured "BtrBlocks scheme sampling plus Pco" together; M1/M4), so the
  writer-level gain is measured in #828 (§5, A3), not claimed here.
- **Decode (full scan):** v1's decode speed on integer list-mode is not measured. The FEATURE-MATRIX table row
  (4.4 GiB/s) is a `u8` counter plus two `f4` columns that already go through Pco (`benches/codec.rs`), so it is
  no baseline for bit-packing. Pco decodes `i2` volumes at 1.02 GiB/s per core (FEATURE-MATRIX, array path).
  v2's full-scan decode is likely slower than v1's on integer columns, by an unmeasured factor (M3). Floor in §5.
- **Random `take` and filters** (load-bearing; ADR-0024 chose Vortex for O(1) take): see §3.

### 2.5 Determinism

The requirement is ADR-0024's and ADR-0026's: same rows + same edition + same profile → same bytes, on every
architecture, build profile and worker count. The cause-level hazards and what v2 does with each:

| Hazard | v1 status | v2 |
|---|---|---|
| ALP/ALPRD float exponent search (ADR-0024 §Determinism) | excluded | still excluded; floats are forced to Pco |
| #468: `#[cfg(debug_assertions)]` sortedness check caches statistics on `Patches` indices | carried by the fork pin; guard `patch_indices_persist_no_sortedness_statistic` | Pco and Zstd emit no `Patches`; `b1`/`utf8` still go through BtrBlocks, so the guard stays, under both `cargo test` and `--release` |
| #472: persisted `Stat::Sum` NaN from `inf + -inf` | fixed upstream 0.89; 0.93 zone maps carry no sum; guard `float_stats_persist_no_nan_for_inf_and_neg_inf` | unchanged; the guard must run on a v2-encoded block |
| Pco float mode detection (`FloatMult`/`FloatQuant`) uses float arithmetic to choose a mode | reached only when BtrBlocks picked Pco for a float column | reached on **every** float column, so the fixture must exercise those modes (below). Whether the choice can differ across arches is not established (M7) |
| Encoder versions: pcodec and libzstd promise decodability, not identical bytes across releases | `pco` transitive, `Cargo.lock` only; zstd unused for tables | the pinned `pco` and libzstd (`zstd-sys … +zstd.1.5.7`) versions join the profile: the drift guard prints both, and a bump is a profile bump |
| zstd multithreading or dictionary training | — | neither used: single-threaded, `without_dict` |
| Worker count (ADR-0026 multi-block parallel encode) | proven worker-independent | each chunk is encoded on one thread; asserted again for v2 |

**The gate that proves v2 deterministic** (all required for #828):

1. **Corpus conformance** (`tessera-io/tests/conformance.rs`): the goldens regenerate once in #828, every moved
   hash explained (ADR-0066's goldens rule), and the gate re-derives them on x86_64 (PR leg) and aarch64.
2. **A new fixture `table_v2_dtypes`** in `corpus/`. It has one column per numeric dtype including `i1`/`u1`,
   nullable variants, a constant column, a 2-distinct column, an empty table and row counts crossing the page
   and row-group boundaries (65,536 + 8,193 rows). Floats must include NaN with payloads, ±inf, −0.0,
   denormals, a decimal-quantised column (`k × 0.1`, to drive Pco's `FloatMult`) and a low-bit-mantissa column
   (to drive `FloatQuant`).
3. **dev == release:** the flake's existing two-profile run covers v2 (the #468 axis).
4. **workers 1 == workers N:** a multi-block v2 table sealed with 1 and 4 workers has the same `content_hash`.
5. **aarch64 before merge:** pull requests run the x86_64 leg only (#725). The #828 PR must link a green
   `workflow_dispatch` run of `nix-check.yml` on its head with both legs (O4).

## 3. Random take and filter pushdown under v2

Vortex was chosen for O(1) take, projection, filter pushdown and zero-copy Arrow/DuckDB (ADR-0024 Context). v2
keeps the file format, the layout and the zone maps. What changes is the granularity of take and of filters
inside a row-group.

- **Projection:** unchanged. Columns are separate Vortex arrays; reading one column touches only its segments.
- **Zone-map pruning:** unchanged at the layout level. Zone maps (`vortex.zoned`, every 8,192 rows) are
  independent of the column encoding, and a row-group whose zones all fail is skipped without decoding.
- **Inside a surviving row-group: whole-chunk decode.** `vortex.pco` has only slice and cast rules
  (`vortex:encodings/pco/src/rules.rs`), no take or filter kernel; `vortex.zstd` likewise decodes frames. A filter
  mask that is one contiguous range becomes a slice (`vortex:vortex-array/src/arrays/filter/vtable.rs:166-169`),
  and a Pco slice decodes only the pages it covers (`vortex:encodings/pco/src/array.rs:641-730`). Any other mask
  canonicalises the whole child (`vtable.rs:177-184`), i.e. the full 65,536-row chunk of each requested column.
  v1's FoR, bit-packed and dictionary arrays have take and compare kernels that avoid that.
- **Random take: O(row-group), not O(1).** tessera's take goes through `with_row_indices`
  (`tessera-io/src/table.rs:1643`), a non-contiguous mask per row-group. 500 uniformly random rows of a full
  2^22-row GE block (64 row-groups, ≈ 7.8 hits each) decode essentially every row-group of every requested
  column: ≈ 58.7 MB of logical decode for all six columns, ≈ 55 ms at ≈ 1 GiB/s (estimate; inferred from the
  filter execution path, the layout reader not traced end to end; M2). A time window or a sorted cluster touches
  few row-groups.
- **The remedy, not in v2:** a page-granular take/filter kernel for `vortex.pco` upstream (the pages are
  independently decodable, `array.rs:641-730`) would bring take down to one 8,192-value page per hit and would
  not change the stored bytes. It is a reader-side follow-up to #828 (O2), not a profile.
- **DuckDB/Arrow interop:** unchanged; both encodings are core-edition members that any Vortex ≥ 0.85 reader
  with the default encodings decodes.

So v2 keeps projection, layout pruning and interop, and trades strict O(1) take and in-chunk compute on
compressed data for size. The floors in §5 (A4) state what is acceptable; the owner accepted it (O2).

## 4. What #828 implements (scope, for the reviewer of step 7)

- `tessera-io::table`: the forced numeric path (§2.1), `TABLE_CODEC_PROFILE = "tessera-table-v2"`, the drift
  guard `codec_set_matches_the_pinned_codec_profile` re-pinned to print the numeric rule (encodings, levels, page
  and frame sizes, `pco` and libzstd versions) beside the BtrBlocks set used for `b1`/`utf8`. Also: the
  `Column.codec` refusal, the unknown-profile note, and the frozen v1 payload decode test.
- `tessera-io` benches for take, projection and full-scan decode at list-mode scale: none exists today
  (`tessera-io/benches` has `codec.rs` and `throughput.rs`; the FEATURE-MATRIX take/projection floors are spike
  numbers).
- `tessera-core`: docs of `TableSpec.codec_profile` and `Column.codec`; no new field.
- `tessera-ingest`, `tessera-cli`, `tessera-py`, the reference reader: no format logic changes (they never
  choose a codec); `inspect` shows the per-column encoding from the footer.
- `tessera/Cargo.toml`: `vortex-file`'s `zstd` feature (O1(b)), with the feature snapshots regenerated.
- ADR-0024 amendment (dated, `tessera-table-v2`, #828) and SPEC §5b: the rule table, the two invariants of
  §2.3, and the refusal points.
- Corpus: `table_v2_dtypes`, the `listmode` fixture without `codec: "zstd"`, and the regenerated goldens, each
  moved hash explained.

## 5. Acceptance criteria for #828

These are #828's acceptance criteria: #828 merges only when every row holds, and its PR reports each with its
evidence. The open measurements of §8 are part of the same acceptance. Measured on the `.tsra` written by the real writer (ingest of the #822 fixtures), not by a standalone codec.
Heavy runs go through `ksb run --exclusive-cpus`; every number reports the command, the host and its exit code.

| # | Criterion | Number | How |
|---|---|---|---|
| A1 | **Size, GE** (54,161,949 events) | **≤ 3.25 B/ev** (v1 3.907; sweep 3.182 plus ≈ 2 % for zone maps, footer and 65,536-row chunks) | Σ table block bytes / events |
| A1 | **Size, Siemens** (55,004,119 events) | **≤ 7.10 B/ev** under O1(b) (v1 7.509; §2.4, two upper bounds stacked: zstd's worst row-group cost +2.9 % and the zone-map bound 0.85 %); O1(a), native 8-bit Pco, would target ≈ 6.95 in a later profile | same |
| A2 | **Lossless** on both fixtures | 0 mismatches: each decoded column `array_equal` to its source `.npy`, blake3 of the decoded column equal to that of the source; `tessera verify` exit 0 | ksb job |
| A3 | **Encode, per core** | v2 whole-writer M ev/s ≥ **1.00 ×** v1 on the same host, pinned core, median of 3, both fixtures; absolute numbers reported beside #826's 6.68 / 6.14 | paired run, same binary except the profile |
| A4 | **Random take** | 500 uniform random rows of one full 2^22-row GE block, all columns, **≤ 100 ms** (estimate ≈ 55 ms, §3); also the FEATURE-MATRIX `take ×500 ≤ 30 ms` floor on a 100k-row table | new `tessera-io` bench |
| A4 | **Projection** | FEATURE-MATRIX floor ≤ 15 ms on a 100k-row table | new bench |
| A4 | **Filter pruning** | a `t_ms` window filter over a GE block decodes only row-groups whose zones overlap the window | test counting decoded row-groups |
| A5 | **Decode, per core** | full-scan decode of the GE fixture **≥ 500 MiB/s** of logical column bytes on one core (≈ 37 M ev/s at 14 B/ev); v1's number reported beside it | new bench |
| A6 | **Corpus size table** | every corpus table block, v1 vs v2 bytes. A block that grows is listed with its cause; the owner signs off on growth before merge | in the #828 PR body |
| A7 | **Determinism gate** | §2.5 items 1–5, including a green both-legs `workflow_dispatch` run on the PR head (O4) | CI links |
| A8 | **Guards** | #468 and #472 guards run on v2-encoded blocks; the drift guard fails on a `pco` or libzstd bump; the v1 frozen payload decodes bit-exactly | unit tests |

Reasons for the floors that are not regressions against v1:

- **A4 take, 100 ms:** about twice the §3 estimate, so a mis-estimate of the decode rate by 2× still passes, and
  a whole-block decode at a pathological rate does not.
- **A5 decode, 500 MiB/s:** half the measured Pco rate on `i2` (1.02 GiB/s), leaving room for Zstd, validity and
  layout costs. At ≈ 37 M ev/s, one core decodes GE faster than the whole v1 writer produces it on 8 cores
  (10.8 M ev/s, #826), so a reader is never the bottleneck of a write-then-read pipeline.

Each criterion needs a test or a recorded run that fails before the change (brief rule): A1/A3/A5 report v1's
number from the same run, and the determinism tests fail against v1's goldens.

A1's GE number is below GE's closed GLEPL codec (3.364 B/ev on the full bed). The slice is representative:
`.tsra` gives 3.899 on the bed and 3.907 on the slice. #822's Siemens stretch target (≤ 6.90 B/ev, HDF5
shuffle + zstd-19) is for #796's dense profile; A1 does not require it.

## 6. Alternatives rejected

- **A. Per-product-type per-column level tables (#828's "fast" and "dense" profiles).** On GE the fast profile
  gains 0.9 %, and only with the `hi_tx − lo_tx` transform (rejected in B). On Siemens its codec choice is v2's
  (Pco on ≥ 16-bit, zstd on 8-bit), with per-column levels on top. Cost: the table needs a home (the product
  schema or the writer) and a drift guard per product type, and an unlisted product type needs a fallback rule
  anyway, which is v2's rule. The owner chose the dtype rule (O0).
- **B. Lossless logical transforms** (store `hi_tx − lo_tx`, delta-code time, narrow or widen the stored dtype).
  Measured gain: 5.6 % on one GE column, 1.85 % of the file (3.143 → 3.085 B/ev). Delta coding time gains
  nothing, because Pco already takes both time columns to ≈ 0 B/ev (#828 finding 5). A transform makes the
  profile a decoding instruction. A reader that ignores the profile, which SPEC §5b requires to be allowed,
  would return wrong values with no error, and a third-party Vortex or DuckDB reader would see `hi_tx − lo_tx`
  as `hi_tx`. That breaks §2.3's first invariant.
- **C. Keep BtrBlocks and admit integer Pco.** BtrBlocks would still sample per chunk, part of the encode cost
  #826 measured. The choice stays data-dependent and harder to pin in a drift guard. #828 requires "no scheme
  sampling at write time".
- **D′. Native 8-bit Pco under `core2026.08.3`.** The `pco` crate supports 8-bit latents behind
  `ChunkConfig::enable_8_bit` (`pco-1.0.2/src/chunk_config.rs:223,306`), but `vortex-pco` maps no 8-bit type
  (`vortex:encodings/pco/src/array.rs:288-300`, `unreachable!`). Patching it on the fork would write `vortex.pco`
  arrays that every stock reader of the edition panics on, which breaks the edition's decodability promise
  (§2.3, second invariant) and deepens the fork #480 tracks. It becomes admissible once a released Vortex
  decodes it, as a later profile.
- **D. A standalone Pco or zstd frame per column, outside Vortex** (what the sweep ran). It loses the Vortex
  layout, zone maps, projection and DuckDB interop. ADR-0024 rejected the same thing for the same reasons.
- **E. Uniform zstd or shuffle+zstd.** Faster to encode (52.9 / 89.6 M ev/s codec) but larger: 3.742 / 7.037
  B/ev, no better than HDF5.
- **F. Another layout.** HDF5 columnar 1-D with shuffle+zstd-3 gives 3.768 B/ev on GE, Parquet zstd-19 4.265
  (#822). An HDF5 compound dataset cannot carry a codec per field at all. Both are larger, and switching would
  reverse ADR-0024.
- **G. A dense profile (Pco-12, zstd-19 per column) as the default.** It is 1.2 % (GE, best source-only 3.143)
  and ≈ 1 % (Siemens) smaller than v2, at 3.6–4.8 M ev/s codec time, slower than v1's whole writer. It belongs
  to #796's offline re-encode, under its own id.
- **H. Joint, geometry-aware crystal-pair modelling** (what GLEPL presumably does). Siemens is at its order-0
  entropy, so this is the only way past v2. It is a new Vortex encoding or block kind, out of scope here.

## 7. Owner decisions (2026-11-19)

The owner chose the recommended option on every question. The options and their rationale are kept as the record.

- **O0 — what "the per-column codec profile" means for v2. Decided: (a), the dtype rule of §2.1.** The owner
  adopted "the per-column codec profile as the new default `tessera-table-v2`" (#828, 2026-11-19; ADR-0066 rule
  13). #828 proposed per-product-type tables with per-column levels and transforms. This note keeps the
  per-column codec but chooses it by dtype.
  - **(a) Chosen:** the dtype rule of §2.1. It gets GE to within 0.9 % of #828's fast table, uses the same
    codecs on Siemens, has no transforms (§6 B), and covers every product type with one drift-guarded rule.
  - (b) #828's per-product-type tables without transforms, the dtype rule as fallback. Per-column levels, a table
    per product type in the writer or schema, a drift guard per table; gain ≤ 1 % on the fixtures.
  - (c) #828's tables with transforms. Rejected by §2.3's first invariant unless the owner drops "readers never
    refuse an unknown profile".
- **O1 — the codec for 8-bit integer columns (`i1`, `u1`; 9 of Siemens' 11 columns). Decided: (b),
  `vortex.zstd`, one frame per row-group, no dictionary; M6 fixes the level.**
  - **(b) Chosen: `vortex.zstd`.** It is a core-edition encoding, so the edition promise holds and no fork grows.
    A reader built on `vortex-file` without its `zstd` feature gets an "encoding not found" error on Siemens
    blocks (not a panic); tessera's readers enable it. Cost: new crates (feature snapshots, `cargo-deny`), and
    Siemens lands at ≤ 7.10 B/ev rather than ≈ 6.95 (A1), about 1 % above HDF5 shuffle+zstd.
  - (a) Native 8-bit Pco: wait for a released Vortex that decodes it (upstream PR from us), then adopt it as a
    later profile. Not admissible before that (§6 D′). Size ≈ 6.95 B/ev if it matches widened Pco (M6).
  - (c) Widen to `u16` in storage: rejected by §2.3's first invariant (the stored dtype changes).
- **O2 — row-group-granular take. Decided: accept, with A4's floors.** The dominant list-mode access is a scan
  or a time-window filter, not a uniform random take. The page-granular Pco take/filter kernel (§3) is a
  reader-side follow-up that moves no bytes; it is not part of #828.
- **O3 — per-column codec record. Decided: none (§2.2).** The codec is derived from (profile, dtype) and is
  visible in the footer. The rejected alternative was a writer-owned `Column.codec` value per column, sealed, at
  the cost of a redundant field that could disagree with the payload.
- **O4 — aarch64 before merge. Decided: require** a green `workflow_dispatch` run of `nix-check.yml` with both
  legs on the #828 head before it merges, since a nightly catch after merge would mean a second regeneration
  inside the same alpha.

## 8. Open measurements — part of #828's acceptance (none ran for this note)

#828 runs each measurement and reports it in its PR beside the §5 row it feeds. A result that breaks a §5 floor
blocks the merge until the owner re-decides; M6's result fixes the Zstd level of §2.1 before the goldens are
regenerated.

- **M1** Per-column encoded bytes and stats/layout bytes of one v1 block and one v2 block of each fixture
  (#828's 2026-11-11 ask), with the scheme v1's BtrBlocks chose per column (#828 question 1), and `pco` 1.0.2
  against the sweep's 1.1.0.
- **M2** Take latency on a full 2^22-row block: 1, 500 and 5,000 uniform random rows, and a 500-row sorted
  cluster; confirm the row-group granularity of §3 through the layout reader.
- **M3** Full-scan decode per core, v1 vs v2, both fixtures.
- **M4** Whole-writer encode per core, v1 vs v2 (A3), split into codec, hashing and framing time if the profiler
  allows. This also answers #828 question 3 (does v2 plus full workers close #826?). #826 stays open on its own
  evidence.
- **M5** Pco page and zone alignment for nullable columns (Pco pages count valid values).
- **M6** Zstd at levels 1, 3 and 9 with one frame per 65,536-row group against the sweep's 2^22-row frames, on
  Siemens' 8-bit columns; native 8-bit Pco against widened `u16` Pco, for O1(a).
- **M7** Whether Pco's float mode choice depends on the architecture for edge-case columns (the fixture of §2.5
  item 2 is the test).

## 9. Pre-mortem — v2 shipped as the default and failed a year later. How?

1. **A dependency bump moved every table hash.** A `cargo update` pulled a `pco` minor release (or a `zstd-sys`
   with a new libzstd) whose encoder made different, still decodable bytes. Goldens went red and the hashes of a
   released format moved. *Guard:* both versions are part of the profile and printed by the drift guard (A8);
   bumping either is a profile bump and an ADR-0024 amendment.
2. **Nightly aarch64 went red on a float table.** Pco's float mode detection picked `FloatMult` on x86_64 and
   classic on aarch64 for a column on the decision edge. *Guard:* the `table_v2_dtypes` fixture drives both
   float modes and the edge cases (§2.5 item 2), and the aarch64 leg runs before merge (O4).
3. **A fork patch broke the edition promise.** Someone "fixed" Siemens' size by patching 8-bit Pco into the fork,
   and DuckDB's Vortex extension, a stock reader of the same edition, panicked on Siemens products. *Guard:*
   §2.3's second invariant in SPEC §5b and ADR-0024, and §6 D′.
4. **Random-access consumers regressed.** The explorer (#286) and a sampling tool did uniform random takes over
   list-mode and decoded a whole 65,536-row group per column for each scattered hit, where v1 decoded the rows.
   *Guard:* A4's list-mode take floor, O2 recording that the change was accepted with its cost, and the
   page-granular kernel as the follow-up.
5. **Small tables got bigger.** ROI and summary tables of a few hundred rows paid Pco's and Zstd's per-chunk
   headers, where v1's constant and FoR encodings were smaller. *Guard:* A6's corpus size table, with every
   growth signed off. A small-row profile is #869's second comment, a separate id.
6. **Reconstruction got slower.** A full-scan consumer decoding 10⁹ events per run lost decode speed per core,
   and nothing measured it. *Guard:* A5's decode floor, with v1's number published beside it.
7. **The speed claim did not hold.** The writer's per-core rate barely moved because #826's cost was in the
   single committer or fsync, not in BtrBlocks' sampling, and v2 was credited with a speed-up it did not
   deliver. *Guard:* A3 is a paired whole-writer measurement, and the note claims only "no regression".
8. **Someone added a transform "just for GE".** A later profile stored `hi_tx − lo_tx`, and an old reader,
   allowed to ignore the profile, returned wrong `hi_tx` values silently. *Guard:* §2.3's first invariant,
   written into SPEC §5b and ADR-0024: a profile never changes decoded values.
9. **Siemens never beat HDF5.** O1(b) left Siemens about 1 % above HDF5 shuffle+zstd, and the comparison table in
   `why-tessera.md` had to say so. *Guard:* A1 states the number up front; O1(a) is the path past it, and #796
   the dense one.
