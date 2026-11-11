//! **tessera-capi** — a C ABI over the `tessera-io` table writer, so C and C++ producers write
//! tessera products through tessera's own writer (epic #799, spike #800).
//!
//! This crate is a **prototype**: it answers the ADR's design questions with working code and
//! numbers (`docs/spikes/800-capi-prototype.md`); the ADR decides the shipped API.
//!
//! # Lifecycle
//!
//! ```text
//! tsra_session_open(stage_dir, &opts, &session)
//!   tsra_table_begin(session, "events", &arrow_schema, &table_opts, &table)
//!     tsra_table_push(table, seq, &arrow_array, &arrow_schema)   // any thread; see the window below
//!   tsra_table_end(table)                                        // consumes the table
//! tsra_session_seal(session, "out.tsra", hash, sizeof hash)      // consumes the session
//! ```
//!
//! Batches cross as the **Arrow C Data Interface**: a struct-typed `ArrowArray` (one child per
//! column) and its `ArrowSchema`. The table's schema is declared once at `tsra_table_begin`; see
//! [`arrow`] for the type map and why it is declared rather than inferred.
//!
//! # Ownership
//!
//! - `tsra_table_push` **takes ownership** of the `ArrowArray` whenever the pointer is non-NULL, on
//!   success and on failure alike: it moves the struct out (the caller's copy is left released, its
//!   `release` NULL) and calls the producer's `release` callback exactly once, before returning.
//! - Every `ArrowSchema` is **borrowed**: read during the call, never released, never kept.
//! - Strings are borrowed, NUL-terminated UTF-8.
//!
//! # Threads
//!
//! `tsra_table_push` may be called from any number of threads at once on one table. Everything else
//! is single-threaded per session: no two calls on one session (or on its table, other than pushes)
//! may overlap, and every push must have returned before `tsra_table_end`. One table is open per
//! session at a time.
//!
//! # Determinism
//!
//! The sealed bytes depend only on the rows in **sequence-number** order (see [`order`]): batch `seq`
//! is written after batch `seq - 1`, whichever thread pushed it and whenever. Batch sizes don't matter
//! either: the writer re-chunks rows to its fixed row-group grid.
//!
//! # Errors
//!
//! Every function returns a [`TsraStatus`] (or, for the two that can't fail, a plain value), and
//! [`tsra_last_error`] gives the thread's last failure message. No panic unwinds across the boundary
//! (see [`error`]).

pub mod arrow;
pub mod error;
pub mod order;

use std::ffi::{c_char, CStr};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use arrow_array::ffi::{from_ffi, FFI_ArrowArray, FFI_ArrowSchema};
use arrow_array::StructArray;
use arrow_schema::DataType;
use tessera_core::block::table::{Column, TableSpec};
use tessera_io::stream::StreamWriter;
use tessera_io::table::{TableData, ROWS_PER_GROUP};
use tessera_io::{TableMultiBlockSink, WriteSession};

use crate::arrow::DeclaredColumn;
pub use crate::error::TsraStatus;
use crate::error::{guard, CapiError, CapiResult};
use crate::order::Reorder;

/// The ABI major version. A header and a library with different majors are incompatible.
pub const TSRA_ABI_VERSION_MAJOR: u32 = 0;
/// The ABI minor version. A minor bump only adds (functions, status codes, options fields at the
/// end of an options struct), so a library at least as new as the header serves it.
pub const TSRA_ABI_VERSION_MINOR: u32 = 1;

/// `TsraTableOptions::rows` for a table whose row count isn't known before the first batch.
pub const TSRA_ROWS_UNKNOWN: u64 = u64::MAX;

/// The default `TsraTableOptions::reorder_window`: the keys admitted from the next one due.
pub const TSRA_DEFAULT_REORDER_WINDOW: u32 = 64;

/// Bytes `tsra_session_seal` needs for the content hash, terminating NUL included.
pub const TSRA_HASH_CAPACITY: usize = 128;

