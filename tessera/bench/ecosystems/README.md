# Cross-ecosystem I/O comparison (#143, method hardened in #485)

Compares Tessera `.tsra` against the storage formats it competes with — **HDF5, Zarr, NeXus, NIfTI,
DICOM, Parquet, ROOT** — on the same synthetic data, through one driver that times every format
identically.

## What it measures

Two modalities (sized to match the Rust `.tsra`-vs-bare bench, `crates/tessera-io/examples/bench_compare.rs`):

- **Volume** — int16 CT-like 256³ (32 MiB raw).
- **Table** — `u8 + 2×f4`, 1M rows (~15 MiB raw), in **two fixtures** (below).

Per format, per variant, per modality: compression ratio, on-disk size, write throughput, full read,
partial access (one Z-slice / one column), **warm and cold**, as **median of N with [min..max]**.
Every adapter is asserted **bit-exact** before its timings count. Integrity mechanisms are timed and
described separately.

## The method, and what each rule is there to prevent

Each of these exists because it was already got wrong once.

| Rule | Why |
|---|---|
| **Median + spread**, never min-of-N | The minimum is a throughput ceiling. It hides variance and cannot show when two formats are within noise of each other — which is the question a comparison table exists to answer. |
| **Default AND standard tuning**, settings printed on every row | #487 found HDF5 was written without `shuffle`: adding it took the tuned table 5.9 MiB → 155.3 KiB and **inverted that headline**. #503 found Parquet without `BYTE_STREAM_SPLIT`: 39.3 → 23.4 MiB, turning a claimed 2.0× win into 1.19×. Omitting a format's standard tuning is not a neutral default. |
| **Both table fixtures, always, each labelled** | #497: same encoder, same command, opposite verdicts. Publishing one is choosing the answer. |
| **Cold reads with MEASURED residency** | `POSIX_FADV_DONTNEED` silently skips dirty pages, so #487's first cold implementation evicted nothing and cold matched warm to four decimals. The harness fsyncs first and prints the `mincore` residency it actually achieved. A cold-labelled row served from RAM is worse than no row. |
| **Correctness gates timing** | Reads are verified against the source before their numbers count, so no format can win by returning something cheaper. |
| **No invented knobs** | A format with no real tuning lever gets one variant and must say why (`SINGLE_VARIANT_REASON`, printed). Fabricating a row to satisfy symmetry is the same dishonesty as omitting a real one. |

### The two fixtures

- **periodic** — `e0`/`e1` with exact periods 7 and 5. **Adversarial for value-distribution
  codecs**: deflate's LZ77 window locks onto the repeating byte block, while Pco, dictionary and
  bit-packing model the value *distribution* and cannot exploit periodicity. HDF5 shuffle+gzip wins
  here by ~6×.
- **continuous** — listmode-like, shaped after what #493 measured on real `/events_2p`: a coarse
  millisecond clock with **Poisson** run lengths (an exact stride would smuggle the periodic
  fixture's adversarial property into the fixture meant to be realistic) and continuous floats.

Both are reported for every format and variant. Neither is the headline.

## What this harness was carrying before #485

The audit that motivated the scope change found the published matrix was wrong, not merely
under-specified:

- `adapters/hdf5.py` and `adapters/nexus.py` wrote `shuffle=False` with gzip-4 on **both**
  modalities — the #487 omission, in two places.
- `adapters/parquet.py` wrote plain `compression="zstd"`, no split, dictionary on — the #503 omission.
- `adapters/zarr_.py` wrote a bare `ZstdCodec`, no shuffle.
- The table fixture was the **periodic** one only.
- `adapters/root.py` was labelled `ROOT (uproot/TTree)` with `CODEC = "zstd-3, TTree"` — but uproot
  5.7 writes a **`ROOT::RNTuple`** for a dict assignment. Every ROOT row was RNTuple's numbers under
  TTree's name. This one ran **against** Tessera (RNTuple was ~11% smaller), which is worth stating:
  the audit was not one-directional.

The first four all favoured Tessera on size, and **they interacted**: a fixture chosen against
value-distribution codecs paired with a deflate configuration that had shuffle disabled. Fixing
either alone relocates the bias rather than removing it, which is why #485's scope was widened to
land them together.

## Layout

- `common.py` — the two fixtures, the adapter contract (in its docstring), `stats()` (median+spread),
  and the cold-cache machinery (`evict`, `residency` via `mincore(2)`).
- `run.py` — the driver: generates data once, drives every adapter × variant × fixture, prints an
  ALOCA report + `results.json` (which records CPU, kernel, affinity and every library version).
- `adapters/<name>.py` — one self-contained adapter per ecosystem, each declaring `VARIANTS`.
- `test_common.py` / `test_adapters.py` — the guards. `test_adapters` round-trips every adapter at
  every variant against both fixtures, and **fails rather than skips** if the reference adapter is
  missing: a skipped reference still reports the suite green.

## Run

```sh
# inside `nix develop` (provides python + libstdc++ on LD_LIBRARY_PATH)
cd tessera/bench/ecosystems
# Build the extension and assemble the `tessera` package next to the driver.
# NOTE: the cdylib is `lib_native.so`, NOT `libtessera.so` -- tessera-py's module was renamed to
# `tessera._native` and is wrapped by a pure-Python `tessera/__init__.py`. Copying a file called
# `libtessera.so` (as an earlier version of this README said) silently yields no importable
# `tessera`, the reference adapter is then SKIPPED, and the suite still reports green.
cargo build -p tessera-py --release            # from tessera/
mkdir -p tessera
cp ../../crates/tessera-py/python/tessera/{__init__.py,py.typed} tessera/
cp "${CARGO_TARGET_DIR:-../../target}/release/lib_native.so" tessera/_native.so
uv sync
uv run python -m pytest -q                     # contract + bit-exactness first

# pin to a quiet core-slice on a busy box:
taskset -c 10-39 nice -n 19 uv run python run.py --iters 5
```

Absolute MB/s is machine- and slice-dependent — `results.json` records the box so a number is never
quoted without it. The portable findings are the **compression ratios between formats on identical
input**, the **partial-read speedups**, the **cold/warm gap**, and what each format's integrity
mechanism does and does not prove.

`tessera.so` and `.venv/` are build artifacts (gitignored); the adapters, driver and tests are the
reproducible part.
