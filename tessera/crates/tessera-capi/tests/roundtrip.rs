//! The C ABI end to end, called exactly as C calls it: the columns read back are the columns pushed,
//! the bytes are those of the native writer, and every misuse is a status, never a crash.

mod common;

use std::ffi::CString;
use std::sync::Arc;

use arrow_array::ffi::{from_ffi, FFI_ArrowSchema};
use arrow_array::{ArrayRef, StringArray, StructArray, TimestampMicrosecondArray, UInt64Array};
use arrow_schema::{DataType, Field, Fields};
use common::*;
use tessera_capi::*;
use tessera_io::container::Reader;
use tessera_io::stream::StreamWriter;
use tessera_io::table::ColumnData;
use tessera_io::WriteSession;

const ROWS: u64 = 150_000; // > 2 row-groups, so the writer re-chunks across batch boundaries
const BATCH: u64 = 10_007; // deliberately off the row-group grid

#[test]
fn columns_read_back_are_the_columns_pushed() {
    let dir = tempfile::tempdir().unwrap();
    write_through_capi(dir.path(), ROWS, BATCH, ROWS);
    let mut reader = Reader::open(&dir.path().join("capi.tsra")).unwrap();
    reader.verify_payloads("capi").unwrap();
    let view = reader.logical_table("events").unwrap();
    assert_eq!(view.row_count(), ROWS);
    let col = |r: &mut Reader<_>, n: &str| view.column(r, n).unwrap();
    let all = 0..ROWS;
    assert_eq!(
        col(&mut reader, "t"),
        ColumnData::U64(all.clone().map(t_of).collect())
    );
    assert_eq!(
        col(&mut reader, "crystal_a"),
        ColumnData::U32(all.clone().map(crystal_a_of).collect())
    );
    assert_eq!(
        col(&mut reader, "crystal_b"),
        ColumnData::U32(all.clone().map(crystal_b_of).collect())
    );
    let ColumnData::F32(energy) = col(&mut reader, "energy") else {
        panic!("energy is f32")
    };
    let want: Vec<u32> = all.clone().map(|i| energy_of(i).to_bits()).collect();
    assert_eq!(energy.iter().map(|v| v.to_bits()).collect::<Vec<_>>(), want);
    let ColumnData::Nullable { values, validity } = col(&mut reader, "tof") else {
        panic!("tof is declared nullable, so it seals as a nullable column")
    };
    assert_eq!(validity, all.clone().map(tof_valid).collect::<Vec<_>>());
    let ColumnData::F32(values) = *values else {
        panic!("tof values are f32")
    };
    for i in all.filter(|&i| tof_valid(i)) {
        assert_eq!(
            values[i as usize].to_bits(),
            tof_of(i).to_bits(),
            "tof[{i}]"
        );
    }
}

/// The bridge adds no byte of its own: the product it seals is the product the native Rust
/// `push_table` seals from the same rows, so the content hashes agree.
#[test]
fn the_bytes_are_those_of_the_native_writer() {
    let dir = tempfile::tempdir().unwrap();
    let capi = write_through_capi(dir.path(), ROWS, BATCH, ROWS);

    let session = WriteSession::create(
        &dir.path().join("native-stage"),
        "listmode",
        "capi-test",
        "written through the C ABI",
        TS,
    )
    .unwrap();
    let mut sw = StreamWriter::new(session, 2, 4);
    sw.push_table("events", native_spec(ROWS), native_groups(ROWS, 65_536))
        .unwrap();
    let native = sw.finish(&dir.path().join("native.tsra")).unwrap();
    assert_eq!(capi, native.content_hash.unwrap());
}

/// An unknown row count (staged in full, encoded at `tsra_table_end`) seals the same bytes as a
/// declared one (encoded as blocks fill).
#[test]
fn an_unknown_row_count_seals_the_same_bytes() {
    let known = tempfile::tempdir().unwrap();
    let unknown = tempfile::tempdir().unwrap();
    assert_eq!(
        write_through_capi(known.path(), ROWS, BATCH, ROWS),
        write_through_capi(unknown.path(), ROWS, BATCH, TSRA_ROWS_UNKNOWN)
    );
}

/// The import does not copy: the imported buffer is the producer's own memory.
#[test]
fn the_import_is_zero_copy() {
    let values = UInt64Array::from_iter_values(0..1024);
    let producer_ptr = values.values().as_ptr();
    let (array, schema) = export(&values);
    // SAFETY: a fresh export and its own schema.
    let imported = unsafe { from_ffi(array, &schema) }.unwrap();
    assert_eq!(imported.buffers()[0].as_ptr(), producer_ptr.cast());
}