/// Session options. Set `struct_size = sizeof(TsraSessionOptions)`: the library reads only the
/// fields that size covers, which is what lets a later minor version append fields.
#[repr(C)]
pub struct TsraSessionOptions {
    /// `sizeof(TsraSessionOptions)` as the caller compiled it.
    pub struct_size: usize,
    /// The product type, e.g. `"listmode"`. Required.
    pub product: *const c_char,
    /// The product's name. Required.
    pub name: *const c_char,
    /// A human-readable description. Required (may be empty).
    pub description: *const c_char,
    /// The product's RFC 3339 timestamp. Required: the library never reads the clock, because the
    /// timestamp is part of the sealed manifest.
    pub timestamp: *const c_char,
    /// Encode worker threads; 0 means 1. The sealed bytes don't depend on it.
    pub workers: u32,
}

/// Table options; `struct_size` as for `TsraSessionOptions`.
#[repr(C)]
pub struct TsraTableOptions {
    /// `sizeof(TsraTableOptions)` as the caller compiled it.
    pub struct_size: usize,
    /// The table's row count, or `TSRA_ROWS_UNKNOWN`. A known count lets the writer encode each
    /// block as soon as it fills; an unknown one stages every row on disk and encodes at
    /// `tsra_table_end`. The sealed bytes are the same either way.
    pub rows: u64,
    /// How many keys from the next one due are admitted; a push of a key further ahead blocks until
    /// the window moves. 0 means `TSRA_DEFAULT_REORDER_WINDOW`. Blocking is deadlock-free only when
    /// keys are claimed in increasing order and each thread pushes its keys in increasing order
    /// (see `src/order.rs`); with any other assignment, set it to at least the number of batches.
    pub reorder_window: u32,
}

/// An open write session (opaque).
pub struct TsraSession {
    inner: Arc<Mutex<SessionInner>>,
    tables_dir: PathBuf,
}

/// An open table of a session (opaque).
pub struct TsraTable {
    declared: Vec<DeclaredColumn>,
    reorder: Reorder<TableData, SyncSender<TableData>>,
    feeder: Option<JoinHandle<Fed>>,
    session: Arc<Mutex<SessionInner>>,
    name: String,
}

struct SessionInner {
    writer: WriterState,
    tables: u64,
}

enum WriterState {
    Ready(StreamWriter),
    InTable(String),
    Poisoned(String),
}

/// What the feeder thread hands back: the writer, and the table's outcome (its leaf count).
type Fed = (StreamWriter, tessera_core::Result<u64>);

fn invalid(message: impl Into<String>) -> CapiError {
    CapiError::new(TsraStatus::InvalidArgument, message)
}

fn state_err(message: impl Into<String>) -> CapiError {
    CapiError::new(TsraStatus::State, message)
}

/// A required C string argument as `&str`.
///
/// # Safety
/// `ptr` is NULL or points to a NUL-terminated string that stays valid for the call.
unsafe fn arg_str<'a>(ptr: *const c_char, what: &str) -> CapiResult<&'a str> {
    if ptr.is_null() {
        return Err(invalid(format!("{what} is NULL")));
    }
    // SAFETY: non-NULL, and the caller guarantees a NUL-terminated string valid for the call.
    unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .map_err(|_| invalid(format!("{what} is not UTF-8")))
}

/// Check an options struct's `struct_size` against what this library knows: it must cover at least
/// the ABI 0.1 fields (`base`). A larger size, from a newer header, is accepted, and the fields past
/// this library's struct are ignored.
fn check_size(struct_size: usize, base: usize, what: &str) -> CapiResult<()> {
    if struct_size < base {
        return Err(invalid(format!(
            "{what}.struct_size is {struct_size}, below the {base} bytes ABI {TSRA_ABI_VERSION_MAJOR}.{TSRA_ABI_VERSION_MINOR} needs; set it to sizeof({what})"
        )));
    }
    Ok(())
}

impl TsraSession {
    /// Give the session up without sealing: drop its writer and remove its table staging.
    ///
    /// `StreamWriter` has no abort: dropping it closes its input and **detaches** its threads, which
    /// drain what they hold into `stage_dir/session` after this returns. A job whose fragments go with
    /// the table staging fails on its detached thread, which is harmless for a product nobody seals.
    /// With a table still open, the writer is the table's, so its staging stays until the caller ends
    /// the table, which it still must do to stop the table's thread.
    fn abandon(&self) {
        let mut inner = SessionInner::lock(&self.inner);
        let table_open = matches!(inner.writer, WriterState::InTable(_));
        drop(inner.take_ready(WriterState::Poisoned("the session was given up".into())));
        drop(inner);
        if !table_open {
            self.remove_tables_dir();
        }
    }

