//! Attribute the cost of a **full-materialise** table read, stage by stage.
//!
//! `decode` is three costs stacked. This example peels them apart on the *same*
//! blob so the answer to "why is a materialise-everything read slower than a row
//! store?" is measured, not guessed:
//!
//! | stage | adds | measures |
//! |---|---|---|
//! | S0 `scan`       | drain the stream, never `execute` | layout walk + segment fetch |
//! | S1 `+ struct`   | `chunk.execute::<StructArray>()`  | struct canonicalisation |
//! | S2 `+ fields`   | `field.execute::<PrimitiveArray>()` | **the real decompress** |
//! | S3 `decode()`   | `extend_column` copies into `Vec`   | host materialisation |
//!
//! S2−S1 is the genuine per-value decompress cost of Vortex's cascaded
//! encodings; S3−S2 is the copy a columnar consumer would not pay; S1+S0 is path
//! overhead. Run with a row count large enough to clear the fixed costs:
//!
//! ```text
//! cargo run --release -p tessera-io --example read_profile -- [rows] [reps]
//! ```

use std::time::Instant;

use futures::StreamExt;
use tessera_core::block::table::{Column, TableSpec};
use tessera_io::table::{decode, decode_projected, encode, ColumnData, TableData, ROWS_PER_GROUP};
use vortex_array::arrays::struct_::StructArrayExt;
use vortex_array::arrays::{PrimitiveArray, StructArray};
use vortex_array::dtype::PType;
use vortex_array::scalar_fn::session::ScalarFnSession;
use vortex_array::session::ArraySession;
use vortex_array::VortexSessionExecute;
use vortex_buffer::ByteBuffer;
use vortex_file::{register_default_encodings, OpenOptionsSessionExt};
use vortex_io::runtime::current::CurrentThreadRuntime;
use vortex_io::runtime::BlockingRuntime;
use vortex_io::session::{RuntimeSession, RuntimeSessionExt};
use vortex_layout::session::LayoutSession;
use vortex_session::VortexSession;

/// A PET-listmode-shaped table: wide, float-dominated, with the many
/// low-cardinality / constant detector columns real listmode carries.
fn listmode(n: usize) -> TableData {
    let zf = || vec![0.0f64; n];
    let ramp = |scale: f64| {
        (0..n)
            .map(|i| (i as f64 % 977.0) * scale)
            .collect::<Vec<f64>>()
    };
    vec![
        ("decay_id".into(), ColumnData::U64((0..n as u64).collect())),
        ("order_key".into(), ColumnData::U64((0..n as u64).collect())),
        ("energy_kev".into(), ColumnData::F64(ramp(0.5))),
        ("x_mm".into(), ColumnData::F64(ramp(0.31))),
        ("y_mm".into(), ColumnData::F64(ramp(0.17))),
        ("z_mm".into(), ColumnData::F64(ramp(0.09))),
        ("dir_x".into(), ColumnData::F64(zf())),
        ("dir_y".into(), ColumnData::F64(zf())),
        ("dir_z".into(), ColumnData::F64(vec![1.0; n])),
        ("tof_ns".into(), ColumnData::F64(ramp(0.003))),
        ("time_ns".into(), ColumnData::F64(ramp(0.003))),
        ("ancestor_transit_ns".into(), ColumnData::F64(zf())),
        ("scattered".into(), ColumnData::U8(vec![0u8; n])),
        ("gamma_origin".into(), ColumnData::U8(vec![0u8; n])),
        ("detected".into(), ColumnData::U8(vec![0u8; n])),
        ("crystal_id".into(), ColumnData::U64(vec![0u64; n])),
        ("det_x_mm".into(), ColumnData::F64(zf())),
        ("det_y_mm".into(), ColumnData::F64(zf())),
        ("det_z_mm".into(), ColumnData::F64(zf())),
        ("det_energy_kev".into(), ColumnData::F64(zf())),
        ("det_time_ns".into(), ColumnData::F64(zf())),
    ]
}

fn session() -> (CurrentThreadRuntime, VortexSession) {
    let rt = CurrentThreadRuntime::new();
    let s = VortexSession::empty()
        .with::<ArraySession>()
        .with::<LayoutSession>()
        .with::<ScalarFnSession>()
        .with::<RuntimeSession>();
    register_default_encodings(&s);
    let s = s.with_handle(rt.handle());
    (rt, s)
}