#[test]
fn version_and_last_error() {
    assert_eq!(
        tsra_abi_version(),
        TSRA_ABI_VERSION_MAJOR << 16 | TSRA_ABI_VERSION_MINOR
    );
    let s = Strings::new();
    let mut session = std::ptr::null_mut();
    // SAFETY: a NULL stage dir is refused before anything is read through it.
    let st = unsafe { tsra_session_open(std::ptr::null(), &s.opts(1), &mut session) };
    expect(st, TsraStatus::InvalidArgument);
    assert!(session.is_null());
    assert!(last_error().unwrap().contains("stage_dir is NULL"));
    let mut short = s.opts(1);
    short.struct_size = 8;
    let dir = CString::new("/nonexistent-never-created").unwrap();
    // SAFETY: valid pointers; the short struct_size is refused.
    let st = unsafe { tsra_session_open(dir.as_ptr(), &short, &mut session) };
    expect(st, TsraStatus::InvalidArgument);
    assert!(last_error().unwrap().contains("struct_size"));
}

#[test]
fn a_success_clears_the_last_error() {
    let dir = tempfile::tempdir().unwrap();
    // SAFETY: a NULL table is refused.
    expect(
        unsafe { tsra_table_end(std::ptr::null_mut()) },
        TsraStatus::InvalidArgument,
    );
    assert!(last_error().is_some());
    let session = open(&dir.path().join("stage"), 1);
    assert_eq!(last_error(), None);
    // SAFETY: a live session.
    unsafe { tsra_session_free(session) };
}

fn struct_of(fields: Vec<(Field, ArrayRef)>) -> StructArray {
    let (f, a): (Vec<_>, Vec<_>) = fields.into_iter().unzip();
    StructArray::new(Fields::from(f), a, None)
}

#[test]
fn unsupported_and_unnullable_declarations_are_schema_errors() {
    let dir = tempfile::tempdir().unwrap();
    let session = open(&dir.path().join("stage"), 1);
    let ts = Field::new(
        "t",
        DataType::Timestamp(arrow_schema::TimeUnit::Microsecond, None),
        false,
    );
    let schema = FFI_ArrowSchema::try_from(DataType::Struct(Fields::from(vec![ts]))).unwrap();
    let (st, table) = begin(session, &schema, 1);
    expect(st, TsraStatus::Schema);
    assert!(table.is_null());
    assert!(last_error().unwrap().contains("column 't'"));
    let s = Field::new("label", DataType::Utf8, true);
    let schema = FFI_ArrowSchema::try_from(DataType::Struct(Fields::from(vec![s]))).unwrap();
    expect(begin(session, &schema, 1).0, TsraStatus::Schema);
    assert!(last_error().unwrap().contains("declared nullable"));
    let flat = FFI_ArrowSchema::try_from(DataType::UInt64).unwrap();
    expect(begin(session, &flat, 1).0, TsraStatus::Schema);
    // A refused declaration leaves the session usable.
    let schema = FFI_ArrowSchema::try_from(DataType::Struct(fields())).unwrap();
    let (st, table) = begin(session, &schema, 10);
    expect(st, TsraStatus::Ok);
    expect(push(table, 0, &batch(0, 10)), TsraStatus::Ok);
    expect(end(table), TsraStatus::Ok);
    expect(seal(session, &dir.path().join("p.tsra")).0, TsraStatus::Ok);
}