    /// Remove the staging of the session's unknown-row-count tables. Only once the writer is
    /// finished or dropped: until then an encode job may still be reading a fragment there.
    fn remove_tables_dir(&self) {
        let _ = std::fs::remove_dir_all(&self.tables_dir);
    }
}

impl SessionInner {
    fn lock(inner: &Mutex<SessionInner>) -> std::sync::MutexGuard<'_, SessionInner> {
        inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn take_ready(&mut self, then: WriterState) -> CapiResult<StreamWriter> {
        match std::mem::replace(&mut self.writer, then) {
            WriterState::Ready(w) => Ok(w),
            other => {
                let err = match &other {
                    WriterState::InTable(name) => state_err(format!(
                        "table '{name}' is still open; tsra_table_end it first"
                    )),
                    WriterState::Poisoned(why) => {
                        state_err(format!("the session failed earlier: {why}"))
                    }
                    WriterState::Ready(_) => unreachable!("matched above"),
                };
                self.writer = other;
                Err(err)
            }
        }
    }
}

/// The ABI version of this library: `major << 16 | minor`. Compare its major with
/// `TSRA_ABI_VERSION_MAJOR` from the header you compiled against.
#[no_mangle]
pub extern "C" fn tsra_abi_version() -> u32 {
    (TSRA_ABI_VERSION_MAJOR << 16) | TSRA_ABI_VERSION_MINOR
}

/// The message of the last failed `tsra_*` call on this thread, or NULL if the last call succeeded.
/// Valid until the next `tsra_*` call on this thread; copy it to keep it.
#[no_mangle]
pub extern "C" fn tsra_last_error() -> *const c_char {
    error::last_error_ptr()
}

/// Open a write session staging under `stage_dir` (created if absent). The library owns its
/// contents until the session is sealed, or, after `tsra_session_free` or a failed seal, until the
/// writer's detached threads finish draining (there is no abort yet, #810). On success `*out` is
/// the session; on failure it is left NULL.
///
/// # Safety
/// `stage_dir` and the strings in `opts` are NUL-terminated; `opts` and `out` are valid pointers.
#[no_mangle]
pub unsafe extern "C" fn tsra_session_open(
    stage_dir: *const c_char,
    opts: *const TsraSessionOptions,
    out: *mut *mut TsraSession,
) -> TsraStatus {
    guard(|| {
        if out.is_null() {
            return Err(invalid("out is NULL"));
        }
        // SAFETY: `out` is non-NULL and the caller guarantees it is valid for a write.
        unsafe { out.write(std::ptr::null_mut()) };
        if opts.is_null() {
            return Err(invalid("opts is NULL"));
        }
        // SAFETY: `opts` is non-NULL, and the caller guarantees it points to a TsraSessionOptions.
        // Only `struct_size` is read before it is checked to cover the rest.
        let opts = unsafe { &*opts };
        check_size(
            opts.struct_size,
            std::mem::size_of::<TsraSessionOptions>(),
            "TsraSessionOptions",
        )?;
        // SAFETY: the caller guarantees every string argument is NUL-terminated (or NULL, refused).
        let (stage, product, name, description, timestamp) = unsafe {
            (
                arg_str(stage_dir, "stage_dir")?,
                arg_str(opts.product, "opts.product")?,
                arg_str(opts.name, "opts.name")?,
                arg_str(opts.description, "opts.description")?,
                arg_str(opts.timestamp, "opts.timestamp")?,
            )
        };
        let stage = Path::new(stage);
        let session = WriteSession::create(
            &stage.join("session"),
            product,
            name,
            description,
            timestamp,
        )?;
        let writer = StreamWriter::new(session, opts.workers.max(1) as usize, 4);
        let handle = Box::new(TsraSession {
            inner: Arc::new(Mutex::new(SessionInner {
                writer: WriterState::Ready(writer),
                tables: 0,
            })),
            tables_dir: stage.join("tables"),
        });
        // SAFETY: checked non-NULL above.
        unsafe { out.write(Box::into_raw(handle)) };
        Ok(())
    })
}

