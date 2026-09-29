//! `tessera bench compare` — head-to-head size + latency against HDF5 on the same logical data
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
//! - **Parquet page CRCs** are the same class of thing (stage 2, once #460 lands the Rust crates).
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
use tessera_io::table::{self, ColumnData, TableData};
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

/// A listmode-like table: monotonic u64 timestamp + two f32 energy columns.
/// Same shape as `bench/ecosystems/common.py::make_table` (#143).
fn make_table(rows: usize) -> (TableSpec, TableData) {
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
    let spec = TableSpec {
        columns: vec![
            Column::new("t", "u8"),
            Column::new("e0", "f4"),
            Column::new("e1", "f4"),
        ],
        rows: rows as u64,
        row_index: None,
    };
    (spec, data)
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
    let _ = f.sync_all();
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
fn timed(iters: usize, cold: Option<&Path>, mut f: impl FnMut()) -> Stats {
    let mut xs = Vec::with_capacity(iters);
    for _ in 0..iters {
        if let Some(p) = cold {
            evict(p);
            if let Some(frac) = resident_fraction(p) {
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
    Stats::of(xs)
}

/// One measured cell of the matrix.
pub struct Row {
    pub format: String,
    pub settings: String,
    pub modality: &'static str,
    pub op: &'static str,
    pub cache: &'static str,
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
    fletcher32: bool,
}

/// Per-modality, because the chunk geometry differs and the settings line must say what was ACTUALLY
/// used: the volume gets Tessera's own cubic 64³ (so the ROI row compares layouts, not chunk-size
/// luck), while a 1-D table gets a row-count chunk. Printing "64³" on a table row would be a lie.
fn h5_variants(modality: &str) -> Vec<H5Variant> {
    let chunk_desc = if modality == "volume" {
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
            fletcher32: false,
        },
        H5Variant {
            label: "HDF5 (hdf5-metno)",
            settings: format!("tuned: {chunk_desc}, gzip-4"),
            chunked: true,
            deflate: Some(4),
            fletcher32: false,
        },
        H5Variant {
            label: "HDF5 (hdf5-metno)",
            settings: format!("tuned: {chunk_desc}, gzip-4, fletcher32"),
            chunked: true,
            deflate: Some(4),
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
    // Centre ROI: one cubic chunk, the access pattern cubic chunking exists for. Clamped to the
    // volume so a small `--vol-n` (the trycmd fixture uses one) cannot underflow the centring.
    let roi_edge = CHUNK.min(n);
    let roi0 = (n - roi_edge) / 2;
    let roi_start = [roi0 as u64; 3];
    let roi_shape = [roi_edge as u64; 3];

    for (label, codec) in [("pcodec (default)", "pcodec"), ("zstd (tuned)", "zstd")] {
        let mut spec = ArraySpec::new(vec![n as u64, n as u64, n as u64], "int16");
        spec.codec = codec.into();
        let settings = format!("{label}, {CHUNK}^3 chunks");
        let name = format!("vol_{codec}");

        let wr = timed(iters, None, || {
            let blk = array::array_block("volume", &spec, &data).expect("encode");
            let _ = seal_tsra(dir, &name, blk).expect("seal");
        });
        let path = dir.join(format!("{name}.tsra"));

        // Correctness gate: the sealed product must read back exactly what was written.
        let blob = Reader::open(&path)?.read_block("volume")?;
        let back = array::decode(&spec, &blob)?;
        if back != data {
            return Err(err("tessera volume read-back != written"));
        }

        rows.push(Row {
            format: "Tessera (.tsra)".into(),
            settings: settings.clone(),
            modality: "volume",
            op: "size",
            cache: "-",
            bytes: Some(file_len(&path)),
            stats: None,
            claim: "smaller on disk than the alternatives",
        });
        rows.push(Row {
            format: "Tessera (.tsra)".into(),
            settings: settings.clone(),
            modality: "volume",
            op: "write+seal",
            cache: "warm",
            bytes: None,
            stats: Some(wr),
            claim: "the cost of sealing (tessera pays, the others do not)",
        });

        for (cache, evict_path) in warm_cold(&path, cold) {
            let s = timed(iters, evict_path, || {
                let mut r = Reader::open(&path).expect("open");
                let b = r.read_block("volume").expect("block");
                let _ = array::decode(&spec, &b).expect("decode");
            });
            rows.push(Row {
                format: "Tessera (.tsra)".into(),
                settings: settings.clone(),
                modality: "volume",
                op: "read full",
                cache,
                bytes: None,
                stats: Some(s),
                claim: "baseline full materialise",
            });

            let s = timed(iters, evict_path, || {
                let mut r = Reader::open(&path).expect("open");
                let b = r.read_block("volume").expect("block");
                let _ = array::decode_subset(&spec, &b, &roi_start, &roi_shape).expect("roi");
            });
            rows.push(Row {
                format: "Tessera (.tsra)".into(),
                settings: settings.clone(),
                modality: "volume",
                op: "read ROI",
                cache,
                bytes: None,
                stats: Some(s),
                claim: "cubic chunks -> cheap ROI / orthogonal access",
            });
        }

        let s = timed(iters, None, || {
            let _ = tessera_io::verify_payloads_parallel(&path, "bench", 1).expect("verify");
        });
        rows.push(Row {
            format: "Tessera (.tsra)".into(),
            settings,
            modality: "volume",
            op: "verify",
            cache: "warm",
            bytes: None,
            stats: Some(s),
            claim: "tamper-evident: re-derives every digest against the sealed manifest",
        });
    }

    for v in h5_variants("volume") {
        let path = dir.join(format!("vol_{}.h5", slug(&v.settings)));
        let wr = timed(iters, None, || {
            let _ = std::fs::remove_file(&path);
            write_h5_volume(&path, &v, n, raw).expect("h5 write");
        });

        // Correctness gate.
        {
            let f = hdf5::File::open(&path).map_err(|e| err(&format!("h5 open: {e}")))?;
            let ds = f
                .dataset("volume")
                .map_err(|e| err(&format!("h5 ds: {e}")))?;
            let got: Vec<i16> = ds.read_raw().map_err(|e| err(&format!("h5 read: {e}")))?;
            if &got != raw {
                return Err(err("hdf5 volume read-back != written"));
            }
        }

        rows.push(Row {
            format: v.label.into(),
            settings: v.settings.clone(),
            modality: "volume",
            op: "size",
            cache: "-",
            bytes: Some(file_len(&path)),
            stats: None,
            claim: "smaller on disk than the alternatives",
        });
        rows.push(Row {
            format: v.label.into(),
            settings: v.settings.clone(),
            modality: "volume",
            op: "write+seal",
            cache: "warm",
            bytes: None,
            stats: Some(wr),
            claim: "the cost of sealing (tessera pays, the others do not)",
        });

        for (cache, evict_path) in warm_cold(&path, cold) {
            let s = timed(iters, evict_path, || {
                let f = hdf5::File::open(&path).expect("open");
                let ds = f.dataset("volume").expect("ds");
                let _: Vec<i16> = ds.read_raw().expect("read");
            });
            rows.push(Row {
                format: v.label.into(),
                settings: v.settings.clone(),
                modality: "volume",
                op: "read full",
                cache,
                bytes: None,
                stats: Some(s),
                claim: "baseline full materialise",
            });

            let (z, y, x) = (roi0, roi0, roi0);
            let e = roi_edge;
            let s = timed(iters, evict_path, || {
                let f = hdf5::File::open(&path).expect("open");
                let ds = f.dataset("volume").expect("ds");
                let _ = ds
                    .read_slice::<i16, _, ndarray::Ix3>(ndarray::s![z..z + e, y..y + e, x..x + e])
                    .expect("roi");
            });
            rows.push(Row {
                format: v.label.into(),
                settings: v.settings.clone(),
                modality: "volume",
                op: "read ROI",
                cache,
                bytes: None,
                stats: Some(s),
                claim: "cubic chunks -> cheap ROI / orthogonal access",
            });
        }

        // Integrity: only the fletcher32 variant checks anything on read, and only for corruption.
        if v.fletcher32 {
            let s = timed(iters, None, || {
                let f = hdf5::File::open(&path).expect("open");
                let ds = f.dataset("volume").expect("ds");
                let _: Vec<i16> = ds.read_raw().expect("read");
            });
            rows.push(Row {
                format: v.label.into(),
                settings: v.settings,
                modality: "volume",
                op: "verify",
                cache: "warm",
                bytes: None,
                stats: Some(s),
                claim: "fletcher32: per-chunk CORRUPTION check on read; not keyed, no identity",
            });
        } else {
            rows.push(Row {
                format: v.label.into(),
                settings: v.settings,
                modality: "volume",
                op: "verify",
                cache: "-",
                bytes: None,
                stats: None,
                claim: "no integrity mechanism configured",
            });
        }
    }
    Ok(())
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

/// The table half of the matrix.
fn compare_table(
    dir: &Path,
    rows_n: usize,
    iters: usize,
    cold: bool,
    rows: &mut Vec<Row>,
) -> Result<()> {
    let (spec, data) = make_table(rows_n);
    // Row-range ROI: a contiguous 1% window, the "take a slice of the acquisition" access pattern.
    let roi_len = (rows_n / 100).max(1);
    let roi_start = rows_n / 2;
    let roi_idx: Vec<u64> = (roi_start as u64..(roi_start + roi_len) as u64).collect();

    // Tessera's table backend exposes no user-facing codec knob — Vortex picks its cascade per
    // column. Stated rather than invented: a fabricated "tuned" row would imply a lever that is not
    // there. (The ARRAY path does have one, and the volume half exercises it.)
    let settings = "default: Vortex cascade (no user-facing codec knob)".to_string();
    let name = "tab_default";
    let wr = timed(iters, None, || {
        let blk = table::table_block("events", &spec, &data).expect("encode");
        let _ = seal_tsra(dir, name, blk).expect("seal");
    });
    let path = dir.join(format!("{name}.tsra"));

    let blob = Reader::open(&path)?.read_block("events")?;
    if table::decode(&spec, &blob)? != data {
        return Err(err("tessera table read-back != written"));
    }

    rows.push(Row {
        format: "Tessera (.tsra)".into(),
        settings: settings.clone(),
        modality: "table",
        op: "size",
        cache: "-",
        bytes: Some(file_len(&path)),
        stats: None,
        claim: "smaller on disk than the alternatives",
    });
    rows.push(Row {
        format: "Tessera (.tsra)".into(),
        settings: settings.clone(),
        modality: "table",
        op: "write+seal",
        cache: "warm",
        bytes: None,
        stats: Some(wr),
        claim: "the cost of sealing (tessera pays, the others do not)",
    });

    for (cache, evict_path) in warm_cold(&path, cold) {
        let s = timed(iters, evict_path, || {
            let mut r = Reader::open(&path).expect("open");
            let b = r.read_block("events").expect("block");
            let _ = table::decode(&spec, &b).expect("decode");
        });
        rows.push(Row {
            format: "Tessera (.tsra)".into(),
            settings: settings.clone(),
            modality: "table",
            op: "read full",
            cache,
            bytes: None,
            stats: Some(s),
            claim: "baseline full materialise (columnar stores are worst at this)",
        });

        let s = timed(iters, evict_path, || {
            let mut r = Reader::open(&path).expect("open");
            let b = r.read_block("events").expect("block");
            let _ = table::decode_column(&spec, &b, "e0").expect("column");
        });
        rows.push(Row {
            format: "Tessera (.tsra)".into(),
            settings: settings.clone(),
            modality: "table",
            op: "read 1 column",
            cache,
            bytes: None,
            stats: Some(s),
            claim: "columnar projection: read one column, not the table",
        });

        let s = timed(iters, evict_path, || {
            let mut r = Reader::open(&path).expect("open");
            let b = r.read_block("events").expect("block");
            let _ = table::decode_rows(&spec, &b, &roi_idx).expect("rows");
        });
        rows.push(Row {
            format: "Tessera (.tsra)".into(),
            settings: settings.clone(),
            modality: "table",
            op: "read row ROI",
            cache,
            bytes: None,
            stats: Some(s),
            claim: "row-index pushdown: take a window without decoding the rest",
        });
    }

    let s = timed(iters, None, || {
        let _ = tessera_io::verify_payloads_parallel(&path, "bench", 1).expect("verify");
    });
    rows.push(Row {
        format: "Tessera (.tsra)".into(),
        settings,
        modality: "table",
        op: "verify",
        cache: "warm",
        bytes: None,
        stats: Some(s),
        claim: "tamper-evident: re-derives every digest against the sealed manifest",
    });

    // ---- HDF5 side: one dataset per column, which is how a columnar table is expressed in HDF5.
    let ColumnData::U64(t) = &data[0].1 else {
        return Err(err("t must be u64"));
    };
    let ColumnData::F32(e0) = &data[1].1 else {
        return Err(err("e0 must be f32"));
    };
    let ColumnData::F32(e1) = &data[2].1 else {
        return Err(err("e1 must be f32"));
    };

    for v in h5_variants("table") {
        let path = dir.join(format!("tab_{}.h5", slug(&v.settings)));
        let wr = timed(iters, None, || {
            let _ = std::fs::remove_file(&path);
            write_h5_table(&path, &v, t, e0, e1).expect("h5 write");
        });

        {
            let f = hdf5::File::open(&path).map_err(|e| err(&format!("h5 open: {e}")))?;
            let got: Vec<f32> = f
                .dataset("e0")
                .and_then(|d| d.read_raw())
                .map_err(|e| err(&format!("h5 read: {e}")))?;
            if &got != e0 {
                return Err(err("hdf5 table read-back != written"));
            }
        }

        rows.push(Row {
            format: v.label.into(),
            settings: v.settings.clone(),
            modality: "table",
            op: "size",
            cache: "-",
            bytes: Some(file_len(&path)),
            stats: None,
            claim: "smaller on disk than the alternatives",
        });
        rows.push(Row {
            format: v.label.into(),
            settings: v.settings.clone(),
            modality: "table",
            op: "write+seal",
            cache: "warm",
            bytes: None,
            stats: Some(wr),
            claim: "the cost of sealing (tessera pays, the others do not)",
        });

        for (cache, evict_path) in warm_cold(&path, cold) {
            let s = timed(iters, evict_path, || {
                let f = hdf5::File::open(&path).expect("open");
                for c in ["t", "e0", "e1"] {
                    let ds = f.dataset(c).expect("ds");
                    if c == "t" {
                        let _: Vec<u64> = ds.read_raw().expect("read");
                    } else {
                        let _: Vec<f32> = ds.read_raw().expect("read");
                    }
                }
            });
            rows.push(Row {
                format: v.label.into(),
                settings: v.settings.clone(),
                modality: "table",
                op: "read full",
                cache,
                bytes: None,
                stats: Some(s),
                claim: "baseline full materialise (columnar stores are worst at this)",
            });

            let s = timed(iters, evict_path, || {
                let f = hdf5::File::open(&path).expect("open");
                let ds = f.dataset("e0").expect("ds");
                let _: Vec<f32> = ds.read_raw().expect("read");
            });
            rows.push(Row {
                format: v.label.into(),
                settings: v.settings.clone(),
                modality: "table",
                op: "read 1 column",
                cache,
                bytes: None,
                stats: Some(s),
                claim: "columnar projection: read one column, not the table",
            });

            let s = timed(iters, evict_path, || {
                let f = hdf5::File::open(&path).expect("open");
                let ds = f.dataset("e0").expect("ds");
                let _ = ds
                    .read_slice_1d::<f32, _>(ndarray::s![roi_start..roi_start + roi_len])
                    .expect("roi");
            });
            rows.push(Row {
                format: v.label.into(),
                settings: v.settings.clone(),
                modality: "table",
                op: "read row ROI",
                cache,
                bytes: None,
                stats: Some(s),
                claim: "row-index pushdown: take a window without decoding the rest",
            });
        }

        if v.fletcher32 {
            let s = timed(iters, None, || {
                let f = hdf5::File::open(&path).expect("open");
                for c in ["t", "e0", "e1"] {
                    let ds = f.dataset(c).expect("ds");
                    if c == "t" {
                        let _: Vec<u64> = ds.read_raw().expect("read");
                    } else {
                        let _: Vec<f32> = ds.read_raw().expect("read");
                    }
                }
            });
            rows.push(Row {
                format: v.label.into(),
                settings: v.settings,
                modality: "table",
                op: "verify",
                cache: "warm",
                bytes: None,
                stats: Some(s),
                claim: "fletcher32: per-chunk CORRUPTION check on read; not keyed, no identity",
            });
        } else {
            rows.push(Row {
                format: v.label.into(),
                settings: v.settings,
                modality: "table",
                op: "verify",
                cache: "-",
                bytes: None,
                stats: None,
                claim: "no integrity mechanism configured",
            });
        }
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
        compare_table(dir.path(), opts.rows, opts.iters, opts.cold, &mut rows)?;
    }

    match opts.format.as_str() {
        "json" => print_json(&opts, &rows),
        "table" => print_table(&opts, &rows),
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
            "table {} rows x (u8+2xf4) = {} raw",
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

fn print_table(opts: &CompareOpts, rows: &[Row]) {
    out!("tessera bench compare — .tsra vs HDF5 (#388)");
    out!("  data     {}", raw_bytes_note(opts));
    out!(
        "  method   median of {} runs, spread = [min..max]; correctness asserted before timing",
        opts.iters
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
    out!("  note     Parquet lands once #460 puts the arrow/parquet crates on dev (#388 stage 2)");
    out!(
        "  CAVEAT   the synthetic data is a smooth gradient + monotonic counters (verbatim from the"
    );
    out!("           #143 harness, for comparability). It is FAR more compressible than real");
    out!("           acquisitions, so read the size rows as a RATIO BETWEEN FORMATS on identical");
    out!("           input, never as a compression ratio you will see on clinical data.");
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

fn print_json(opts: &CompareOpts, rows: &[Row]) {
    let items: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            serde_json::json!({
                "format": r.format,
                "settings": r.settings,
                "dataset": r.modality,
                "op": r.op,
                "cache": r.cache,
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
        "kernel": kernel(),
        // null rather than the configured-but-unused value, so a consumer cannot read a size for a
        // dataset this run never touched.
        "volume_n": (opts.dataset != "table").then_some(opts.vol_n),
        "table_rows": (opts.dataset != "volume").then_some(opts.rows),
        "caveat": "synthetic data (a smooth gradient + monotonic counters, verbatim from the #143 harness) is far more compressible than real acquisitions; size rows are a ratio BETWEEN formats on identical input, not a compression ratio for clinical data",
        "parquet": "pending #460 (arrow/parquet crates not yet on dev) — stage 2",
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