#[test]
fn a_mismatched_batch_is_a_schema_error_and_is_still_released() {
    let dir = tempfile::tempdir().unwrap();
    let session = open(&dir.path().join("stage"), 1);
    let schema = FFI_ArrowSchema::try_from(DataType::Struct(fields())).unwrap();
    let (st, table) = begin(session, &schema, 20);
    expect(st, TsraStatus::Ok);
    // Wrong type for a column (and a different shape entirely): refused, released.
    let wrong = struct_of(vec![(
        Field::new("t", DataType::Utf8, false),
        Arc::new(StringArray::from(vec!["a"])) as ArrayRef,
    )]);
    expect(push(table, 0, &wrong), TsraStatus::Schema);
    let wrong_type = struct_of(vec![(
        Field::new(
            "t",
            DataType::Timestamp(arrow_schema::TimeUnit::Microsecond, None),
            false,
        ),
        Arc::new(TimestampMicrosecondArray::from(vec![1])) as ArrayRef,
    )]);
    expect(push(table, 0, &wrong_type), TsraStatus::Schema);
    // A NULL in a column declared non-nullable.
    let mut cols: Vec<ArrayRef> = batch(0, 10).columns().to_vec();
    cols[0] = Arc::new(UInt64Array::from(
        vec![Some(1); 9]
            .into_iter()
            .chain([None])
            .collect::<Vec<_>>(),
    ));
    let nulls = StructArray::new(
        Fields::from(
            fields()
                .iter()
                .enumerate()
                .map(|(i, f)| f.as_ref().clone().with_nullable(i == 0 || f.is_nullable()))
                .collect::<Vec<_>>(),
        ),
        cols,
        None,
    );
    expect(push(table, 0, &nulls), TsraStatus::Schema);
    assert!(last_error().unwrap().contains("declared non-nullable"));
    // A NULL array pointer, and a released one.
    let s = schema_of(&batch(0, 1));
    // SAFETY: a NULL array is refused before anything is read.
    let st = unsafe { tsra_table_push(table, 0, std::ptr::null_mut(), &s) };
    expect(st, TsraStatus::InvalidArgument);
    let mut released = arrow_array::ffi::FFI_ArrowArray::empty();
    // SAFETY: an empty (released) array is detected and refused.
    let st = unsafe { tsra_table_push(table, 0, &mut released, &s) };
    expect(st, TsraStatus::InvalidArgument);
    // The table is still fine: refused batches don't consume a sequence number.
    expect(push(table, 0, &batch(0, 10)), TsraStatus::Ok);
    expect(push(table, 1, &batch(10, 10)), TsraStatus::Ok);
    expect(end(table), TsraStatus::Ok);
    expect(seal(session, &dir.path().join("p.tsra")).0, TsraStatus::Ok);
}

#[test]
fn duplicates_gaps_and_short_tables_are_refused_and_poison() {
    let dir = tempfile::tempdir().unwrap();
    let schema = FFI_ArrowSchema::try_from(DataType::Struct(fields())).unwrap();

    // A duplicate is refused at once; a gap at the end poisons the session.
    let session = open(&dir.path().join("a"), 1);
    let (_, table) = begin(session, &schema, 30);
    expect(push(table, 0, &batch(0, 10)), TsraStatus::Ok);
    expect(push(table, 0, &batch(0, 10)), TsraStatus::Order);
    expect(push(table, 2, &batch(20, 10)), TsraStatus::Ok);
    expect(end(table), TsraStatus::Order);
    assert!(last_error().unwrap().contains("wait for seq 1"));
    let (st, _) = seal(session, &dir.path().join("a.tsra"));
    expect(st, TsraStatus::State);
    assert!(!dir.path().join("a.tsra").exists());

    // Fewer rows than declared: the writer's own refusal, reported at the end.
    let session = open(&dir.path().join("b"), 1);
    let (_, table) = begin(session, &schema, 30);
    expect(push(table, 0, &batch(0, 10)), TsraStatus::Ok);
    expect(end(table), TsraStatus::Writer);
    assert!(
        last_error().unwrap().contains("events"),
        "{:?}",
        last_error()
    );
    expect(
        seal(session, &dir.path().join("b.tsra")).0,
        TsraStatus::State,
    );

    // More rows than declared: the writer stops, and a later push or the end says so.
    let session = open(&dir.path().join("c"), 1);
    let (_, table) = begin(session, &schema, 5);
    let _ = push(table, 0, &batch(0, 10));
    expect(end(table), TsraStatus::Writer);
    // SAFETY: a live session.
    unsafe { tsra_session_free(session) };
}

#[test]
fn one_table_at_a_time_and_none_open_at_seal() {
    let dir = tempfile::tempdir().unwrap();
    let session = open(&dir.path().join("stage"), 1);
    let schema = FFI_ArrowSchema::try_from(DataType::Struct(fields())).unwrap();
    let (st, table) = begin(session, &schema, 10);
    expect(st, TsraStatus::Ok);
    expect(begin(session, &schema, 10).0, TsraStatus::State);
    assert!(last_error().unwrap().contains("still open"));
    expect(push(table, 0, &batch(0, 10)), TsraStatus::Ok);
    expect(end(table), TsraStatus::Ok);
    // A second table after the first ended is fine.
    let other = CString::new("singles").unwrap();
    let mut t2 = std::ptr::null_mut();
    // SAFETY: a live session, valid name/schema/options/out.
    let st =
        unsafe { tsra_table_begin(session, other.as_ptr(), &schema, &table_opts(10), &mut t2) };
    expect(st, TsraStatus::Ok);
    expect(push(t2, 0, &batch(0, 10)), TsraStatus::Ok);
    expect(end(t2), TsraStatus::Ok);
    let out = dir.path().join("p.tsra");
    expect(seal(session, &out).0, TsraStatus::Ok);
    let reader = Reader::open(&out).unwrap();
    assert_eq!(reader.block_names(), vec!["events", "singles"]);
}

