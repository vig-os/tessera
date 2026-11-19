"""Shared scaffold for the cross-ecosystem I/O comparison (#143, method hardened in #485).

The driver (`run.py`) generates TWO synthetic volumes + TWO synthetic tables, then drives every
ecosystem adapter through the same operations and times them identically. Each adapter lives in
`adapters/<name>.py` and implements this contract:

    NAME: str                      # display name, e.g. "HDF5 (h5py)"
    VARIANTS: dict                 # {variant_key: settings}, MUST contain at least "default" and
                                   # "tuned" (see below). `settings` is either a string, or a
                                   # {modality: string} dict when the layout DIFFERS BY MODALITY --
                                   # see `settings_for`.
    CAPS: dict                     # {"volume": bool, "table": bool, "swmr": bool}

    # Volume — vol is an int16 C-contiguous ndarray, shape (D, H, W):
    def write_volume(path: str, vol, variant: str) -> None
    def read_volume(path: str, variant: str)        -> "ndarray"   # full, == vol
    def read_volume_zslice(path, z, variant: str)   -> "ndarray"   # one axial plane (H, W)

    # Table — cols is a dict[str, 1-D ndarray] of equal length:
    def write_table(path: str, cols: dict, variant: str) -> None
    def read_table(path: str, variant: str)         -> dict        # full
    def read_table_column(path, name, variant: str) -> "ndarray"   # one column

Unsupported modality → raise NotImplementedError (the driver skips it per CAPS). Correctness is
asserted (read back == written) before any timing counts.

WHY VARIANTS (#485): every format is reported at a sensible DEFAULT and at its STANDARD TUNING,
with the settings string printed on every row. #487 and #503 each found that omitting a format's
standard tuning flatters Tessera by a large factor -- HDF5 without `shuffle` (5.9 MiB → 155.3 KiB
once added, which inverted that headline) and Parquet without BYTE_STREAM_SPLIT (39.3 → 23.4 MiB,
turning a claimed 2.0x win into 1.19x). A single hard-coded codec per adapter cannot express this,
so the contract carries the variant.
"""

from __future__ import annotations

import ctypes
import mmap
import os
import statistics
import time

import numpy as np

# Dataset sizes — chosen to match the Rust .tsra-vs-bare bench (bench_compare.rs) for continuity.
VOL_N = 256  # 256^3 int16 = 32 MiB raw
TABLE_ROWS = 1_000_000  # u8 + 2x f4 = ~15 MiB raw

FIXTURES = ("periodic", "continuous")


def settings_for(mod, variant: str, modality: str) -> str:
    """The settings string for one variant AND MODALITY.

    Per-modality because a settings line must say what was ACTUALLY used. #487 shipped a draft that
    printed "chunked 64^3 (= tessera's)" on the TABLE rows while the code chunked 1-D at 65536 rows
    -- a fairness claim the code did not honour. Any adapter whose layout differs between volume and
    table maps the variant to {modality: string} rather than one string covering both.
    """
    v = mod.VARIANTS[variant]
    if isinstance(v, dict):
        try:
            return v[modality]
        except KeyError:
            raise KeyError(
                f"{mod.NAME}/{variant}: no settings string for modality {modality!r} "
                f"(has {sorted(v)}) -- a row cannot be printed without saying what it used"
            ) from None
    return v


VOLUMES = ("gradient", "acquired")


def make_volume(kind: str = "acquired") -> np.ndarray:
    """One of the two volume fixtures. BOTH are always reported, for the same reason as the tables
    (#497): same encoder, same command, opposite verdicts.

    gradient
        A pure linear ramp in z and y. Its deltas are CONSTANT, so LZ77 match lengths run the
        length of the array — ideal for deflate/zstd and adversarial for value-distribution codecs,
        exactly as the `periodic` table fixture is. This was the harness's only volume fixture.

        It is worse than merely unrepresentative: its verdict **depends on the fixture size**.
        Measured pcodec/zstd size ratio at n = 64/128/192/256/320 → 1.11, 1.25, 1.39, 1.65, 0.41.
        zstd's advantage grows as match lengths scale, then inverts once the pattern outgrows its
        window. A benchmark whose answer moves with the array size is not measuring the codec, and
        the 256 this harness uses sits near the worst point for pcodec.

    acquired
        Anatomy plus **detector noise** — what a real CT/PET reconstruction contains. The noise is
        what destroys long LZ matches while remaining exactly what a numeric codec models. Its
        ratio is SIZE-INVARIANT: pcodec/zstd = 0.85, 0.83, 0.82, 0.82, 0.82 across the same sizes,
        i.e. pcodec is ~18% smaller at every scale. That tracks the -21% CT / -33% PET measured on
        real acquisitions, which is the test of whether a fixture is representative.
    """
    if kind not in VOLUMES:
        raise ValueError(f"unknown volume fixture {kind!r}; expected one of {VOLUMES}")
    n = VOL_N
    if kind == "gradient":
        z = np.arange(n, dtype=np.int64)[:, None, None]
        y = np.arange(n, dtype=np.int64)[None, :, None]
        x = np.zeros((1, 1, n), dtype=np.int64)
        return np.ascontiguousarray((z * 8 + y * 2 - 1024 + x).astype("<i2"))

    # A spherical phantom (soft tissue ~40 HU, a denser core ~340 HU, air -1000) plus Gaussian
    # detector noise. Fixed seed so the fixture is byte-reproducible across runs.
    rng = np.random.default_rng(20240101)
    zz, yy, xx = np.meshgrid(*[np.arange(n)] * 3, indexing="ij")
    r = np.sqrt((zz - n / 2) ** 2 + (yy - n / 2) ** 2 + (xx - n / 2) ** 2)
    body = np.where(r < n * 0.4, 40.0, -1000.0) + np.where(r < n * 0.15, 300.0, 0.0)
    body = body + rng.normal(0.0, 25.0, size=body.shape)
    return np.ascontiguousarray(np.clip(body, -1024, 3071).astype("<i2"))


