//! Read a `.tsra` the C++ smoke test wrote and compare its `events` table, value for value, with the
//! raw column dumps the C++ side wrote of what it pushed (`<col>.bin` little-endian values,
//! `tof.valid` one byte per row). Exits non-zero on the first difference.
//!
//!   cargo run -p tessera-capi --example readback -- <product.tsra> <dump_dir>

use std::path::Path;

use tessera_io::container::Reader;
use tessera_io::table::ColumnData;

fn fail(msg: String) -> ! {
    eprintln!("FAIL: {msg}");
    std::process::exit(1);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: readback <product.tsra> <dump_dir>");
        std::process::exit(2);
    }
    let dumps = Path::new(&args[2]);
    let read = |name: &str| {
        std::fs::read(dumps.join(name)).unwrap_or_else(|e| fail(format!("read {name}: {e}")))
    };
    let mut reader =
        Reader::open(Path::new(&args[1])).unwrap_or_else(|e| fail(format!("open: {e}")));
    let view = reader
        .logical_table("events")
        .unwrap_or_else(|e| fail(format!("events: {e}")));
    let rows = view.row_count() as usize;
    for name in ["t", "crystal_a", "crystal_b", "energy"] {
        let col = view
            .column(&mut reader, name)
            .unwrap_or_else(|e| fail(format!("{name}: {e}")));
        if col.to_le_bytes() != read(&format!("{name}.bin")) {
            fail(format!("column '{name}' differs from what C++ pushed"));
        }
    }
    let ColumnData::Nullable { values, validity } = view
        .column(&mut reader, "tof")
        .unwrap_or_else(|e| fail(format!("tof: {e}")))
    else {
        fail("tof is not a nullable column".into())
    };
    let want_valid: Vec<bool> = read("tof.valid").into_iter().map(|b| b != 0).collect();
    if validity != want_valid {
        fail("tof's validity differs from what C++ pushed".into());
    }
    let ColumnData::F32(values) = *values else {
        fail("tof values are not f32".into())
    };
    let want = read("tof.bin");
    for (i, v) in values.iter().enumerate().filter(|(i, _)| validity[*i]) {
        if v.to_le_bytes() != want[4 * i..4 * i + 4] {
            fail(format!("tof[{i}] differs from what C++ pushed"));
        }
    }
    println!("readback OK: {rows} rows, 5 columns identical to what C++ pushed");
}
