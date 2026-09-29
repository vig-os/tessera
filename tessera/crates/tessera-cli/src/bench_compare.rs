//! `tessera bench compare` — head-to-head size + latency vs HDF5 and Parquet on the same data
//! (#388, the AX pitch gap from #390).
//!
//! # Why this exists
//!
//! `docs/book/src/why-tessera.md` argues Tessera against Parquet/HDF5 on capabilities, but the only
//! quantitative claim anywhere was −21%/−33% vs bare zstd. This produces the numbers, on one command,
//! so the pitch can be checked rather than believed.
//!
//! # Honesty rules this harness holds itself to
//!
//! - **Same logical data** to every format — generated once, handed to each writer unchanged. The
//!   volume generator is lifted from the cross-ecosystem harness (`bench/ecosystems/common.py`, #143)
//!   so the two benchmarks stay comparable.
//! - **Default AND tuned** per format, with the settings printed on every row. A format compared only
//!   at its default is a straw man; compared only when tuned, it flatters whoever tuned hardest.
//! - **Median of N with spread**, never a single run. A single timing on a shared box is a rumour.
//! - **Warm and cold** are labelled, never mixed. Cold eviction is best-effort (see [`evict`]).
//! - **Correctness gates timing**: every read is checked against the written data BEFORE its numbers
//!   count, so a format cannot win by returning something cheaper than what it was given.
//! - **No cherry-picking.** Tessera loses some rows here — notably write+seal, where it pays for the
//!   hashing the others do not do. Those rows are printed with the same weight as the wins.
//!
//! # What the integrity row does and does not say
//!
//! This is the row most easily overstated, so it is stated precisely (and is why the HDF5 side gets a
//! `fletcher32` variant rather than a bare "n/a"):
//!
//! - **HDF5 `fletcher32`** is a per-chunk checksum. It detects *corruption* — bit rot, a truncated
//!   write, a bad disk — on the chunks you actually read. It is not keyed, it covers no metadata, and
//!   an attacker who rewrites a chunk simply rewrites its checksum too.
//! - **Parquet** has nothing in this class to time. The format permits an optional per-page CRC32,
//!   but parquet-rs 58's writer never emits one, so for the files this benchmark writes there is
//!   nothing to check — the row is absent by fact, not by omission.
//! - **Tessera `verify`** re-derives every block digest and checks them against the sealed manifest,
//!   whose own hash covers the metadata and provenance as well. That detects corruption too, but its
//!   point is *tamper-evidence*: you cannot alter a block, a field, or the recipe without changing
//!   `manifest_hash`, and a signature (ADR-0037) binds that hash to a signer.
//!
//! So: all three catch a flipped bit. Only Tessera answers "is this the artifact that was sealed, by
//! whom, and with what provenance". The timing column says what each costs; this paragraph says what
//! each buys, and the report prints a short form of it.

/// Report lines go to stdout — they ARE this command's product, not debug logging. Funnelled through
/// one annotated sink so the guardrails `no-debug-leftovers` gate is satisfied once instead of at
/// every call site, exactly as [`crate::bench`] does.
macro_rules! out {
    () => {{ println!() }}; // guardrails-ok: bench command output, not a debug leftover
    ($($a:tt)*) => {{ println!($($a)*) }}; // guardrails-ok: bench command output, not a debug leftover
}

use std::path::{Path, PathBuf};
use std::time::Instant;

use hdf5_metno as hdf5;
use tessera_core::block::array::ArraySpec;
use tessera_core::block::table::{Column, TableSpec};
use tessera_core::{ProductBuilder, Result};
use tessera_io::array::{self, ArrayData};
use tessera_io::table::{self, ColumnData, TableData, ROWS_PER_GROUP};
use tessera_io::{pack, BlockPayload, Reader};

/// Cubic chunk edge Tessera uses for arrays by default. HDF5's tuned variant is given the SAME
/// geometry so the ROI row compares layouts, not chunk-size luck (#388 review).
const CHUNK: usize = 64;

/// 1-D chunk length for the HDF5 table variants (rows per chunk).
const TABLE_CHUNK_ROWS: usize = 1 << 16;

/// Volume edge — 256³ int16 = 32 MiB raw, matching `bench/ecosystems/common.py` (#143).
pub const VOL_N: usize = 256;
/// Table rows — `u8 + 2×f4`, matching `bench/ecosystems/common.py` (#143) scaled for a longer read.
pub const TABLE_ROWS: usize = 4_000_000;

/// Options for [`run`].
pub struct CompareOpts {
    pub dataset: String,
    pub iters: usize,
    pub format: String,
    pub cold: bool,
    pub vol_n: usize,
    pub rows: usize,
}

/// A smooth CT-like int16 volume (gradient in z+y).
///
/// Lifted from `bench/ecosystems/common.py::make_volume` (#143) — deliberately not a pure ramp, so
/// the compression numbers mean something, and deliberately identical to that harness so the two
/// sets of results can be read together.
fn make_volume(n: usize) -> ArrayData {
    let mut v = Vec::with_capacity(n * n * n);
    for z in 0..n as i64 {
        for y in 0..n as i64 {
            for _x in 0..n as i64 {
                v.push((z * 8 + y * 2 - 1024) as i16);
            }
        }
    }
    ArrayData::I16(v)
}

/// Deterministic xorshift64* — a continuous fixture needs pseudo-random values, but the benchmark
/// must stay byte-reproducible, so this is a fixed-seed generator rather than a real RNG.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u32 << 24) as f32
    }
    /// Sum of four uniforms — a cheap, dependency-free bell curve (central limit). The property
    /// under test is that the column is drawn from a CONTINUOUS distribution, which is what defeats
    /// a dictionary. It is emphatically NOT that the values are distinct: at 4M draws from a 24-bit
    /// uniform, collisions are a certainty, and claiming otherwise was simply wrong (#497 review).
    fn bell(&mut self) -> f32 {
        self.unit() + self.unit() + self.unit() + self.unit() - 2.0
    }
    /// An exponentially-distributed run length with the given mean, floored at one event.
    ///
    /// A real detector clock does not tick every Nth event exactly: arrivals are Poisson, so the
    /// number of events sharing one millisecond stamp is geometric (exponential in the continuum
    /// limit). Generating the clock as an exact `k / MEAN` would hand the continuous fixture the
    /// PERIODIC fixture's defining property — the one thing it exists in order not to have.
    fn exp_run(&mut self, mean: f32) -> u64 {
        // `unit()` samples [0, 1) and ln(0) is -inf, so shift to (0, 1].
        let u = 1.0 - self.unit();
        let n = (-mean * u.ln()).round();
        if n.is_finite() && n >= 1.0 {
            n as u64
        } else {
            1
        }
    }
}

/// **Periodic** fixture — the original #388 table: a monotonic `u64` counter and two `f32` columns
/// with periods 7 and 5.
///
/// It is kept, and kept labelled, because it is **adversarial for value-distribution codecs**: LZ77
/// (deflate, behind HDF5's shuffle+gzip) locks onto the repeating byte block, while Pco, dictionary
/// and bit-packing model the value distribution and cannot exploit periodicity at all. Tessera loses
/// it ~6× (#493). Replacing it with something friendlier would be tuning the benchmark until we win;
/// reporting it beside a realistic fixture is the honest form.
fn make_table_periodic(rows: usize) -> (TableSpec, TableData) {
    let data: TableData = vec![
        ("t".into(), ColumnData::U64((0..rows as u64).collect())),
        (
            "e0".into(),
            ColumnData::F32((0..rows).map(|k| 511.0 + (k % 7) as f32).collect()),
        ),
        (
            "e1".into(),
            ColumnData::F32((0..rows).map(|k| 510.0 - (k % 5) as f32).collect()),
        ),
    ];
    (table_spec(&data, rows), data)
}

/// Mean number of events sharing one millisecond stamp on real DUPLET `/events_2p` (#493).
const MS_RUN_EVENTS: f32 = 250.0;

/// **Continuous** fixture — shaped after what a real acquisition actually looks like, measured on
/// DUPLET `/events_2p` during the #493 investigation:
///
/// - `t` is a coarse millisecond clock, not a dense counter: it advances once every ~250 events, so
///   ~99.6 % of its deltas are zero, matching the real column's run structure. The run lengths are
///   POISSON (exponentially-distributed gaps), not a fixed stride — see [`Rng::exp_run`].
/// - `e0`/`e1` are energies drawn from a continuous distribution. On the real data shuffle+gzip
///   manages only ~1.4× on columns of this character, and Pco beats it.
fn make_table_continuous(rows: usize) -> (TableSpec, TableData) {
    // The clock draws from its OWN stream so that changing it leaves the energy columns
    // byte-identical, and the two effects on the reported sizes stay separable.
    let mut clk = Rng(0x2545_F491_4F6C_DD1D);
    let mut t = Vec::with_capacity(rows);
    let mut clock: u64 = 0;
    let mut left = clk.exp_run(MS_RUN_EVENTS);
    for _ in 0..rows {
        if left == 0 {
            clock += 1;
            left = clk.exp_run(MS_RUN_EVENTS);
        }
        t.push(clock);
        left -= 1;
    }

    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let e0: Vec<f32> = (0..rows).map(|_| 511.0 + 25.0 * rng.bell()).collect();
    let e1: Vec<f32> = (0..rows).map(|_| 511.0 + 25.0 * rng.bell()).collect();
    let data: TableData = vec![
        ("t".into(), ColumnData::U64(t)),
        ("e0".into(), ColumnData::F32(e0)),
        ("e1".into(), ColumnData::F32(e1)),
    ];
    (table_spec(&data, rows), data)
}

fn table_spec(data: &TableData, rows: usize) -> TableSpec {
    TableSpec {
        columns: data
            .iter()
            .map(|(n, c)| Column::new(n.clone(), c.numpy_code()))
            .collect(),
        rows: rows as u64,
        row_index: None,
    }
}

