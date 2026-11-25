//! Dev tool: write a synthetic GE-3p-shaped compound `.h5` for perf work (#325).
//!
//! Deliberately synthetic — the real DUPLET acquisitions are patient data and must never become a
//! fixture. The record layout mirrors `events_3p` (38 B/row), so a row count here maps to the same
//! bytes/row the issue measured.
//!
//! Usage: `cargo run -p tessera-ingest --example gen_synth_3p -- <out.h5> <rows> [chunk_rows] [gzip]`
//!
//! `chunk_rows` (optional) makes the dataset CHUNKED rather than contiguous, and `gzip` (0-9) adds
//! deflate. Vendor acquisitions are chunked+compressed, and that layout is what stresses the
//! streaming slab reader against libhdf5's (1 MiB by default) per-dataset chunk cache.
use hdf5_metno as hdf5;
use hdf5_metno::H5Type;

#[repr(C)]
#[derive(H5Type, Clone, Copy)]
struct Rec3p {
    ms: u32,
    id: [u16; 3],
    en: [f32; 3],
    vtx: [f32; 3],
    lt: f32,
}

fn main() {
    let mut args = std::env::args().skip(1);
    let out = args.next().expect("usage: gen_synth_3p <out.h5> <rows>");
    let rows: usize = args
        .next()
        .expect("usage: gen_synth_3p <out.h5> <rows>")
        .parse()
        .expect("rows must be an integer");

    let chunk_rows: Option<usize> = args.next().and_then(|s| s.parse().ok()).filter(|c| *c > 0);
    let gzip: Option<u8> = args.next().and_then(|s| s.parse().ok()).filter(|g| *g > 0);

    let f = hdf5::File::create(&out).expect("create h5");
    let g = f.create_group("proc_data").expect("create group");
    let mut b = g.new_dataset::<Rec3p>().shape(rows);
    if let Some(c) = chunk_rows {
        b = b.chunk(c.min(rows));
    }
    if let Some(z) = gzip {
        b = b.deflate(z.min(9));
    }
    let ds = b.create("events_3p").expect("create dataset");

    // Write in chunks so the generator itself stays bounded-memory.
    const CHUNK: usize = 1 << 20;
    let mut written = 0usize;
    while written < rows {
        let n = CHUNK.min(rows - written);
        let recs: Vec<Rec3p> = (0..n)
            .map(|i| {
                let k = (written + i) as u32;
                Rec3p {
                    ms: k,
                    id: [(k % 400) as u16, (k % 397) as u16, (k % 391) as u16],
                    en: [511.0, 511.0 - (k % 64) as f32, 340.0 + (k % 32) as f32],
                    vtx: [0.1 * (k % 128) as f32, 0.2, 0.3],
                    lt: 1.5 + (k % 16) as f32 * 0.01,
                }
            })
            .collect();
        ds.write_slice(&recs, written..written + n).expect("write");
        written += n;
    }
    println!("wrote {rows} rows (38 bytes/row, chunk_rows={chunk_rows:?}, gzip={gzip:?}) -> {out}");
}