/// `stages` controls how far down the decode chain each chunk is taken.
fn read_stage(blob: &[u8], ncols: usize, stage: u8) -> usize {
    let (rt, s) = session();
    let mut ctx = s.create_execution_ctx();
    let mut rows = 0usize;
    rt.block_on(async {
        let stream = s
            .open_options()
            .open_buffer(ByteBuffer::copy_from(blob))
            .unwrap()
            .scan()
            .unwrap()
            .into_array_stream()
            .unwrap();
        futures::pin_mut!(stream);
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.unwrap();
            if stage == 0 {
                rows += chunk.len();
                continue;
            }
            let st: StructArray = chunk.execute(&mut ctx).unwrap();
            rows += st.len();
            if stage == 1 {
                continue;
            }
            for i in 0..ncols {
                let field = st.unmasked_field(i).clone();
                // Bool/Utf8 columns are absent from this schema by construction.
                let prim: PrimitiveArray = field.execute(&mut ctx).unwrap();
                if stage == 2 {
                    std::hint::black_box(prim.len());
                } else {
                    // Stage 3: touch every decompressed value, so a lazily-canonical
                    // array cannot masquerade as a cheap `execute`. (This schema is
                    // f64/u64/u8 only.)
                    match prim.ptype() {
                        PType::F64 => {
                            std::hint::black_box(prim.as_slice::<f64>().iter().sum::<f64>());
                        }
                        PType::U64 => {
                            std::hint::black_box(
                                prim.as_slice::<u64>().iter().fold(0u64, |a, &x| a ^ x),
                            );
                        }
                        _ => {
                            std::hint::black_box(
                                prim.as_slice::<u8>().iter().fold(0u8, |a, &x| a ^ x),
                            );
                        }
                    }
                }
            }
        }
    });
    rows
}

/// Prototype: **column-parallel** materialise. The scan + struct-execute stay on
/// the driver (they're ~2 % of the cost); the expensive part — per-field
/// `execute` (decompress) plus the copy into the host `Vec` — fans out with one
/// thread per column group. Each column is touched by exactly one thread and
/// row-groups are appended in order, so the result is bit-identical to `decode`.
fn read_column_parallel(blob: &[u8], spec: &TableSpec, threads: usize) -> Vec<ColumnData> {
    let (rt, s) = session();
    let ncols = spec.columns.len();
    let rows = spec.rows as usize;

    // Phase A (driver): drain the stream into per-chunk struct arrays. These still
    // hold *compressed* fields — the resident cost is ~the blob, which the caller
    // already has in memory.
    let mut ctx = s.create_execution_ctx();
    let mut chunks: Vec<StructArray> = Vec::new();
    rt.block_on(async {
        let stream = s
            .open_options()
            .open_buffer(ByteBuffer::copy_from(blob))
            .unwrap()
            .scan()
            .unwrap()
            .into_array_stream()
            .unwrap();
        futures::pin_mut!(stream);
        while let Some(chunk) = stream.next().await {
            chunks.push(chunk.unwrap().execute(&mut ctx).unwrap());
        }
    });

    // Phase B: column-parallel decompress + copy.
    let mut cols: Vec<ColumnData> = spec
        .columns
        .iter()
        .map(|c| match c.dtype.as_str() {
            "f8" => ColumnData::F64(Vec::with_capacity(rows)),
            "u8" => ColumnData::U64(Vec::with_capacity(rows)),
            _ => ColumnData::U8(Vec::with_capacity(rows)),
        })
        .collect();
    let per = ncols.div_ceil(threads.max(1));
    let chunks = &chunks;
    let sref = &s;
    std::thread::scope(|sc| {
        for (t, group) in cols.chunks_mut(per).enumerate() {
            sc.spawn(move || {
                let mut ctx = sref.create_execution_ctx();
                for (j, col) in group.iter_mut().enumerate() {
                    let i = t * per + j;
                    for st in chunks {
                        let prim: PrimitiveArray =
                            st.unmasked_field(i).clone().execute(&mut ctx).unwrap();
                        match col {
                            ColumnData::F64(v) => v.extend_from_slice(prim.as_slice::<f64>()),
                            ColumnData::U64(v) => v.extend_from_slice(prim.as_slice::<u64>()),
                            ColumnData::U8(v) => v.extend_from_slice(prim.as_slice::<u8>()),
                            _ => unreachable!(),
                        }
                    }
                }
            });
        }
    });
    cols
}