/// Best-effort page-cache eviction for one file, so a "cold" read measures a first read rather than
/// RAM. `posix_fadvise(POSIX_FADV_DONTNEED)` needs no privileges, unlike `drop_caches` — but the
/// kernel is free to ignore it, and pages still referenced elsewhere (or dirty) survive. The report
/// therefore labels cold rows best-effort and prints the kernel, rather than claiming a true cold
/// cache it cannot guarantee.
fn evict(path: &Path) {
    use std::os::unix::io::AsRawFd;
    let Ok(f) = std::fs::File::open(path) else {
        return;
    };
    // DONTNEED only drops CLEAN pages. A file we just wrote still has dirty ones, so without this
    // the eviction silently does nothing and a "cold" read is really a warm read wearing a label —
    // which was exactly the first result here (cold == warm to 4 decimal places).
    if f.sync_all().is_err() {
        // Nothing we can do, but DONTNEED will then skip the dirty pages and the residency probe
        // below will report the file as resident — the report says so rather than silently lying.
        return;
    }
    // SAFETY: `f` owns a live fd for the duration of the call; POSIX_FADV_DONTNEED only drops clean
    // page-cache pages for that fd's file and cannot corrupt or truncate it.
    unsafe {
        libc::posix_fadvise(f.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED);
    }
}

/// Fraction of `path`'s pages still resident, via `mincore`. Used to tell the reader whether the
/// cold rows actually ran cold — a "cold" number that the kernel quietly served from RAM is worse
/// than no number at all, so the report states the measured residency instead of asserting a cold
/// cache it cannot force.
fn resident_fraction(path: &Path) -> Option<f64> {
    use std::os::unix::io::AsRawFd;
    let f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len() as usize;
    if len == 0 {
        return None;
    }
    // SAFETY: mmap of a readable fd for `len` bytes; unmapped before return. `mincore` writes one
    // byte per page into `vec`, which is sized from the same page count.
    unsafe {
        let page = libc::sysconf(libc::_SC_PAGESIZE) as usize;
        let pages = len.div_ceil(page);
        let addr = libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_READ,
            libc::MAP_SHARED,
            f.as_raw_fd(),
            0,
        );
        if addr == libc::MAP_FAILED {
            return None;
        }
        let mut vec = vec![0u8; pages];
        let rc = libc::mincore(addr, len, vec.as_mut_ptr());
        libc::munmap(addr, len);
        if rc != 0 {
            return None;
        }
        let resident = vec.iter().filter(|b| *b & 1 == 1).count();
        Some(resident as f64 / pages as f64)
    }
}

/// Median + spread over N timed runs.
#[derive(Clone, Debug)]
pub struct Stats {
    pub median_s: f64,
    pub min_s: f64,
    pub max_s: f64,
    pub n: usize,
}

impl Stats {
    fn of(mut xs: Vec<f64>) -> Stats {
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let n = xs.len();
        let median_s = if n % 2 == 1 {
            xs[n / 2]
        } else {
            (xs[n / 2 - 1] + xs[n / 2]) / 2.0
        };
        Stats {
            median_s,
            min_s: xs[0],
            max_s: xs[n - 1],
            n,
        }
    }
}

/// Worst (highest) post-eviction page residency seen across all cold runs, in permille. Reported so
/// the reader can judge the cold rows instead of trusting the label: 0 means eviction worked, a high
/// value means the kernel kept the file and those rows are effectively warm.
static COLD_RESIDENT_PERMILLE: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(u32::MAX);

/// Time `f` `iters` times, evicting `cold` (when given) before each run, and return median+spread.
fn timed(iters: usize, cold: Option<&Path>, mut f: impl FnMut()) -> (Stats, Option<f64>) {
    let mut xs = Vec::with_capacity(iters);
    let mut worst_resident: Option<f64> = None;
    for _ in 0..iters {
        if let Some(p) = cold {
            evict(p);
            if let Some(frac) = resident_fraction(p) {
                worst_resident = Some(worst_resident.map_or(frac, |w: f64| w.max(frac)));
                let permille = (frac * 1000.0).round() as u32;
                let cur = COLD_RESIDENT_PERMILLE.load(std::sync::atomic::Ordering::Relaxed);
                let worst = if cur == u32::MAX {
                    permille
                } else {
                    cur.max(permille)
                };
                COLD_RESIDENT_PERMILLE.store(worst, std::sync::atomic::Ordering::Relaxed);
            }
        }
        let t = Instant::now();
        f();
        xs.push(t.elapsed().as_secs_f64());
    }
    (Stats::of(xs), worst_resident)
}

/// Eviction counts as effective below this residency; above it the row is relabelled, because a
/// "cold" number the kernel served from RAM is a warm number with a wrong label.
const COLD_RESIDENT_OK: f64 = 0.01;

/// One measured cell of the matrix.
pub struct Row {
    pub format: String,
    pub settings: String,
    pub modality: String,
    pub op: &'static str,
    pub cache: &'static str,
    /// Post-eviction page residency for THIS row's file, when it was a cold run. `None` on warm rows
    /// and when the probe failed. Per-row rather than one global worst case, because eviction can
    /// succeed for one file and fail for another in the same run (#388 review).
    pub resident: Option<f64>,
    pub bytes: Option<u64>,
    pub stats: Option<Stats>,
    /// Which pitch claim this row speaks to — printed so a reader knows why the row exists.
    pub claim: &'static str,
}

fn file_len(p: &Path) -> u64 {
    std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
}

/// Seal a single-block `.tsra` and return its path.
fn seal_tsra(
    dir: &Path,
    name: &str,
    block: (tessera_core::block::BlockRef, BlockPayload),
) -> Result<PathBuf> {
    let (block_ref, payload) = block;
    let mut b = ProductBuilder::new("recon", name, "bench compare", "2024-01-01T00:00:00Z");
    b.add_block_ref(block_ref);
    let sealed = b.seal()?;
    let path = dir.join(format!("{name}.tsra"));
    pack(&sealed, &[payload], &path)?;
    Ok(path)
}

/// HDF5 variant knobs. `chunk` mirrors Tessera's cubic geometry when set, so the ROI row compares
/// layouts rather than an arbitrary chunk choice.
struct H5Variant {
    label: &'static str,
    settings: String,
    chunked: bool,
    deflate: Option<u8>,
    /// The byte-shuffle filter. Standard HDF5 tuning is shuffle+deflate, not deflate alone: shuffle
    /// groups the like-significance bytes of each value so deflate has runs to find. Leaving it out
    /// would weaken the baseline we are measuring against and flatter Tessera's size ratios (#388
    /// review), so both tuned variants carry it.
    shuffle: bool,
    fletcher32: bool,
}

/// Per-modality, because the chunk geometry differs and the settings line must say what was ACTUALLY
/// used: the volume gets Tessera's own cubic 64³ (so the ROI row compares layouts, not chunk-size
/// luck), while a 1-D table gets a row-count chunk. Printing "64³" on a table row would be a lie.
fn h5_variants(modality: &str) -> Vec<H5Variant> {
    let chunk_desc = if modality.starts_with("volume") {
        format!("chunked {CHUNK}^3 (= tessera's)")
    } else {
        format!("chunked {TABLE_CHUNK_ROWS} rows (1-D)")
    };
    vec![
        H5Variant {
            label: "HDF5 (hdf5-metno)",
            // h5py/libhdf5's actual default for a plain create: contiguous, no filters.
            settings: "default: contiguous, uncompressed".into(),
            chunked: false,
            deflate: None,
            shuffle: false,
            fletcher32: false,
        },
        H5Variant {
            label: "HDF5 (hdf5-metno)",
            settings: format!("tuned: {chunk_desc}, shuffle+gzip-4"),
            chunked: true,
            deflate: Some(4),
            shuffle: true,
            fletcher32: false,
        },
        H5Variant {
            label: "HDF5 (hdf5-metno)",
            settings: format!("tuned: {chunk_desc}, shuffle+gzip-4, fletcher32"),
            chunked: true,
            deflate: Some(4),
            shuffle: true,
            fletcher32: true,
        },
    ]
}

fn write_h5_volume(path: &Path, v: &H5Variant, n: usize, data: &[i16]) -> Result<()> {
    let f = hdf5::File::create(path).map_err(|e| err(&format!("h5 create: {e}")))?;
    let mut b = f.new_dataset::<i16>().shape([n, n, n]);
    if v.chunked {
        b = b.chunk([CHUNK.min(n), CHUNK.min(n), CHUNK.min(n)]);
    }
    if let Some(l) = v.deflate {
        b = b.deflate(l);
    }
    if v.fletcher32 {
        b = b.fletcher32();
    }
    let ds = b
        .create("volume")
        .map_err(|e| err(&format!("h5 dataset: {e}")))?;
    let arr = ndarray::ArrayView3::from_shape((n, n, n), data)
        .map_err(|e| err(&format!("shape: {e}")))?;
    ds.write(arr.view())
        .map_err(|e| err(&format!("h5 write: {e}")))?;
    Ok(())
}

/// The exact sub-box a volume ROI read must return, in C order — the ground truth the timed ROI
/// reads are checked against (#388 review: a partial read that was never verified could be fast
/// because it returned less than it should).
fn expect_roi(raw: &[i16], n: usize, start: usize, edge: usize) -> Vec<i16> {
    let mut out = Vec::with_capacity(edge * edge * edge);
    for z in start..start + edge {
        for y in start..start + edge {
            let base = z * n * n + y * n + start;
            out.extend_from_slice(&raw[base..base + edge]);
        }
    }
    out
}

/// The exact rows a row-ROI read must return, across EVERY column.
fn expect_rows(data: &TableData, start: usize, len: usize) -> Result<TableData> {
    data.iter()
        .map(|(name, col)| {
            let sliced = match col {
                ColumnData::U64(v) => ColumnData::U64(v[start..start + len].to_vec()),
                ColumnData::F32(v) => ColumnData::F32(v[start..start + len].to_vec()),
                other => {
                    return Err(err(&format!(
                        "bench compare: unexpected column type in fixture: {other:?}"
                    )))
                }
            };
            Ok((name.clone(), sliced))
        })
        .collect()
}