/// Begin the table `name`, declared by `schema` (a struct schema, one child per column; borrowed).
/// On success `*out` is the table and the session is busy until `tsra_table_end`.
///
/// # Safety
/// `session` is a live session; `name` is NUL-terminated; `schema` is a valid, unreleased
/// `ArrowSchema`; `opts` and `out` are valid pointers.
#[no_mangle]
pub unsafe extern "C" fn tsra_table_begin(
    session: *mut TsraSession,
    name: *const c_char,
    schema: *const FFI_ArrowSchema,
    opts: *const TsraTableOptions,
    out: *mut *mut TsraTable,
) -> TsraStatus {
    guard(|| {
        if out.is_null() {
            return Err(invalid("out is NULL"));
        }
        // SAFETY: `out` is non-NULL and the caller guarantees it is valid for a write.
        unsafe { out.write(std::ptr::null_mut()) };
        if session.is_null() || schema.is_null() || opts.is_null() {
            return Err(invalid("session, schema and opts must not be NULL"));
        }
        // SAFETY: non-NULL; the caller guarantees a live session, a valid unreleased schema (read
        // only, during this call) and a TsraTableOptions of at least `struct_size` bytes.
        let (session, schema, opts) = unsafe { (&*session, &*schema, &*opts) };
        check_size(
            opts.struct_size,
            std::mem::size_of::<TsraTableOptions>(),
            "TsraTableOptions",
        )?;
        // SAFETY: the caller guarantees `name` is NUL-terminated.
        let name = unsafe { arg_str(name, "name")? }.to_owned();
        let declared_type = DataType::try_from(schema).map_err(|e| {
            CapiError::new(TsraStatus::Schema, format!("unreadable ArrowSchema: {e}"))
        })?;
        let declared = arrow::declare(&declared_type)?;
        let columns: Vec<Column> = declared.iter().map(|d| d.column.clone()).collect();
        let rows = (opts.rows != TSRA_ROWS_UNKNOWN).then_some(opts.rows);
        let window = match opts.reorder_window {
            0 => TSRA_DEFAULT_REORDER_WINDOW,
            w => w,
        };

        let mut inner = SessionInner::lock(&session.inner);
        let writer = inner.take_ready(WriterState::InTable(name.clone()))?;
        let stage = session.tables_dir.join(format!("{:04}", inner.tables));
        inner.tables += 1;
        drop(inner);

        // The channel holds two batches: the reorder buffer is where early batches wait.
        let (tx, rx) = sync_channel::<TableData>(2);
        let feeder = {
            let name = name.clone();
            std::thread::Builder::new()
                .name(format!("tsra-table-{name}"))
                .spawn(move || feed(writer, name, columns, rows, stage, rx))
                .map_err(|e| {
                    CapiError::new(TsraStatus::Writer, format!("spawn the table thread: {e}"))
                })
        };
        let feeder = match feeder {
            Ok(f) => f,
            Err(e) => {
                // The writer moved into the closure that failed to spawn, and is gone.
                SessionInner::lock(&session.inner).writer =
                    WriterState::Poisoned(e.message.clone());
                return Err(e);
            }
        };
        let table = Box::new(TsraTable {
            declared,
            reorder: Reorder::new(tx, u64::from(window)),
            feeder: Some(feeder),
            session: Arc::clone(&session.inner),
            name,
        });
        // SAFETY: checked non-NULL above.
        unsafe { out.write(Box::into_raw(table)) };
        Ok(())
    })
}

impl TsraTable {
    /// How many pushes waited for the reorder window (Rust-side observability, not part of the C
    /// ABI; the tests use it to prove the backpressure path ran).
    #[doc(hidden)]
    pub fn reorder_waits(&self) -> u64 {
        self.reorder.waits()
    }
}