fn main() {
    let rows: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(2_000_000);
    let reps: usize = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(7);

    let data = listmode(rows);
    let columns: Vec<Column> = data
        .iter()
        .map(|(n, c)| Column::new(n.clone(), c.numpy_code()))
        .collect();
    let ncols = columns.len();
    let spec = TableSpec {
        columns,
        rows: rows as u64,
        row_index: None,
    };
    let blob = encode(&spec, &data).expect("encode");
    let groups = rows.div_ceil(ROWS_PER_GROUP);

    let best = |f: &dyn Fn() -> usize| {
        let mut b = f64::MAX;
        for _ in 0..reps {
            let t = Instant::now();
            std::hint::black_box(f());
            b = b.min(t.elapsed().as_secs_f64());
        }
        b * 1e3
    };

    let s0 = best(&|| read_stage(&blob, ncols, 0));
    let s1 = best(&|| read_stage(&blob, ncols, 1));
    let s2 = best(&|| read_stage(&blob, ncols, 2));
    let s2t = best(&|| read_stage(&blob, ncols, 3));
    let s3 = best(&|| decode(&spec, &blob).expect("decode").len());

    println!(
        "# read cost attribution — {rows} rows x {ncols} cols, {groups} row-groups, \
         {:.1} MiB blob, best-of-{reps}\n",
        blob.len() as f64 / (1024.0 * 1024.0)
    );
    println!("{:<34} {:>9} {:>9}", "stage", "ms", "delta");
    println!("{:<34} {:>9.2} {:>9.2}", "S0 scan (no execute)", s0, s0);
    println!("{:<34} {:>9.2} {:>9.2}", "S1 + struct execute", s1, s1 - s0);
    println!(
        "{:<34} {:>9.2} {:>9.2}   <- decompress",
        "S2 + field execute",
        s2,
        s2 - s1
    );
    println!(
        "{:<34} {:>9.2} {:>9.2}   <- touch every byte",
        "S2b + read decompressed bytes",
        s2t,
        s2t - s2
    );
    println!(
        "{:<34} {:>9.2} {:>9.2}   <- host copy",
        "S3 decode() to ColumnData",
        s3,
        s3 - s2t
    );
    println!(
        "\nsplit: path overhead {:.0}% | decompress {:.0}% | materialise {:.0}%",
        100.0 * s1 / s3,
        100.0 * (s2t - s1) / s3,
        100.0 * (s3 - s2t) / s3
    );

    // Projection must still pay off: reading 5 of 21 columns should cost ~5/21 of the work.
    let proj = ["energy_kev", "x_mm", "y_mm", "z_mm", "time_ns"];
    let p = best(&|| {
        decode_projected(&spec, &blob, &proj)
            .expect("projected")
            .len()
    });
    println!(
        "\nprojected read (5 of 21 cols): {p:.2} ms   ({:.2}x vs full decode {s3:.2} ms)",
        s3 / p
    );

    // Thread-scaling of the materialise, measured standalone (own unpooled session) so the curve
    // is visible independent of what `decode` currently does. `threads 1` is the old serial
    // behaviour; `decode` itself now runs the pooled column-parallel path.
    let reference = decode(&spec, &blob).expect("decode");
    println!("\ncolumn-parallel materialise — thread scaling (standalone, unpooled session):");
    let mut serial = f64::NAN;
    for t in [1usize, 2, 4, 8, 10, 16] {
        let ms = best(&|| read_column_parallel(&blob, &spec, t).len());
        if t == 1 {
            serial = ms;
        }
        let got = read_column_parallel(&blob, &spec, t);
        let same = reference.iter().map(|(_, c)| c).eq(got.iter());
        println!(
            "  threads {t:>3}: {ms:>7.2} ms   {:.2}x vs 1 thread   bit-identical={same}",
            serial / ms
        );
    }
}