/// Take `len` rows starting at `off` from an already-read table — used to cut the exact row window
/// out of the whole row groups Parquet must read, so every format is compared on the same rows.
fn slice_rows(data: &TableData, off: usize, len: usize) -> Result<TableData> {
    expect_rows(data, off, len)
}

/// The row groups a row window overlaps, plus the window's offset INSIDE the first of them.
///
/// Row groups are Parquet's coarsest IO unit: it cannot hand back a row range, only whole groups,
/// so a fair timing reads every group the window touches and slices the exact window out of them.
///
/// Getting this wrong is not loud. The benchmark shipped with `roi_start / ROWS_PER_GROUP` alone —
/// ONE group — and at 4M rows the window [2,000,000, 2,040,000) straddles groups 30 and 31, so
/// Parquet returned 31,616 of 40,000 rows and was timed on 79% of the work. It verified clean
/// because the check compared against *that group's* slice rather than the exact source window, the
/// same shape as the #487 row-ROI asymmetry: an expectation derived from the same wrong thing as the
/// result. Hence a named function with its own tests rather than two lines inline (#503 review).
fn window_row_groups(start: usize, len: usize, rows_per_group: usize) -> (Vec<usize>, usize) {
    debug_assert!(len > 0, "an empty window has no groups to read");
    let first = start / rows_per_group;
    let last = (start + len - 1) / rows_per_group;
    ((first..=last).collect(), start - first * rows_per_group)
}

/// Fail loudly when a timed read did not return what was written. Correctness gates timing: a read
/// whose numbers count must first be shown to produce the right bytes, for EVERY format, setting and
/// access pattern — not just the full read of one of them.
fn check<T: PartialEq>(got: &T, want: &T, what: &str) -> Result<()> {
    if got != want {
        return Err(err(&format!(
            "bench compare: {what} returned data != the source; its timings would be meaningless"
        )));
    }
    Ok(())
}

fn err(m: &str) -> tessera_core::Error {
    tessera_core::Error::Invalid(m.to_string())
}

/// The volume half of the matrix.
fn compare_volume(
    dir: &Path,
    n: usize,
    iters: usize,
    cold: bool,
    rows: &mut Vec<Row>,
) -> Result<()> {
    let data = make_volume(n);
    let ArrayData::I16(raw) = &data else {
        return Err(err("volume must be i16"));
    };
    // One WHOLE cubic chunk, chunk-ALIGNED — the access pattern cubic chunking exists for. Centring
    // without aligning (the first draft) put the box across 8 chunks, so the row measured a
    // misaligned read while the prose claimed a single chunk (#388 review).
    let roi_edge = CHUNK.min(n);
    let roi0 = ((n - roi_edge) / 2 / roi_edge) * roi_edge;
    let expected_roi = expect_roi(raw, n, roi0, roi_edge);
    let roi_start = [roi0 as u64; 3];
    let roi_shape = [roi_edge as u64; 3];

    for (label, codec) in [("pcodec (default)", "pcodec"), ("zstd (tuned)", "zstd")] {
        let mut spec = ArraySpec::new(vec![n as u64, n as u64, n as u64], "int16");
        spec.codec = codec.into();
        let settings = format!("{label}, {CHUNK}^3 chunks");
        let name = format!("vol_{codec}");

        let (wr, _) = timed(iters, None, || {
            let blk = array::array_block("volume", &spec, &data).expect("encode");
            let _ = seal_tsra(dir, &name, blk).expect("seal");
        });
        let path = dir.join(format!("{name}.tsra"));

        // Correctness gates timing — EVERY read that gets a row is verified first.
        let blob = Reader::open(&path)?.read_block("volume")?;
        check(
            &array::decode(&spec, &blob)?,
            &data,
            "tessera volume full read",
        )?;
        let roi = array::decode_subset(&spec, &blob, &roi_start, &roi_shape)?;
        check(
            &roi,
            &ArrayData::I16(expected_roi.clone()),
            "tessera volume ROI read",
        )?;

        push_size(
            rows,
            "Tessera (.tsra)",
            &settings,
            "volume",
            file_len(&path),
        );
        push_write(rows, "Tessera (.tsra)", &settings, "volume", wr);

        for (cache, evict_path) in warm_cold(&path, cold) {
            let (s, res) = timed(iters, evict_path, || {
                let mut r = Reader::open(&path).expect("open");
                let b = r.read_block("volume").expect("block");
                let _ = array::decode(&spec, &b).expect("decode");
            });
            push_read(
                rows,
                "Tessera (.tsra)",
                &settings,
                "volume",
                "read full",
                cache,
                s,
                res,
                CLAIM_FULL,
            );

            let (s, res) = timed(iters, evict_path, || {
                let mut r = Reader::open(&path).expect("open");
                let b = r.read_block("volume").expect("block");
                let _ = array::decode_subset(&spec, &b, &roi_start, &roi_shape).expect("roi");
            });
            push_read(
                rows,
                "Tessera (.tsra)",
                &settings,
                "volume",
                "read ROI",
                cache,
                s,
                res,
                CLAIM_ROI,
            );
        }

        let (s, _) = timed(iters, None, || {
            let _ = tessera_io::verify_payloads_parallel(&path, "bench", 1).expect("verify");
        });
        push_read(
            rows,
            "Tessera (.tsra)",
            &settings,
            "volume",
            "verify",
            "warm",
            s,
            None,
            CLAIM_VERIFY_TSRA,
        );
    }

    for v in h5_variants("volume") {
        let path = dir.join(format!("vol_{}.h5", slug(&v.settings)));
        let (wr, _) = timed(iters, None, || {
            let _ = std::fs::remove_file(&path);
            write_h5_volume(&path, &v, n, raw).expect("h5 write");
        });

        // Correctness: the full read AND the ROI, same as the tessera side.
        {
            let f = hdf5::File::open(&path).map_err(|e| err(&format!("h5 open: {e}")))?;
            let ds = f
                .dataset("volume")
                .map_err(|e| err(&format!("h5 ds: {e}")))?;
            let got: Vec<i16> = ds.read_raw().map_err(|e| err(&format!("h5 read: {e}")))?;
            check(&got, raw, "hdf5 volume full read")?;
            let got_roi = ds
                .read_slice::<i16, _, ndarray::Ix3>(ndarray::s![
                    roi0..roi0 + roi_edge,
                    roi0..roi0 + roi_edge,
                    roi0..roi0 + roi_edge
                ])
                .map_err(|e| err(&format!("h5 roi: {e}")))?;
            let flat: Vec<i16> = got_roi.iter().copied().collect();
            check(&flat, &expected_roi, "hdf5 volume ROI read")?;
        }

        push_size(rows, v.label, &v.settings, "volume", file_len(&path));
        push_write(rows, v.label, &v.settings, "volume", wr);

        for (cache, evict_path) in warm_cold(&path, cold) {
            let (s, res) = timed(iters, evict_path, || {
                let f = hdf5::File::open(&path).expect("open");
                let ds = f.dataset("volume").expect("ds");
                let _: Vec<i16> = ds.read_raw().expect("read");
            });
            push_read(
                rows,
                v.label,
                &v.settings,
                "volume",
                "read full",
                cache,
                s,
                res,
                CLAIM_FULL,
            );

            let e = roi_edge;
            let (s, res) = timed(iters, evict_path, || {
                let f = hdf5::File::open(&path).expect("open");
                let ds = f.dataset("volume").expect("ds");
                let _ = ds
                    .read_slice::<i16, _, ndarray::Ix3>(ndarray::s![
                        roi0..roi0 + e,
                        roi0..roi0 + e,
                        roi0..roi0 + e
                    ])
                    .expect("roi");
            });
            push_read(
                rows,
                v.label,
                &v.settings,
                "volume",
                "read ROI",
                cache,
                s,
                res,
                CLAIM_ROI,
            );
        }

        if v.fletcher32 {
            let (s, _) = timed(iters, None, || {
                let f = hdf5::File::open(&path).expect("open");
                let ds = f.dataset("volume").expect("ds");
                let _: Vec<i16> = ds.read_raw().expect("read");
            });
            push_read(
                rows,
                v.label,
                &v.settings,
                "volume",
                "verify",
                "warm",
                s,
                None,
                CLAIM_VERIFY_H5,
            );
        } else {
            rows.push(Row {
                format: v.label.into(),
                settings: v.settings,
                modality: "volume".into(),
                op: "verify",
                cache: "-",
                resident: None,
                bytes: None,
                stats: None,
                claim: CLAIM_VERIFY_NONE,
            });
        }
    }
    Ok(())
}

const CLAIM_FULL: &str = "baseline full materialise";
const CLAIM_ROI: &str = "cubic chunks -> cheap ROI / orthogonal access";
const CLAIM_COLUMN: &str =
    "columnar projection: decode one column (tessera still READS the block's bytes)";
const CLAIM_ROWS: &str =
    "row window without decoding the rest (parquet reads whole row groups, then slices)";
const CLAIM_SIZE: &str = "smaller on disk than the alternatives";
const CLAIM_WRITE: &str = "the cost of sealing (tessera pays, the others do not)";
const CLAIM_VERIFY_TSRA: &str =
    "tamper-evident: re-derives every digest against the sealed manifest";
const CLAIM_VERIFY_H5: &str =
    "fletcher32: per-chunk CORRUPTION check on read; not keyed, no identity";
const CLAIM_VERIFY_NONE: &str = "no integrity mechanism configured";
/// Parquet's FORMAT has an optional per-page CRC32, but parquet-rs 58's **writer never emits one**
/// (no property enables it), and its reader checks a page CRC only under the crate's `crc` feature
/// AND only when the page header carries one. For files this benchmark writes there is therefore
/// nothing to verify — reported as absent rather than timed, because timing a mechanism that is not
/// in the file would be inventing a number.
const CLAIM_VERIFY_PQ: &str =
    "parquet-rs writes NO page CRC (the format allows one; this writer omits it) -> nothing to check";
const PQ: &str = "Parquet (parquet-rs)";

