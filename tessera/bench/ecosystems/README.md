# Cross-ecosystem I/O comparison (#143)

Compares Tessera `.tsra` against the storage formats it competes with — **HDF5, Zarr, NeXus, NIfTI,
DICOM, Parquet** — on the same synthetic data, through one driver that times every format
identically.

## What it measures

Two datasets (sized to match the Rust `.tsra`-vs-bare bench, `crates/tessera-io/examples/bench_compare.rs`):
- **Volume** — int16 CT-like 256³ (32 MiB raw).
- **Table** — listmode `u8 + 2×f4`, 1M rows (15 MiB raw).

Per format, per modality it records: compression ratio, on-disk size, write throughput, full
sequential-read throughput, and **partial access** (a single Z-slice for volumes / a single column for
tables). Reads are warm (page-cache) → this is decode/parse throughput, not cold disk. Every adapter is
asserted **bit-exact** (read-back == written) before its timings count. SWMR / concurrent-reader support
is reported as a capability flag.

Not every format does every modality — NIfTI and DICOM are volume-only, Parquet is table-only; the
driver skips per each adapter's `CAPS`.

## Layout

- `common.py` — synthetic datasets + the adapter contract (in its docstring) + timing helpers.
- `run.py` — the driver: generates data once, drives every adapter, prints an ALOCA report + `results.json`.
- `adapters/<name>.py` — one self-contained adapter per ecosystem (each picks a sensible lossless
  codec, documented in its `CODEC`). `tessera.py` is the reference (uses the real `tessera.so`).

## Run

```sh
# inside `nix develop` (provides python + libstdc++ on LD_LIBRARY_PATH)
cd tessera/bench/ecosystems
cp ../../target/release/libtessera.so ./tessera.so   # build first: cargo build -p tessera-py --release
uv sync
# pin to a quiet core-slice on a busy box, take min-of-N:
taskset -c 10-39 nice -n 19 uv run python run.py --vol-iters 5 --tab-iters 5
```

Absolute MB/s is machine + slice dependent; the portable findings are **compression ratio, the
self-describing/content-addressed envelope each format carries, and the partial-read speedups**.
`tessera.so` and `.venv/` are build artifacts (gitignored); the adapters + driver are the reproducible part.