/// The table thread: run the writer over the batches the reorder buffer releases, in order, then
/// hand the writer back.
fn feed(
    mut writer: StreamWriter,
    name: String,
    columns: Vec<Column>,
    rows: Option<u64>,
    stage: PathBuf,
    rx: std::sync::mpsc::Receiver<TableData>,
) -> Fed {
    let outcome = match rows {
        Some(rows) => {
            let spec = TableSpec {
                columns,
                rows,
                row_index: None,
                partition: None,
            };
            // `push_table` takes groups of at most ROWS_PER_GROUP rows; a producer's batch may be
            // larger, so split it. The writer re-chunks to its grid regardless.
            writer.push_table(name, spec, rx.into_iter().flat_map(split_groups))
        }
        None => (|| {
            let mut sink = TableMultiBlockSink::new(columns, name, &stage, &mut writer)?;
            for batch in rx {
                sink.push(batch)?;
            }
            sink.finish()
        })(),
    };
    // The sink's fragments stay in `stage` until the encode jobs have read them, which only the
    // seal waits for; `tsra_session_seal` / `tsra_session_free` remove the tables directory.
    (writer, outcome)
}

/// `batch` as consecutive groups of at most [`ROWS_PER_GROUP`] rows.
fn split_groups(batch: TableData) -> Vec<TableData> {
    let rows = batch.first().map_or(0, |(_, c)| c.len());
    if rows <= ROWS_PER_GROUP {
        return vec![batch];
    }
    (0..rows)
        .step_by(ROWS_PER_GROUP)
        .map(|start| {
            let end = (start + ROWS_PER_GROUP).min(rows);
            batch
                .iter()
                .map(|(n, c)| (n.clone(), c.slice(start, end)))
                .collect()
        })
        .collect()
}

/// Push batch `seq` (0, 1, 2, … across the whole table) of `table`. `array` is a struct-typed
/// `ArrowArray` matching the declared schema, and `schema` its `ArrowSchema` (borrowed).
///
/// Takes ownership of `*array` whenever `array` is non-NULL, on failure too, and releases it before
/// returning. Safe to call from many threads at once; blocks while `seq` is at or past
/// `next + reorder_window` (`next` being the next key due), which is deadlock-free only under the
/// precondition `TsraTableOptions::reorder_window` states.
///
/// # Safety
/// `table` is a live table; `array` points to an `ArrowArray` the caller owns; `schema` is a valid,
/// unreleased `ArrowSchema` describing it.
#[no_mangle]
pub unsafe extern "C" fn tsra_table_push(
    table: *mut TsraTable,
    seq: u64,
    array: *mut FFI_ArrowArray,
    schema: *const FFI_ArrowSchema,
) -> TsraStatus {
    guard(|| {
        if array.is_null() {
            return Err(invalid("array is NULL"));
        }
        // SAFETY: non-NULL, and the caller owns a valid ArrowArray there. `from_raw` moves it out
        // and leaves the caller's struct released, so ownership is ours from here on and dropping
        // `array` calls the producer's release callback, on every path below.
        let array = unsafe { FFI_ArrowArray::from_raw(array) };
        if array.is_released() {
            return Err(invalid("array is already released"));
        }
        if table.is_null() || schema.is_null() {
            return Err(invalid("table and schema must not be NULL"));
        }
        // SAFETY: non-NULL; the caller guarantees a live table (shared: pushes may overlap, and
        // TsraTable's push path is `&self` + interior locking) and a valid unreleased schema.
        let (table, schema) = unsafe { (&*table, &*schema) };
        // SAFETY: `array` is an owned, unreleased ArrowArray and `schema` describes it (caller
        // contract). `from_ffi` trusts the layout, so the result is validated in full right below.
        let data = unsafe { from_ffi(array, schema) }.map_err(|e| {
            CapiError::new(TsraStatus::Schema, format!("unreadable ArrowArray: {e}"))
        })?;
        data.validate_full().map_err(|e| {
            CapiError::new(
                TsraStatus::Schema,
                format!("the ArrowArray is malformed: {e}"),
            )
        })?;
        if !matches!(data.data_type(), DataType::Struct(_)) {
            return Err(CapiError::new(
                TsraStatus::Schema,
                format!(
                    "a batch must be a struct array (format \"+s\"); got {}",
                    data.data_type()
                ),
            ));
        }
        let batch = StructArray::from(data);
        let rows = arrow::to_table_data(&table.declared, &batch)?;
        drop(batch); // releases the producer's buffers before we block in the reorder buffer
        table.reorder.push(seq, rows)
    })
}