fn push_size(rows: &mut Vec<Row>, format: &str, settings: &str, modality: &str, b: u64) {
    rows.push(Row {
        format: format.into(),
        settings: settings.into(),
        modality: modality.into(),
        op: "size",
        cache: "-",
        resident: None,
        bytes: Some(b),
        stats: None,
        claim: CLAIM_SIZE,
    });
}

fn push_write(rows: &mut Vec<Row>, format: &str, settings: &str, modality: &str, s: Stats) {
    rows.push(Row {
        format: format.into(),
        settings: settings.into(),
        modality: modality.into(),
        op: "write+seal",
        cache: "warm",
        resident: None,
        bytes: None,
        stats: Some(s),
        claim: CLAIM_WRITE,
    });
}

#[allow(clippy::too_many_arguments)] // one row of the matrix; every field is a distinct column
fn push_read(
    rows: &mut Vec<Row>,
    format: &str,
    settings: &str,
    modality: &str,
    op: &'static str,
    cache: &'static str,
    s: Stats,
    resident: Option<f64>,
    claim: &'static str,
) {
    // A "cold" row whose file the kernel kept is a warm row with a wrong label — say so in the
    // label itself, not only in a footnote (#388 review).
    let cache = match (cache, resident) {
        ("cold", Some(r)) if r > COLD_RESIDENT_OK => "cold?",
        _ => cache,
    };
    rows.push(Row {
        format: format.into(),
        settings: settings.into(),
        modality: modality.into(),
        op,
        cache,
        resident,
        bytes: None,
        stats: Some(s),
        claim,
    });
}

fn warm_cold(path: &Path, cold: bool) -> Vec<(&'static str, Option<&Path>)> {
    if cold {
        vec![("warm", None), ("cold", Some(path))]
    } else {
        vec![("warm", None)]
    }
}