#[test]
fn a_hash_buffer_too_small_is_refused_after_the_seal() {
    let dir = tempfile::tempdir().unwrap();
    let session = open(&dir.path().join("stage"), 1);
    let path = CString::new(dir.path().join("p.tsra").to_str().unwrap()).unwrap();
    let mut tiny = [0 as std::ffi::c_char; 4];
    // SAFETY: a live session, a valid path and a 4-byte buffer.
    let st = unsafe { tsra_session_seal(session, path.as_ptr(), tiny.as_mut_ptr(), tiny.len()) };
    expect(st, TsraStatus::InvalidArgument);
    assert_eq!(tiny, [0; 4], "nothing is written into a buffer too small");
    assert!(dir.path().join("p.tsra").exists());
}

/// Hazard H4 (ADR-0056 §5), characterised: the bridge, like the native writer, seals a float's
/// bits as given, so two NaNs with different payloads seal two different products. A producer
/// whose NaNs come from arithmetic (x86-64 and AArch64 disagree on the sign of `0.0 / 0.0`) would
/// seal per-architecture bytes unless something canonicalises them first.
#[test]
fn nan_payloads_are_sealed_bit_for_bit() {
    let hash_with = |nan_bits: u32| {
        let dir = tempfile::tempdir().unwrap();
        let session = open(&dir.path().join("stage"), 1);
        let field = Field::new("x", DataType::Float32, false);
        let schema =
            FFI_ArrowSchema::try_from(DataType::Struct(Fields::from(vec![field.clone()]))).unwrap();
        let (st, table) = begin(session, &schema, 3);
        expect(st, TsraStatus::Ok);
        let values = arrow_array::Float32Array::from(vec![1.0, f32::from_bits(nan_bits), 2.0]);
        let batch = struct_of(vec![(field, Arc::new(values) as ArrayRef)]);
        expect(push(table, 0, &batch), TsraStatus::Ok);
        expect(end(table), TsraStatus::Ok);
        let (st, hash) = seal(session, &dir.path().join("p.tsra"));
        expect(st, TsraStatus::Ok);
        hash
    };
    let quiet = hash_with(0x7fc0_0000); // the canonical quiet NaN
    let negative = hash_with(0xffc0_0000); // x86-64's default NaN: the sign bit set
    assert_eq!(quiet, hash_with(0x7fc0_0000));
    assert_ne!(quiet, negative, "NaN payload bits reach content_hash");
}

/// A seal that fails still releases the session: with a NULL path its unknown-row-count staging
/// goes at once; with a table open the seal is refused (`STATE`), and ending the table afterwards
/// stops the table's thread and removes the staging.
#[test]
fn a_failed_seal_releases_the_session() {
    let schema = FFI_ArrowSchema::try_from(DataType::Struct(fields())).unwrap();
    let dir = tempfile::tempdir().unwrap();

    let stage = dir.path().join("a");
    let session = open(&stage, 1);
    let (_, table) = begin(session, &schema, TSRA_ROWS_UNKNOWN);
    expect(push(table, 0, &batch(0, 10)), TsraStatus::Ok);
    expect(end(table), TsraStatus::Ok);
    // SAFETY: a live session; a NULL path is refused, and the session is released.
    let st = unsafe { tsra_session_seal(session, std::ptr::null(), std::ptr::null_mut(), 0) };
    expect(st, TsraStatus::InvalidArgument);
    assert!(
        !stage.join("tables").exists(),
        "the table staging outlived the session"
    );

    let stage = dir.path().join("b");
    let session = open(&stage, 1);
    let (_, table) = begin(session, &schema, TSRA_ROWS_UNKNOWN);
    // More than one row-group, so the sink has spilled a fragment into the staging.
    expect(push(table, 0, &batch(0, 70_000)), TsraStatus::Ok);
    while !stage.join("tables").exists() {
        std::thread::yield_now(); // the table thread spills asynchronously
    }
    let (st, _) = seal(session, &dir.path().join("b.tsra"));
    expect(st, TsraStatus::State);
    assert!(last_error().unwrap().contains("still open"));
    assert!(
        stage.join("tables").exists(),
        "the open table's staging is still in use"
    );
    expect(end(table), TsraStatus::Ok);
    assert!(
        !stage.join("tables").exists(),
        "ending the orphaned table removes the staging"
    );
    assert!(!dir.path().join("b.tsra").exists());
}