/// End `table`: wait until every batch is written, and give the session back. Consumes `table`
/// (it is freed whatever the outcome). A gap in the sequence numbers, a row count short of the
/// declared one, or any writer failure poisons the session.
///
/// # Safety
/// `table` is a live table with no push in progress; it must not be used afterwards.
#[no_mangle]
pub unsafe extern "C" fn tsra_table_end(table: *mut TsraTable) -> TsraStatus {
    guard(|| {
        if table.is_null() {
            return Err(invalid("table is NULL"));
        }
        // SAFETY: the caller hands over a live table it no longer uses; it is freed here.
        let mut table = unsafe { Box::from_raw(table) };
        // Close the input: dropping the sender (here, or with the gap error) ends the writer's input.
        let closed = table.reorder.close().map(|(_, tx)| drop(tx));
        let joined = table
            .feeder
            .take()
            .expect("a live table has its feeder")
            .join();
        let mut inner = SessionInner::lock(&table.session);
        let (writer, outcome) = match joined {
            Ok(fed) => fed,
            Err(_) => {
                let why = format!("the writer thread of table '{}' panicked", table.name);
                inner.writer = WriterState::Poisoned(why.clone());
                return Err(CapiError::new(TsraStatus::Panic, why));
            }
        };
        let result = closed.and(outcome.map(|_| ()).map_err(CapiError::from));
        inner.writer = match &result {
            Ok(()) => WriterState::Ready(writer),
            Err(e) => {
                drop(writer);
                WriterState::Poisoned(format!("table '{}': {}", table.name, e.message))
            }
        };
        result
    })
}

/// Seal the session's product to `out_path` and write its content hash (NUL-terminated) into
/// `hash_out` (`hash_cap` bytes; `TSRA_HASH_CAPACITY` is enough). `hash_out` may be NULL. Consumes
/// `session` (freed whatever the outcome).
///
/// # Safety
/// `session` is a live session with no open table; `out_path` is NUL-terminated; `hash_out` is NULL
/// or valid for `hash_cap` bytes.
#[no_mangle]
pub unsafe extern "C" fn tsra_session_seal(
    session: *mut TsraSession,
    out_path: *const c_char,
    hash_out: *mut c_char,
    hash_cap: usize,
) -> TsraStatus {
    guard(|| {
        if session.is_null() {
            return Err(invalid("session is NULL"));
        }
        // SAFETY: the caller hands over a live session it no longer uses; it is freed here.
        let session = unsafe { Box::from_raw(session) };
        // SAFETY: the caller guarantees `out_path` is NUL-terminated.
        let out_path = match unsafe { arg_str(out_path, "out_path") } {
            Ok(p) => p,
            Err(e) => {
                session.abandon();
                return Err(e);
            }
        };
        // Bound first, so the lock guard is released before `abandon` takes it again.
        let taken = SessionInner::lock(&session.inner)
            .take_ready(WriterState::Poisoned("the session was sealed".into()));
        let writer = match taken {
            Ok(w) => w,
            Err(e) => {
                session.abandon();
                return Err(e);
            }
        };
        let sealed = writer.finish(Path::new(out_path));
        session.remove_tables_dir();
        let hash = sealed?.content_hash.ok_or_else(|| {
            CapiError::new(
                TsraStatus::Writer,
                "the sealed manifest carries no content hash",
            )
        })?;
        if !hash_out.is_null() {
            if hash.len() + 1 > hash_cap {
                return Err(invalid(format!(
                    "the product was sealed, but hash_cap {hash_cap} can't hold its {}-byte hash",
                    hash.len() + 1
                )));
            }
            // SAFETY: `hash_out` is valid for `hash_cap` bytes and the hash plus its NUL fit.
            unsafe {
                std::ptr::copy_nonoverlapping(hash.as_ptr().cast(), hash_out, hash.len());
                hash_out.add(hash.len()).write(0);
            }
        }
        Ok(())
    })
}

/// Free `session` without sealing; its staged data is abandoned. NULL is a no-op.
///
/// # Safety
/// `session` is NULL or a live session that is not used afterwards.
#[no_mangle]
pub unsafe extern "C" fn tsra_session_free(session: *mut TsraSession) {
    let _ = guard(|| {
        if !session.is_null() {
            // SAFETY: the caller hands over a live session it no longer uses.
            unsafe { Box::from_raw(session) }.abandon();
        }
        Ok(())
    });
}