fn slug(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

/// Parquet variants for the head-to-head (#388 stage 2), written with the real `parquet` crate.
struct PqVariant {
    settings: String,
    compression: parquet::basic::Compression,
    dictionary: bool,
    statistics: parquet::file::properties::EnabledStatistics,
    /// BYTE_STREAM_SPLIT on the float columns — Parquet's counterpart to HDF5's shuffle filter.
    byte_stream_split: bool,
    /// Rows per row group. Always [`ROWS_PER_GROUP`] in the benchmark, so Parquet gets the same IO
    /// granularity as the other two formats; a test overrides it to exercise a window that spans
    /// more than one group without writing millions of rows.
    rows_per_group: usize,
}

fn pq_variants() -> Vec<PqVariant> {
    use parquet::basic::{Compression, ZstdLevel};
    use parquet::file::properties::EnabledStatistics;
    vec![
        PqVariant {
            // parquet-58's actual DEFAULT_COMPRESSION is UNCOMPRESSED. Calling snappy "the default"
            // was wrong; both are shown so the crate default and the ecosystem default are distinct.
            settings: format!(
                "default (crate): uncompressed, dict, page stats, {ROWS_PER_GROUP}-row groups"
            ),
            compression: Compression::UNCOMPRESSED,
            dictionary: true,
            statistics: EnabledStatistics::Page,
            byte_stream_split: false,
            rows_per_group: ROWS_PER_GROUP,
        },
        PqVariant {
            settings: format!(
                "pyarrow/Spark default: snappy, dict, page stats, {ROWS_PER_GROUP}-row groups"
            ),
            compression: Compression::SNAPPY,
            dictionary: true,
            statistics: EnabledStatistics::Page,
            byte_stream_split: false,
            rows_per_group: ROWS_PER_GROUP,
        },
        PqVariant {
            // The float analogue of HDF5's shuffle: BYTE_STREAM_SPLIT regroups each float's bytes by
            // significance so the codec sees runs. Dictionary OFF, because a dictionary over
            // continuous floats defeats the split. Omitting this understated Parquet exactly as
            // omitting shuffle understated HDF5 (#487).
            settings: format!(
                "tuned: zstd-4, BYTE_STREAM_SPLIT floats, no dict, {ROWS_PER_GROUP}-row groups"
            ),
            compression: Compression::ZSTD(ZstdLevel::try_new(4).expect("valid zstd level")),
            dictionary: false,
            statistics: EnabledStatistics::Page,
            byte_stream_split: true,
            rows_per_group: ROWS_PER_GROUP,
        },
    ]
}

/// Write one table as Parquet with the given settings. One row group per [`ROWS_PER_GROUP`]-sized
/// slice so the row-range read below can select a row group, mirroring the other two formats' chunk
/// granularity rather than giving Parquet a free layout advantage.
/// Build the Arrow batch ONCE, outside the timed region — cloning the column vectors into Arrow is
/// this benchmark's own marshalling cost, not Parquet's write cost, and timing it would charge
/// Parquet for work the other two formats never do (#503 review).
fn arrow_batch(data: &TableData) -> Result<arrow_array::RecordBatch> {
    use arrow_array::{ArrayRef, Float32Array, RecordBatch, UInt64Array};
    use arrow_schema::{DataType, Field, Schema};
    let mut fields = Vec::new();
    let mut arrays: Vec<ArrayRef> = Vec::new();
    for (name, col) in data {
        match col {
            ColumnData::U64(v) => {
                fields.push(Field::new(name, DataType::UInt64, false));
                arrays.push(std::sync::Arc::new(UInt64Array::from(v.clone())));
            }
            ColumnData::F32(v) => {
                fields.push(Field::new(name, DataType::Float32, false));
                arrays.push(std::sync::Arc::new(Float32Array::from(v.clone())));
            }
            other => return Err(err(&format!("parquet bench: unsupported column {other:?}"))),
        }
    }
    let schema = std::sync::Arc::new(Schema::new(fields));
    RecordBatch::try_new(schema, arrays).map_err(|e| err(&format!("arrow batch: {e}")))
}

/// Write one prebuilt batch as Parquet with the given settings.
fn write_parquet(
    path: &Path,
    v: &PqVariant,
    batch: &arrow_array::RecordBatch,
    data: &TableData,
) -> Result<()> {
    use parquet::arrow::ArrowWriter;
    use parquet::file::properties::WriterProperties;
    let schema = batch.schema();
    let mut pb = WriterProperties::builder()
        .set_compression(v.compression)
        .set_dictionary_enabled(v.dictionary)
        .set_statistics_enabled(v.statistics)
        .set_max_row_group_row_count(Some(v.rows_per_group));
    if v.byte_stream_split {
        // Per-column, because the split only makes sense for the floats; the integer clock keeps
        // whatever the codec picks for it.
        for (name, col) in data {
            if matches!(col, ColumnData::F32(_) | ColumnData::F64(_)) {
                let path = parquet::schema::types::ColumnPath::from(name.as_str());
                pb = pb
                    .set_column_encoding(path.clone(), parquet::basic::Encoding::BYTE_STREAM_SPLIT)
                    .set_column_dictionary_enabled(path, false);
            }
        }
    }
    let props = pb.build();
    let file = std::fs::File::create(path)?;
    let mut w = ArrowWriter::try_new(file, schema, Some(props))
        .map_err(|e| err(&format!("parquet writer: {e}")))?;
    w.write(batch)
        .map_err(|e| err(&format!("parquet write: {e}")))?;
    w.close().map_err(|e| err(&format!("parquet close: {e}")))?;
    Ok(())
}

/// Read Parquet back as `(column name, values)`, optionally projecting columns and/or selecting row
/// groups — the two pushdowns Parquet is actually good at.
fn read_parquet(
    path: &Path,
    project: Option<&[usize]>,
    row_groups: Option<Vec<usize>>,
) -> Result<TableData> {
    use arrow_array::{Array, Float32Array, UInt64Array};
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
    use parquet::arrow::ProjectionMask;

    let file = std::fs::File::open(path)?;
    let mut b = ParquetRecordBatchReaderBuilder::try_new(file)
        .map_err(|e| err(&format!("parquet open: {e}")))?;
    if let Some(idx) = project {
        let mask = ProjectionMask::roots(b.parquet_schema(), idx.iter().copied());
        b = b.with_projection(mask);
    }
    if let Some(rg) = row_groups {
        b = b.with_row_groups(rg);
    }
    let reader = b
        .build()
        .map_err(|e| err(&format!("parquet reader: {e}")))?;

    let mut out: TableData = Vec::new();
    for batch in reader {
        let batch = batch.map_err(|e| err(&format!("parquet batch: {e}")))?;
        for (i, field) in batch.schema().fields().iter().enumerate() {
            let col = batch.column(i);
            let name = field.name().clone();
            let slot = match out.iter_mut().find(|(n, _)| *n == name) {
                Some(s) => s,
                None => {
                    let empty = if col.as_any().is::<UInt64Array>() {
                        ColumnData::U64(Vec::new())
                    } else {
                        ColumnData::F32(Vec::new())
                    };
                    out.push((name.clone(), empty));
                    out.last_mut().expect("just pushed")
                }
            };
            match (&mut slot.1, col.as_any()) {
                (ColumnData::U64(dst), a) => dst.extend(
                    a.downcast_ref::<UInt64Array>()
                        .expect("u64")
                        .values()
                        .iter(),
                ),
                (ColumnData::F32(dst), a) => dst.extend(
                    a.downcast_ref::<Float32Array>()
                        .expect("f32")
                        .values()
                        .iter(),
                ),
                _ => return Err(err("parquet bench: column type drift")),
            }
        }
    }
    Ok(out)
}

/// The table half of the matrix.
fn compare_table(
    dir: &Path,
    label: &str,
    spec: &TableSpec,
    data: &TableData,
    iters: usize,
    cold: bool,
    rows: &mut Vec<Row>,
) -> Result<()> {
    let rows_n = spec.rows as usize;
    let modality = label;
    // Row-range ROI: a contiguous 1% window, the "take a slice of the acquisition" access pattern.
    let roi_len = (rows_n / 100).max(1);
    let roi_start = rows_n / 2;
    let roi_idx: Vec<u64> = (roi_start as u64..(roi_start + roi_len) as u64).collect();
    let expected_rows = expect_rows(data, roi_start, roi_len)?;
    let ColumnData::U64(t) = &data[0].1 else {
        return Err(err("t must be u64"));
    };
    let ColumnData::F32(e0) = &data[1].1 else {
        return Err(err("e0 must be f32"));
    };
    let ColumnData::F32(e1) = &data[2].1 else {
        return Err(err("e1 must be f32"));
    };

    // Tessera's table backend exposes no user-facing codec knob — Vortex picks its cascade per
    // column. Stated rather than invented: a fabricated "tuned" row would imply a lever that is not
    // there. (The ARRAY path does have one, and the volume half exercises it.)
    let settings = "default: Vortex cascade (no user-facing codec knob)".to_string();
    let name = format!("tab_{}", slug(label));
    let (wr, _) = timed(iters, None, || {
        let blk = table::table_block("events", spec, data).expect("encode");
        let _ = seal_tsra(dir, &name, blk).expect("seal");
    });
    let path = dir.join(format!("{name}.tsra"));

    // Correctness gates timing — full, single-column AND row-window are each verified.
    let blob = Reader::open(&path)?.read_block("events")?;
    check(
        &table::decode(spec, &blob)?,
        data,
        "tessera table full read",
    )?;
    check(
        &table::decode_column(spec, &blob, "e0")?,
        &data[1].1,
        "tessera table 1-column read",
    )?;
    check(
        &table::decode_rows(spec, &blob, &roi_idx)?,
        &expected_rows,
        "tessera table row-ROI read",
    )?;

    push_size(
        rows,
        "Tessera (.tsra)",
        &settings,
        modality,
        file_len(&path),
    );
    push_write(rows, "Tessera (.tsra)", &settings, modality, wr);

    for (cache, evict_path) in warm_cold(&path, cold) {
        let (s, res) = timed(iters, evict_path, || {
            let mut r = Reader::open(&path).expect("open");
            let b = r.read_block("events").expect("block");
            let _ = table::decode(spec, &b).expect("decode");
        });
        push_read(
            rows,
            "Tessera (.tsra)",
            &settings,
            modality,
            "read full",
            cache,
            s,
            res,
            CLAIM_FULL,
        );

        let (s, res) = timed(iters, evict_path, || {
            let mut r = Reader::open(&path).expect("open");
            let b = r.read_block("events").expect("block");
            let _ = table::decode_column(spec, &b, "e0").expect("column");
        });
        push_read(
            rows,
            "Tessera (.tsra)",
            &settings,
            modality,
            "read 1 column",
            cache,
            s,
            res,
            CLAIM_COLUMN,
        );

        let (s, res) = timed(iters, evict_path, || {
            let mut r = Reader::open(&path).expect("open");
            let b = r.read_block("events").expect("block");
            let _ = table::decode_rows(spec, &b, &roi_idx).expect("rows");
        });
        push_read(
            rows,
            "Tessera (.tsra)",
            &settings,
            modality,
            "read row ROI",
            cache,
            s,
            res,
            CLAIM_ROWS,
        );
    }

    let (s, _) = timed(iters, None, || {
        let _ = tessera_io::verify_payloads_parallel(&path, "bench", 1).expect("verify");
    });
    push_read(
        rows,
        "Tessera (.tsra)",
        &settings,
        modality,
        "verify",
        "warm",
        s,
        None,
        CLAIM_VERIFY_TSRA,
    );

    // ---- HDF5 side: one dataset per column, which is how a columnar table is expressed in HDF5.
    for v in h5_variants("table") {
        let path = dir.join(format!("tab_{}_{}.h5", slug(label), slug(&v.settings)));
        let (wr, _) = timed(iters, None, || {
            let _ = std::fs::remove_file(&path);
            write_h5_table(&path, &v, t, e0, e1).expect("h5 write");
        });

        // Correctness: full (ALL three columns, not just e0), the projected column, and the window.
        {
            let f = hdf5::File::open(&path).map_err(|e| err(&format!("h5 open: {e}")))?;
            let got_t: Vec<u64> = f
                .dataset("t")
                .and_then(|d| d.read_raw())
                .map_err(|e| err(&format!("h5 read t: {e}")))?;
            let got_e0: Vec<f32> = f
                .dataset("e0")
                .and_then(|d| d.read_raw())
                .map_err(|e| err(&format!("h5 read e0: {e}")))?;
            let got_e1: Vec<f32> = f
                .dataset("e1")
                .and_then(|d| d.read_raw())
                .map_err(|e| err(&format!("h5 read e1: {e}")))?;
            check(&got_t, t, "hdf5 table full read (t)")?;
            check(&got_e0, e0, "hdf5 table full read (e0)")?;
            check(&got_e1, e1, "hdf5 table full read (e1)")?;
            check(&got_e0, e0, "hdf5 table 1-column read")?;

            let win = |name: &str| -> Result<Vec<f32>> {
                let d = f
                    .dataset(name)
                    .map_err(|e| err(&format!("h5 ds {name}: {e}")))?;
                let a = d
                    .read_slice_1d::<f32, _>(ndarray::s![roi_start..roi_start + roi_len])
                    .map_err(|e| err(&format!("h5 window {name}: {e}")))?;
                Ok(a.to_vec())
            };
            let wt = f
                .dataset("t")
                .and_then(|d| {
                    d.read_slice_1d::<u64, _>(ndarray::s![roi_start..roi_start + roi_len])
                })
                .map_err(|e| err(&format!("h5 window t: {e}")))?;
            check(
                &wt.to_vec(),
                &t[roi_start..roi_start + roi_len].to_vec(),
                "hdf5 row-ROI (t)",
            )?;
            check(
                &win("e0")?,
                &e0[roi_start..roi_start + roi_len].to_vec(),
                "hdf5 row-ROI (e0)",
            )?;
            check(
                &win("e1")?,
                &e1[roi_start..roi_start + roi_len].to_vec(),
                "hdf5 row-ROI (e1)",
            )?;
        }

        push_size(rows, v.label, &v.settings, modality, file_len(&path));
        push_write(rows, v.label, &v.settings, modality, wr);

        for (cache, evict_path) in warm_cold(&path, cold) {
            let (s, res) = timed(iters, evict_path, || {
                let f = hdf5::File::open(&path).expect("open");
                let _: Vec<u64> = f.dataset("t").and_then(|d| d.read_raw()).expect("read t");
                let _: Vec<f32> = f.dataset("e0").and_then(|d| d.read_raw()).expect("read e0");
                let _: Vec<f32> = f.dataset("e1").and_then(|d| d.read_raw()).expect("read e1");
            });
            push_read(
                rows,
                v.label,
                &v.settings,
                modality,
                "read full",
                cache,
                s,
                res,
                CLAIM_FULL,
            );

            let (s, res) = timed(iters, evict_path, || {
                let f = hdf5::File::open(&path).expect("open");
                let ds = f.dataset("e0").expect("ds");
                let _: Vec<f32> = ds.read_raw().expect("read");
            });
            push_read(
                rows,
                v.label,
                &v.settings,
                modality,
                "read 1 column",
                cache,
                s,
                res,
                CLAIM_COLUMN,
            );

            // ALL THREE columns, matching what tessera's `decode_rows` returns — the first draft
            // timed one column here against tessera's three, which is not a comparison (#388 review).
            let (s, res) = timed(iters, evict_path, || {
                let f = hdf5::File::open(&path).expect("open");
                let sel = ndarray::s![roi_start..roi_start + roi_len];
                let _ = f
                    .dataset("t")
                    .and_then(|d| d.read_slice_1d::<u64, _>(sel))
                    .expect("window t");
                let _ = f
                    .dataset("e0")
                    .and_then(|d| d.read_slice_1d::<f32, _>(sel))
                    .expect("window e0");
                let _ = f
                    .dataset("e1")
                    .and_then(|d| d.read_slice_1d::<f32, _>(sel))
                    .expect("window e1");
            });
            push_read(
                rows,
                v.label,
                &v.settings,
                modality,
                "read row ROI",
                cache,
                s,
                res,
                CLAIM_ROWS,
            );
        }

        if v.fletcher32 {
            let (s, _) = timed(iters, None, || {
                let f = hdf5::File::open(&path).expect("open");
                let _: Vec<u64> = f.dataset("t").and_then(|d| d.read_raw()).expect("read t");
                let _: Vec<f32> = f.dataset("e0").and_then(|d| d.read_raw()).expect("read e0");
                let _: Vec<f32> = f.dataset("e1").and_then(|d| d.read_raw()).expect("read e1");
            });
            push_read(
                rows,
                v.label,
                &v.settings,
                modality,
                "verify",
                "warm",
                s,
                None,
                CLAIM_VERIFY_H5,
            );
        } else {
            rows.push(Row {
                format: v.label.into(),
                settings: v.settings,
                modality: modality.into(),
                op: "verify",
                cache: "-",
                resident: None,
                bytes: None,
                stats: None,
                claim: CLAIM_VERIFY_NONE,
            });
        }
    }

    // ---- Parquet (#388 stage 2), via the real `parquet` crate.
    let col_names: Vec<String> = data.iter().map(|(n, _)| n.clone()).collect();
    let e0_idx = col_names.iter().position(|n| n == "e0").unwrap_or(1);
    let (roi_groups, win_off) = window_row_groups(roi_start, roi_len, ROWS_PER_GROUP);

    let batch = arrow_batch(data)?;
    for v in pq_variants() {
        let path = dir.join(format!("tab_{}_{}.parquet", slug(label), slug(&v.settings)));
        let (wr, _) = timed(iters, None, || {
            let _ = std::fs::remove_file(&path);
            write_parquet(&path, &v, &batch, data).expect("parquet write");
        });

        // Correctness gates timing, same rule as the other two formats: full, projected AND the
        // row-group window are each checked against the source before any number counts.
        check(&read_parquet(&path, None, None)?, data, "parquet full read")?;
        let proj = read_parquet(&path, Some(&[e0_idx]), None)?;
        check(&proj.len(), &1usize, "parquet projection column count")?;
        check(&proj[0].1, &data[e0_idx].1, "parquet 1-column read")?;
        // Checked against the EXACT window, not against whatever the selected groups happened to
        // contain — comparing to the group's own slice is what let the short read pass before.
        let win = slice_rows(
            &read_parquet(&path, None, Some(roi_groups.clone()))?,
            win_off,
            roi_len,
        )?;
        check(&win, &expected_rows, "parquet row window")?;

        push_size(rows, PQ, &v.settings, modality, file_len(&path));
        push_write(rows, PQ, &v.settings, modality, wr);

        for (cache, evict_path) in warm_cold(&path, cold) {
            let (s, res) = timed(iters, evict_path, || {
                let _ = read_parquet(&path, None, None).expect("full");
            });
            push_read(
                rows,
                PQ,
                &v.settings,
                modality,
                "read full",
                cache,
                s,
                res,
                CLAIM_FULL,
            );

            let (s, res) = timed(iters, evict_path, || {
                let _ = read_parquet(&path, Some(&[e0_idx]), None).expect("projection");
            });
            push_read(
                rows,
                PQ,
                &v.settings,
                modality,
                "read 1 column",
                cache,
                s,
                res,
                CLAIM_COLUMN,
            );

            let (s, res) = timed(iters, evict_path, || {
                let got = read_parquet(&path, None, Some(roi_groups.clone())).expect("row groups");
                let _ = slice_rows(&got, win_off, roi_len).expect("slice to the exact window");
            });
            push_read(
                rows,
                PQ,
                &v.settings,
                modality,
                "read row ROI",
                cache,
                s,
                res,
                CLAIM_ROWS,
            );
        }

        rows.push(Row {
            format: PQ.into(),
            settings: v.settings,
            modality: modality.into(),
            op: "verify",
            cache: "-",
            resident: None,
            bytes: None,
            stats: None,
            claim: CLAIM_VERIFY_PQ,
        });
    }

    Ok(())
}

fn write_h5_table(path: &Path, v: &H5Variant, t: &[u64], e0: &[f32], e1: &[f32]) -> Result<()> {
    let f = hdf5::File::create(path).map_err(|e| err(&format!("h5 create: {e}")))?;
    // One dataset per column — the columnar shape, so the projection row is a fair comparison.
    macro_rules! col {
        ($name:expr, $ty:ty, $vals:expr) => {{
            let mut b = f.new_dataset::<$ty>().shape([$vals.len()]);
            if v.chunked {
                // 1-D chunking for tables; the cubic geometry only applies to the volume.
                b = b.chunk([TABLE_CHUNK_ROWS.min($vals.len())]);
            }
            if v.shuffle {
                b = b.shuffle();
            }
            if let Some(l) = v.deflate {
                b = b.deflate(l);
            }
            if v.fletcher32 {
                b = b.fletcher32();
            }
            b.create($name)
                .map_err(|e| err(&format!("h5 dataset {}: {e}", $name)))?
                .write(ndarray::ArrayView1::from($vals))
                .map_err(|e| err(&format!("h5 write {}: {e}", $name)))?;
        }};
    }
    col!("t", u64, t);
    col!("e0", f32, e0);
    col!("e1", f32, e1);
    Ok(())
}

/// The machine + toolchain the numbers came from. Timings are meaningless without it, and a reader
/// comparing two runs needs to know whether they are even the same box (#388 review).
struct MachineInfo {
    cpu: String,
    cores: usize,
    ram: String,
    fs: String,
    kernel: String,
    tessera: String,
    libhdf5: String,
}

fn first_field(path: &str, key: &str) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    text.lines()
        .find(|l| l.starts_with(key))
        .and_then(|l| l.split_once(':').map(|(_, v)| v.trim().to_string()))
}