# ---------------------------------------------------------------- table fixtures (#497)
def _xorshift64star(seed: int, n: int) -> np.ndarray:
    """The same generator #497 uses, so the two benchmarks' fixtures are comparable and both stay
    byte-reproducible independently of the numpy version's RNG stream policy."""
    mask = (1 << 64) - 1
    x = seed & mask
    out = np.empty(n, dtype=np.uint64)
    for i in range(n):
        # All three steps are XOR-assignments. Writing the middle one as a plain assignment
        # (`x = (x << 25) & mask`) drops 39 bits of state every iteration and the generator
        # degenerates: it produced 1143 distinct values in 1,000,000 draws, which made the
        # "continuous" fixture categorical -- the exact property it exists NOT to have.
        x ^= x >> 12
        x ^= (x << 25) & mask
        x ^= x >> 27
        out[i] = (x * 0x2545F4914F6CDD1D) & mask
    return out


def make_table(kind: str = "periodic") -> dict:
    """One of the two fixtures. BOTH are always reported (#497): same encoder, same command,
    opposite verdicts — publishing one without the other is choosing the answer.

    periodic
        `t` a dense counter, `e0`/`e1` with exact periods 7 and 5. **Adversarial for
        value-distribution codecs**: deflate's LZ77 window locks onto the repeating byte block,
        while Pco / dictionary / bit-packing model the value distribution and cannot exploit
        periodicity at all. HDF5 shuffle+gzip wins here by ~6x (#493/#497).

    continuous
        Listmode-like, shaped after what #493 measured on real `/events_2p`: `t` is a coarse
        millisecond clock advancing once per ~250 events with **Poisson** run lengths (an exact
        stride would smuggle the periodic fixture's adversarial property into the fixture that is
        supposed to be realistic), and `e0`/`e1` are drawn from a continuous distribution.
    """
    if kind not in FIXTURES:
        raise ValueError(f"unknown fixture {kind!r}; expected one of {FIXTURES}")
    r = TABLE_ROWS
    if kind == "periodic":
        return {
            "t": np.arange(r, dtype="<u8"),
            "e0": (511.0 + (np.arange(r) % 7)).astype("<f4"),
            "e1": (510.0 - (np.arange(r) % 5)).astype("<f4"),
        }

    # Poisson tick positions: exponential inter-arrival gaps, mean ~250 events per clock tick.
    raw = _xorshift64star(0x2545F4914F6CDD1D, r + (r // 100) + 64)
    u = (raw[: r // 100 + 64] >> np.uint64(11)).astype(np.float64) / float(1 << 53)
    gaps = -250.0 * np.log(np.clip(u, 1e-12, 1.0))
    ticks = np.cumsum(gaps)
    t = np.searchsorted(ticks, np.arange(r), side="right").astype("<u8")

    # e0 and e1 must be INDEPENDENT draws. Deriving e1 from e0 (`450 + 120*(1 - f)`) made them
    # perfectly anti-correlated (r = -1.0): the second column carried no information the first did
    # not, which is not what a two-detector energy pair looks like and flatters any format that
    # happens to exploit it.
    half = r // 2
    g = _xorshift64star(0x9E3779B97F4A7C15, 2 * r)
    f0 = (g[:r] >> np.uint64(11)).astype(np.float64) / float(1 << 53)
    f1 = (g[r:] >> np.uint64(11)).astype(np.float64) / float(1 << 53)
    del half
    e0 = (450.0 + 120.0 * f0).astype("<f4")
    e1 = (450.0 + 120.0 * f1).astype("<f4")
    return {"t": t, "e0": e0, "e1": e1}


# ---------------------------------------------------------------- timing (#485)
def stats(fn, iters: int, _elapsed=None) -> dict:
    """Median wall-clock with spread over `iters` runs.

    Replaces min-of-N (#485). The minimum is a throughput ceiling -- defensible, but it hides
    variance and **cannot show when two formats are within noise of each other**, which is the
    question a comparison table exists to answer. Reports median, [min..max] and n so a reader can
    see the overlap.

    `_elapsed` is a test seam returning one run's duration; production always times `fn`.
    """
    if iters < 1:
        raise ValueError(
            f"iters must be >= 1, got {iters}: an empty sample has no median"
        )
    samples = []
    for _ in range(iters):
        if _elapsed is not None:
            fn()
            samples.append(float(_elapsed()))
        else:
            t0 = time.perf_counter()
            fn()
            samples.append(time.perf_counter() - t0)
    return {
        "median": statistics.median(samples),
        "min": min(samples),
        "max": max(samples),
        "n": len(samples),
    }


# ---------------------------------------------------------------- cold cache (#485)
_libc = ctypes.CDLL("libc.so.6", use_errno=True)
_PAGE = mmap.PAGESIZE

# mmap(2)/munmap(2) via ctypes rather than Python's `mmap` module: mincore needs a raw address, and
# `ctypes.from_buffer` refuses a read-only mapping ("underlying buffer is not writable"). Mapping
# PROT_READ|MAP_PRIVATE here keeps the mapping read-only without that detour.
_PROT_READ = 0x1
_MAP_PRIVATE = 0x02
_MAP_FAILED = ctypes.c_void_p(-1).value
_libc.mmap.restype = ctypes.c_void_p
_libc.mmap.argtypes = [
    ctypes.c_void_p,
    ctypes.c_size_t,
    ctypes.c_int,
    ctypes.c_int,
    ctypes.c_int,
    ctypes.c_long,
]
_libc.munmap.restype = ctypes.c_int
_libc.munmap.argtypes = [ctypes.c_void_p, ctypes.c_size_t]
_libc.mincore.restype = ctypes.c_int
_libc.mincore.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_char_p]


def _files_under(path: str) -> list[str]:
    """Every regular file at `path` — Zarr stores and DICOM series are DIRECTORIES, and evicting a
    single fd would leave the rest of the store warm while the row claims to be cold."""
    if os.path.isdir(path):
        out = []
        for root, _dirs, files in os.walk(path):
            out.extend(os.path.join(root, f) for f in files)
        return out
    return [path]


def _residency_one(path: str) -> tuple[int, int]:
    """(resident_pages, total_pages) for one file, via mincore(2)."""
    size = os.path.getsize(path)
    if size == 0:
        return (0, 0)
    fd = os.open(path, os.O_RDONLY)
    try:
        addr = _libc.mmap(None, size, _PROT_READ, _MAP_PRIVATE, fd, 0)
    finally:
        os.close(fd)
    if addr == _MAP_FAILED or not addr:
        err = ctypes.get_errno()
        raise OSError(err, f"mmap failed on {path}: {os.strerror(err)}")
    try:
        npages = (size + _PAGE - 1) // _PAGE
        vec = ctypes.create_string_buffer(npages)
        if _libc.mincore(ctypes.c_void_p(addr), ctypes.c_size_t(size), vec) != 0:
            err = ctypes.get_errno()
            raise OSError(err, f"mincore failed on {path}: {os.strerror(err)}")
        # mincore sets bit 0 of each byte when that page is resident; other bits are undefined.
        resident = int((np.frombuffer(vec.raw, dtype=np.uint8) & 1).sum())
        return (resident, npages)
    finally:
        _libc.munmap(ctypes.c_void_p(addr), size)


def residency(path: str) -> float:
    """Fraction of `path`'s pages currently in the page cache, 0.0..1.0.

    A missing path raises rather than returning 0.0 — "0% resident" reads as *perfectly evicted*,
    which is exactly the reading a missing file must not be able to earn.
    """
    files = _files_under(path)
    if not files:
        raise FileNotFoundError(f"no files under {path}")
    res = tot = 0
    for f in files:
        r, t = _residency_one(f)
        res += r
        tot += t
    return (res / tot) if tot else 0.0


def evict(path: str) -> float:
    """Drop `path` from the page cache and RETURN THE MEASURED worst-case residency after doing so.

    Two hard-won details (#487):

    1. **fsync first.** `POSIX_FADV_DONTNEED` silently skips DIRTY pages, so evicting a file that
       was just written evicts nothing. #487's first implementation had cold matching warm to four
       decimal places for exactly this reason.
    2. **Measure, do not assume.** Eviction is best-effort — the kernel may decline, and another
       process may re-populate the cache between the call and the read. A cold-labelled row the
       kernel actually served from RAM is worse than no row at all, so the caller prints what this
       returns and the report says which it got.
    """
    worst = 0.0
    for f in _files_under(path):
        fd = os.open(f, os.O_RDONLY)
        try:
            os.fsync(fd)
            os.posix_fadvise(fd, 0, 0, os.POSIX_FADV_DONTNEED)
        finally:
            os.close(fd)
        r, t = _residency_one(f)
        if t:
            worst = max(worst, r / t)
    return worst


def dir_or_file_bytes(path: str) -> int:
    """On-disk size of a file or (for Zarr-style stores) a directory tree."""
    if os.path.isdir(path):
        total = 0
        for root, _dirs, files in os.walk(path):
            for f in files:
                total += os.path.getsize(os.path.join(root, f))
        return total
    return os.path.getsize(path)