/// Filesystem type of the directory the benchmark actually wrote to — tmpfs vs ext4 vs a network
/// mount changes every I/O number here, so it is reported rather than assumed.
fn fs_type(dir: &Path) -> String {
    // SAFETY: `statfs` writes into a zeroed, correctly-sized struct; the path is NUL-terminated.
    unsafe {
        let mut buf: libc::statfs = std::mem::zeroed();
        let Ok(c) = std::ffi::CString::new(dir.as_os_str().as_encoded_bytes()) else {
            return "unknown".into();
        };
        if libc::statfs(c.as_ptr(), &mut buf) != 0 {
            return "unknown".into();
        }
        // The common ones by magic; anything else is reported as its raw magic rather than guessed.
        match buf.f_type {
            0x0000_1021 | 0x0000_9123 => "btrfs".into(),
            0x0000_EF53 => "ext2/3/4".into(),
            0x0000_6969 => "nfs".into(),
            0x0102_1994 => "tmpfs".into(),
            0x5846_5342 => "xfs".into(),
            0x2011_BAB0 => "exfat".into(),
            other => format!("0x{other:x}"),
        }
    }
}

fn machine_info(dir: &Path) -> MachineInfo {
    let ram = first_field("/proc/meminfo", "MemTotal")
        .and_then(|v| {
            v.split_whitespace()
                .next()
                .and_then(|k| k.parse::<u64>().ok())
        })
        .map(|kb| human(kb * 1024))
        .unwrap_or_else(|| "unknown".into());
    MachineInfo {
        cpu: first_field("/proc/cpuinfo", "model name").unwrap_or_else(|| "unknown".into()),
        cores: std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(0),
        ram,
        fs: fs_type(dir),
        kernel: kernel(),
        tessera: tessera_core::manifest::TESSERA_VERSION.to_string(),
        libhdf5: {
            let (a, b, c) = hdf5::library_version();
            format!("{a}.{b}.{c}")
        },
    }
}

/// Kernel string for the provenance line — cold-cache eviction behaviour is kernel-dependent, so the
/// report says which kernel produced the numbers rather than implying they transfer.
fn kernel() -> String {
    std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown".into())
}

/// Plain-language verdict on whether the cold rows really ran cold, from the measured residency.
fn cold_verdict() -> String {
    match COLD_RESIDENT_PERMILLE.load(std::sync::atomic::Ordering::Relaxed) {
        u32::MAX => "residency unmeasured".to_string(),
        p if p <= 10 => format!(
            "eviction effective (worst residency {:.1}%)",
            p as f64 / 10.0
        ),
        p => format!(
            "EVICTION INEFFECTIVE (worst residency {:.1}%) — treat the cold rows as warm",
            p as f64 / 10.0
        ),
    }
}

/// Run the comparison and print the report.
pub fn run(opts: CompareOpts) -> Result<()> {
    let dir = tempfile::tempdir().map_err(tessera_core::Error::from)?;
    let mut rows: Vec<Row> = Vec::new();

    let want_vol = opts.dataset == "both" || opts.dataset == "volume";
    let want_tab = opts.dataset == "both" || opts.dataset == "table";
    if !want_vol && !want_tab {
        return Err(err(&format!(
            "bench compare: --dataset must be volume|table|both, got {:?}",
            opts.dataset
        )));
    }
    if want_vol {
        compare_volume(dir.path(), opts.vol_n, opts.iters, opts.cold, &mut rows)?;
    }
    if want_tab {
        // BOTH synthetic fixtures, always, and always labelled. The periodic one is adversarial
        // for tessera and stays; the continuous one is what a real acquisition looks like. One
        // without the other is a chosen answer (#493).
        let (sp, dp) = make_table_periodic(opts.rows);
        compare_table(
            dir.path(),
            "table (periodic)",
            &sp,
            &dp,
            opts.iters,
            opts.cold,
            &mut rows,
        )?;
        let (sc, dc) = make_table_continuous(opts.rows);
        compare_table(
            dir.path(),
            "table (continuous)",
            &sc,
            &dc,
            opts.iters,
            opts.cold,
            &mut rows,
        )?;
    }

    let mi = machine_info(dir.path());
    match opts.format.as_str() {
        "json" => print_json(&opts, &rows, &mi),
        "table" => print_table(&opts, &rows, &mi),
        other => {
            return Err(err(&format!(
                "bench compare: --format must be table|json, got {other:?}"
            )))
        }
    }
    Ok(())
}

fn raw_bytes_note(opts: &CompareOpts) -> String {
    let mut parts = Vec::new();
    if opts.dataset == "both" || opts.dataset == "volume" {
        parts.push(format!(
            "volume {}^3 int16 = {} raw",
            opts.vol_n,
            human((opts.vol_n as u64).pow(3) * 2)
        ));
    }
    if opts.dataset == "both" || opts.dataset == "table" {
        parts.push(format!(
            "table {} rows x (u8+2xf4) = {} raw, TWO fixtures (periodic + continuous)",
            opts.rows,
            human(opts.rows as u64 * (8 + 4 + 4))
        ));
    }
    parts.join(" · ")
}

fn human(b: u64) -> String {
    const U: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{b} B")
    } else {
        format!("{v:.1} {}", U[i])
    }
}

fn print_table(opts: &CompareOpts, rows: &[Row], mi: &MachineInfo) {
    out!("tessera bench compare — .tsra vs HDF5 vs Parquet (#388)");
    out!("  data     {}", raw_bytes_note(opts));
    out!(
        "  machine  {} x{} · {} RAM · bench dir on {} · kernel {}",
        mi.cpu,
        mi.cores,
        mi.ram,
        mi.fs,
        mi.kernel
    );
    out!("  versions tessera {} · libhdf5 {}", mi.tessera, mi.libhdf5);
    out!(
        "  method   median of {} runs, spread = [min..max]; EVERY timed read is verified against",
        opts.iters
    );
    out!("           the source data first (full, projected and ROI, per format and setting)");
    out!("  warm-up  none beyond the correctness pass: the verification read precedes the warm");
    out!(
        "           timings, so warm rows start with the file in page cache; cold rows evict first"
    );
    out!(
        "  cache    {}",
        if opts.cold {
            format!(
                "warm AND cold; cold = fsync + posix_fadvise(DONTNEED), BEST-EFFORT · kernel {} · {}",
                kernel(),
                cold_verdict()
            )
        } else {
            "warm only (pass --cold to add best-effort cold-cache rows)".to_string()
        }
    );
    out!("  note     three formats: .tsra, HDF5 and Parquet (#388)");
    out!(
        "  fixtures table (periodic)   = the original #388 fixture: f32 columns of period 7 and 5."
    );
    out!("           ADVERSARIAL for value-distribution codecs — deflate's LZ77 locks onto the");
    out!(
        "           repeating byte block; Pco/dict/bitpacking model values, not repetition (#493)."
    );
    out!("           table (continuous) = shaped after REAL DUPLET listmode: a coarse ms clock");
    out!(
        "           (~99.6% zero deltas) + continuous energies. Both are reported, always: showing"
    );
    out!("           only one of them would be choosing the answer.");
    out!("  CAVEAT   the VOLUME is a smooth gradient (verbatim from the #143 harness, for");
    out!("           comparability), and the PERIODIC table is adversarial by construction. Both");
    out!("           are far more compressible than a real acquisition, so read every size row as");
    out!("           a RATIO BETWEEN FORMATS on identical input, not as a compression ratio to");
    out!(
        "           expect clinically. The CONTINUOUS table is the one shaped after real listmode."
    );
    out!();
    out!(
        "{:<18} {:<56} {:<7} {:<14} {:<5} {:>10} {:>26}",
        "format",
        "settings",
        "data",
        "op",
        "cache",
        "size",
        "median [min..max]"
    );
    for r in rows {
        let size = r.bytes.map(human).unwrap_or_else(|| "-".into());
        let t = match &r.stats {
            Some(s) => format!("{:.4}s [{:.4}..{:.4}]", s.median_s, s.min_s, s.max_s),
            None => "-".into(),
        };
        out!(
            "{:<18} {:<56} {:<7} {:<14} {:<5} {:>10} {:>26}",
            r.format,
            r.settings,
            r.modality,
            r.op,
            r.cache,
            size,
            t
        );
    }
    out!();
    out!("What each row supports:");
    let mut seen: Vec<&str> = Vec::new();
    for r in rows {
        if !seen.contains(&r.op) {
            seen.push(r.op);
            out!("  {:<14} {}", r.op, r.claim);
        }
    }
    out!();
    out!("Integrity is not one thing — see the module docs for the long form:");
    out!("  fletcher32 detects CORRUPTION on the chunks you read. It is unkeyed and covers no metadata.");
    out!("  tessera verify re-derives every block digest against the sealed manifest, whose hash also");
    out!(
        "  covers metadata + provenance, and which a signature can bind to a signer. Both catch a"
    );
    out!("  flipped bit; only one answers 'is this the artifact that was sealed, and by whom'.");
}

fn print_json(opts: &CompareOpts, rows: &[Row], mi: &MachineInfo) {
    let items: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            serde_json::json!({
                "format": r.format,
                "settings": r.settings,
                "dataset": r.modality,
                "op": r.op,
                // "cold" only survives when eviction actually worked for THIS file; otherwise the
                // row is "cold?" and the residency says why.
                "cache": r.cache,
                "resident_after_evict": r.resident,
                "bytes": r.bytes,
                "median_s": r.stats.as_ref().map(|s| s.median_s),
                "min_s": r.stats.as_ref().map(|s| s.min_s),
                "max_s": r.stats.as_ref().map(|s| s.max_s),
                "n": r.stats.as_ref().map(|s| s.n),
                "claim": r.claim,
            })
        })
        .collect();
    let doc = serde_json::json!({
        "benchmark": "tessera bench compare",
        "issue": 388,
        "iters": opts.iters,
        "statistic": "median with [min..max] spread",
        "cold_cache": opts.cold,
        "cold_method": if opts.cold { "fsync + posix_fadvise(POSIX_FADV_DONTNEED), best-effort" } else { "n/a" },
        "cold_eviction": if opts.cold { cold_verdict() } else { "n/a".to_string() },
        "machine": {
            "cpu": mi.cpu,
            "cores": mi.cores,
            "ram": mi.ram,
            "bench_dir_fs": mi.fs,
            "kernel": mi.kernel,
        },
        "versions": { "tessera": mi.tessera, "libhdf5": mi.libhdf5 },
        "verification": "every timed read (full, projected, ROI) is compared against the source data, per format and setting, before its timings count",
        // null rather than the configured-but-unused value, so a consumer cannot read a size for a
        // dataset this run never touched.
        "volume_n": (opts.dataset != "table").then_some(opts.vol_n),
        "table_rows": (opts.dataset != "volume").then_some(opts.rows),
        // Must say the same thing as the printed CAVEAT: json is the CI format, and a report that
        // contradicts itself between its two renderings is worse than one that is merely terse.
        "caveat": "the VOLUME is a smooth gradient (verbatim from the #143 harness) and the PERIODIC table is adversarial by construction; both are far more compressible than a real acquisition, so size rows are a ratio BETWEEN formats on identical input, not a compression ratio to expect clinically. The CONTINUOUS table is the one shaped after real listmode",
        "parquet": "included (parquet-rs); its writer emits no page CRC, so the integrity row is absent by fact, not omission",
        "rows": items,
    });
    out!("{}", serde_json::to_string_pretty(&doc).expect("json"));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The statistic the whole report rests on. A mean would let one contended run move the headline
    /// number; the median plus an explicit [min..max] says both "typical" and "how noisy".
    #[test]
    fn stats_report_median_and_spread() {
        // Odd N -> the middle element, and an outlier must NOT drag it.
        let s = Stats::of(vec![0.10, 0.11, 0.12, 0.13, 9.99]);
        assert!((s.median_s - 0.12).abs() < 1e-9, "median: {}", s.median_s);
        assert!((s.min_s - 0.10).abs() < 1e-9);
        assert!(
            (s.max_s - 9.99).abs() < 1e-9,
            "the outlier must still be VISIBLE in the spread"
        );
        assert_eq!(s.n, 5);

        // Even N -> the mean of the two middles.
        let s = Stats::of(vec![0.2, 0.4, 0.1, 0.3]);
        assert!((s.median_s - 0.25).abs() < 1e-9, "median: {}", s.median_s);

        // Input order must not matter.
        let a = Stats::of(vec![3.0, 1.0, 2.0]);
        let b = Stats::of(vec![1.0, 2.0, 3.0]);
        assert!((a.median_s - b.median_s).abs() < 1e-9);
    }

    /// #388 fairness: the settings line must describe what was ACTUALLY used. The volume gets
    /// tessera's cubic geometry so the ROI row compares layouts; a 1-D table cannot, and printing
    /// "64^3" on a table row would misrepresent the comparison.
    #[test]
    fn hdf5_settings_describe_the_real_chunk_geometry() {
        let vol: Vec<String> = h5_variants("volume")
            .into_iter()
            .map(|v| v.settings)
            .collect();
        let tab: Vec<String> = h5_variants("table")
            .into_iter()
            .map(|v| v.settings)
            .collect();

        assert!(vol
            .iter()
            .any(|s| s.contains("64^3") && s.contains("tessera's")));
        assert!(
            !tab.iter().any(|s| s.contains("^3")),
            "a 1-D table must not claim cubic chunks: {tab:?}"
        );
        assert!(tab.iter().any(|s| s.contains("65536 rows")));

        // shuffle+deflate is the standard HDF5 tuning. Omitting shuffle weakened the baseline so
        // much that the table size result INVERTED: 5.9 MiB without it vs 155.3 KiB with, against
        // tessera's 959.4 KiB. A tuned variant without shuffle is not a tuned variant (#388 review).
        for set in [&vol, &tab] {
            for tuned in set.iter().filter(|s| s.starts_with("tuned:")) {
                assert!(
                    tuned.contains("shuffle+gzip"),
                    "a tuned HDF5 variant must pair shuffle with deflate: {tuned}"
                );
            }
        }

        // Both modalities must offer a default AND tuned variants, or the comparison is a straw man.
        for set in [&vol, &tab] {
            assert!(set.iter().any(|s| s.starts_with("default:")), "{set:?}");
            assert!(
                set.iter().filter(|s| s.starts_with("tuned:")).count() >= 2,
                "{set:?}"
            );
            assert!(set.iter().any(|s| s.contains("fletcher32")), "{set:?}");
        }
    }

    /// The ROI must be ONE WHOLE, CHUNK-ALIGNED cubic chunk — the claim the row makes. Centring
    /// without aligning put it across 8 chunks (roi0 = 96 for n = 256), so the row measured a
    /// misaligned read while the prose said "one cubic chunk" (#388 review).
    #[test]
    fn volume_roi_is_one_aligned_chunk() {
        for n in [64usize, 128, 256, 512] {
            let edge = CHUNK.min(n);
            let roi0 = ((n - edge) / 2 / edge) * edge;
            assert_eq!(
                roi0 % edge,
                0,
                "n={n}: ROI start {roi0} is not chunk-aligned"
            );
            assert!(roi0 + edge <= n, "n={n}: ROI runs past the volume");
        }
        // And the expectation helper agrees with a hand-computed box.
        let n = 4;
        let raw: Vec<i16> = (0..(n * n * n) as i16).collect();
        let got = expect_roi(&raw, n, 2, 2);
        // z,y in {2,3}, x in 2..4 -> rows starting at 2*16+2*4+2 = 42, 46, 58, 62
        assert_eq!(got, vec![42, 43, 46, 47, 58, 59, 62, 63]);
    }

    /// The window math must cover EVERY group a window touches, not just the one it starts in.
    #[test]
    fn window_row_groups_covers_every_group_the_window_spans() {
        // Straddles a boundary: rows 50..80 over 64-row groups touch groups 0 and 1.
        let (groups, off) = window_row_groups(50, 30, 64);
        assert_eq!(groups, vec![0, 1], "50..80 crosses the 64 boundary");
        assert_eq!(off, 50);

        // Fully inside one group.
        let (groups, off) = window_row_groups(10, 20, 64);
        assert_eq!(groups, vec![0]);
        assert_eq!(off, 10);

        // Exactly aligned to a group start, exactly one group long.
        let (groups, off) = window_row_groups(128, 64, 64);
        assert_eq!(groups, vec![2]);
        assert_eq!(off, 0);

        // Spans three groups: 60..160 ends in group 2.
        let (groups, off) = window_row_groups(60, 100, 64);
        assert_eq!(groups, vec![0, 1, 2]);
        assert_eq!(off, 60);

        // Spans four: 60..200 ends in group 3. One row past a boundary is still a whole extra group.
        let (groups, _) = window_row_groups(60, 140, 64);
        assert_eq!(groups, vec![0, 1, 2, 3]);

        // The real regression: the benchmark's own window at the benchmark's own group size.
        let (groups, off) = window_row_groups(2_000_000, 40_000, ROWS_PER_GROUP);
        assert_eq!(
            groups,
            vec![30, 31],
            "the shipped window straddles two groups"
        );
        assert_eq!(off, 2_000_000 - 30 * ROWS_PER_GROUP);
    }

    /// End-to-end: a Parquet row window that SPANS TWO ROW GROUPS must come back byte-exact.
    ///
    /// The unit test above pins the arithmetic; this one pins the whole chain — write with small
    /// row groups, select the groups, read, slice, compare against the exact source window. It is
    /// the test that would have failed on the shipped code, where only the first group was read.
    #[test]
    fn parquet_row_window_spanning_two_row_groups_round_trips_exactly() {
        use parquet::basic::Compression;
        use parquet::file::properties::EnabledStatistics;

        const ROWS: usize = 200;
        const GROUP: usize = 64; // -> groups [0..64), [64..128), [128..192), [192..200)

        let mut rng = Rng(0xDEAD_BEEF_CAFE_F00D);
        let data: TableData = vec![
            (
                "t".into(),
                ColumnData::U64((0..ROWS as u64).map(|k| k * 7 + 1).collect()),
            ),
            (
                "e0".into(),
                ColumnData::F32((0..ROWS).map(|_| 511.0 + 25.0 * rng.bell()).collect()),
            ),
            (
                "e1".into(),
                ColumnData::F32((0..ROWS).map(|_| 511.0 + 25.0 * rng.bell()).collect()),
            ),
        ];

        let v = PqVariant {
            settings: "test: uncompressed, small groups".into(),
            compression: Compression::UNCOMPRESSED,
            dictionary: false,
            statistics: EnabledStatistics::Page,
            byte_stream_split: false,
            rows_per_group: GROUP,
        };

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("two_groups.parquet");
        let batch = arrow_batch(&data).expect("arrow batch");
        write_parquet(&path, &v, &batch, &data).expect("write");

        // [100, 150) starts in group 1 and ends in group 2 — reading only group 1 yields 28 of the
        // 50 rows, which is exactly the bug.
        let (groups, off) = window_row_groups(100, 50, GROUP);
        assert_eq!(groups, vec![1, 2], "the window must span two groups");

        let got = read_parquet(&path, None, Some(groups)).expect("read row groups");
        let win = slice_rows(&got, off, 50).expect("slice to the exact window");
        let want = expect_rows(&data, 100, 50).expect("source window");
        assert_eq!(win, want, "row window across two groups must be exact");

        // And the count is the full window, not the first group's share of it.
        let ColumnData::U64(t) = &win[0].1 else {
            panic!("t must be u64");
        };
        assert_eq!(t.len(), 50);
        assert_eq!(t[0], 100 * 7 + 1);
        assert_eq!(t[49], 149 * 7 + 1);
    }

    /// The continuous fixture's clock must have POISSON run lengths, not a fixed stride.
    ///
    /// It shipped as `k / 250` — an exact period, which is the PERIODIC fixture's defining property
    /// smuggled into the fixture whose entire purpose is not to have it (#497 review). A fixed
    /// stride makes every run length identical, so that is exactly what this asserts against.
    #[test]
    fn continuous_clock_runs_are_poisson_not_a_fixed_stride() {
        let (_, data) = make_table_continuous(200_000);
        let ColumnData::U64(t) = &data[0].1 else {
            panic!("t must be u64");
        };

        // Run lengths: how many consecutive events share each clock value.
        let mut runs = Vec::new();
        let mut cur = 1usize;
        for w in t.windows(2) {
            if w[0] == w[1] {
                cur += 1;
            } else {
                runs.push(cur);
                cur = 1;
            }
        }
        assert!(runs.len() > 100, "too few runs to judge: {}", runs.len());

        // A fixed stride yields ONE distinct run length. Exponential gaps yield many.
        let distinct: std::collections::BTreeSet<_> = runs.iter().copied().collect();
        assert!(
            distinct.len() > 50,
            "run lengths look like a fixed stride: {} distinct values",
            distinct.len()
        );

        // Mean run length tracks MS_RUN_EVENTS, so the column keeps the real data's run STRUCTURE
        // (~99.6% zero deltas) while losing its periodicity.
        let mean = runs.iter().sum::<usize>() as f64 / runs.len() as f64;
        assert!(
            (150.0..400.0).contains(&mean),
            "mean run length {mean} is nowhere near MS_RUN_EVENTS ({MS_RUN_EVENTS})"
        );

        // The clock still advances monotonically and is still coarse.
        assert!(
            t.windows(2).all(|w| w[1] >= w[0]),
            "clock must be monotonic"
        );
        let zero_deltas = t.windows(2).filter(|w| w[0] == w[1]).count();
        let frac = zero_deltas as f64 / (t.len() - 1) as f64;
        assert!(frac > 0.99, "expected ~99.6% zero deltas, got {frac}");
    }

    /// #497: every table row must carry its FIXTURE label. Eight `push_*` call sites once passed a
    /// literal "table", so the continuous fixture's read rows were emitted indistinguishable from
    /// the periodic ones — the two fixtures exist precisely to be told apart, so a mislabelled row
    /// is worse than a missing one.
    #[test]
    fn table_rows_carry_their_fixture_label() {
        let dir = tempfile::tempdir().unwrap();
        let mut rows: Vec<Row> = Vec::new();
        let (spec, data) = make_table_continuous(2_000);
        compare_table(
            dir.path(),
            "table (continuous)",
            &spec,
            &data,
            1,
            false,
            &mut rows,
        )
        .unwrap();
        assert!(!rows.is_empty());
        for r in &rows {
            assert_eq!(
                r.modality, "table (continuous)",
                "row {:?}/{:?} lost its fixture label",
                r.format, r.op
            );
        }
        // ...and every access pattern is present, so a dropped row cannot pass as a labelled one.
        for op in [
            "size",
            "write+seal",
            "read full",
            "read 1 column",
            "read row ROI",
        ] {
            assert!(rows.iter().any(|r| r.op == op), "missing op {op}");
        }
    }

    /// #388 stage 2 / #503 review: Parquet must be shown at the CRATE's real default, at the
    /// ecosystem default, and tuned — with the standard float tuning actually applied.
    ///
    /// Two things this pins, both of which were wrong first time round. parquet-58's
    /// `DEFAULT_COMPRESSION` is UNCOMPRESSED, so labelling snappy "the default" was false. And the
    /// tuned variant must carry BYTE_STREAM_SPLIT (Parquet's counterpart to HDF5's shuffle) with the
    /// dictionary off: without it the tuned file was 39.3 MiB instead of 23.4 MiB on the continuous
    /// fixture, which flattered Tessera exactly as omitting shuffle had flattered it against HDF5.
    #[test]
    fn parquet_variants_cover_crate_default_ecosystem_default_and_a_real_tuning() {
        let vs = pq_variants();
        let labels: Vec<&str> = vs.iter().map(|p| p.settings.as_str()).collect();

        assert!(
            labels
                .iter()
                .any(|s| s.contains("default (crate)") && s.contains("uncompressed")),
            "the crate default is UNCOMPRESSED and must be shown as such: {labels:?}"
        );
        assert!(
            labels
                .iter()
                .any(|s| s.contains("pyarrow/Spark default") && s.contains("snappy")),
            "snappy is the ecosystem default, not the crate's: {labels:?}"
        );

        let tuned = vs
            .iter()
            .find(|v| v.settings.starts_with("tuned:"))
            .expect("a tuned variant must exist");
        assert!(
            tuned.byte_stream_split,
            "the tuned variant must apply BYTE_STREAM_SPLIT to the floats: {}",
            tuned.settings
        );
        assert!(
            !tuned.dictionary,
            "BYTE_STREAM_SPLIT wants the dictionary OFF: {}",
            tuned.settings
        );
        assert!(
            tuned.settings.contains("BYTE_STREAM_SPLIT"),
            "the settings line must say so: {}",
            tuned.settings
        );
        // Every label states the row-group size, since it sets the row-window granularity.
        for l in &labels {
            assert!(
                l.contains("65536-row groups"),
                "settings must print the row-group size: {l}"
            );
        }

        // The integrity claim must not imply a mechanism the written files do not carry.
        assert!(
            CLAIM_VERIFY_PQ.contains("NO page CRC") && CLAIM_VERIFY_PQ.contains("nothing to check"),
            "{CLAIM_VERIFY_PQ}"
        );
        // ...and the row-window claim must say Parquet reads whole groups and slices.
        assert!(CLAIM_ROWS.contains("row groups"), "{CLAIM_ROWS}");
    }

    #[test]
    fn human_bytes_are_readable() {
        assert_eq!(human(512), "512 B");
        assert_eq!(human(1024), "1.0 KiB");
        assert_eq!(human(1024 * 1024 * 3 / 2), "1.5 MiB");
    }

    /// The cold verdict must say plainly when eviction did NOT work — a cold-labelled row the kernel
    /// served from RAM is worse than no row, so the wording has to be unmissable.
    #[test]
    fn cold_verdict_calls_out_ineffective_eviction() {
        COLD_RESIDENT_PERMILLE.store(0, std::sync::atomic::Ordering::Relaxed);
        assert!(cold_verdict().contains("effective"));
        COLD_RESIDENT_PERMILLE.store(900, std::sync::atomic::Ordering::Relaxed);
        let v = cold_verdict();
        assert!(
            v.contains("INEFFECTIVE") && v.contains("treat the cold rows as warm"),
            "{v}"
        );
        COLD_RESIDENT_PERMILLE.store(u32::MAX, std::sync::atomic::Ordering::Relaxed);
        assert!(cold_verdict().contains("unmeasured"));
    }
}
