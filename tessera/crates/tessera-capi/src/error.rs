//! The error model: every entry point returns a [`TsraStatus`], and the message of the last failure
//! on the calling thread is readable through [`crate::tsra_last_error`].
//!
//! No Rust panic crosses the boundary. Every `extern "C"` function runs its body through [`guard`],
//! which catches an unwind and reports it as [`TsraStatus::Panic`]. (Since Rust 1.81 an unwind out of
//! an `extern "C"` function aborts the process instead of being undefined behaviour, so without the
//! guard a bug would kill the C++ producer; with it, the producer gets a status and a message.) This
//! holds only when the library is built with `panic = "unwind"`, the default; a `panic = "abort"`
//! build turns every panic into an abort, guard or not.

use std::cell::RefCell;
use std::ffi::{c_char, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};

/// The status every `tsra_*` call returns. `TSRA_STATUS_OK` is zero; every other value is a failure
/// whose message `tsra_last_error()` returns on the same thread.
///
/// New codes may be added in a minor ABI version, so a caller must treat an unknown non-zero value
/// as a failure.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TsraStatus {
    /// Success.
    Ok = 0,
    /// A NULL pointer where one is required, a string that is not UTF-8, an options struct of an
    /// unknown size, or a value out of range.
    InvalidArgument = 1,
    /// The Arrow schema or batch does not fit the table: an unsupported type, a nullable column of a
    /// type that can't be nullable, a batch whose fields differ from the declared ones, or a NULL in a
    /// column declared non-nullable.
    Schema = 2,
    /// A batch sequence number was already pushed, or `tsra_table_end` found a gap.
    Order = 3,
    /// A call in the wrong state: a second table while one is open, a seal with a table open, or any
    /// call on a session an earlier failure poisoned.
    State = 4,
    /// The writer failed (I/O, encode, or the declared row count was not met). The message is the
    /// writer's error.
    Writer = 5,
    /// A Rust panic was caught at the boundary. This is a bug in the library; the session is poisoned.
    Panic = 6,
}

/// A failure: the status to return and the message to keep for [`crate::tsra_last_error`].
#[derive(Debug)]
pub struct CapiError {
    pub status: TsraStatus,
    pub message: String,
}

impl CapiError {
    pub fn new(status: TsraStatus, message: impl Into<String>) -> Self {
        CapiError {
            status,
            message: message.into(),
        }
    }
}

impl From<tessera_core::Error> for CapiError {
    fn from(e: tessera_core::Error) -> Self {
        CapiError::new(TsraStatus::Writer, e.to_string())
    }
}

pub type CapiResult<T> = Result<T, CapiError>;

thread_local! {
    /// The message of the last failure on this thread; cleared by every successful call.
    static LAST_ERROR: RefCell<Option<CString>> = const { RefCell::new(None) };
}

fn set_last_error(message: Option<&str>) {
    // A message with an interior NUL can't be a C string; replace the NULs rather than lose it.
    let c = message.map(|m| CString::new(m.replace('\0', "\u{fffd}")).expect("NULs replaced"));
    LAST_ERROR.with(|slot| *slot.borrow_mut() = c);
}

/// The last failure's message on this thread, or NULL after a success. The pointer stays valid until
/// the next `tsra_*` call on this thread.
pub(crate) fn last_error_ptr() -> *const c_char {
    LAST_ERROR.with(|slot| {
        slot.borrow()
            .as_ref()
            .map_or(std::ptr::null(), |c| c.as_ptr())
    })
}

/// Run one entry point's body: a panic becomes [`TsraStatus::Panic`], an error its status, and the
/// thread's last-error message is set (or cleared on success).
pub(crate) fn guard(body: impl FnOnce() -> CapiResult<()>) -> TsraStatus {
    let outcome = catch_unwind(AssertUnwindSafe(body)).unwrap_or_else(|payload| {
        let what = payload
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "a panic with a non-string payload".into());
        Err(CapiError::new(
            TsraStatus::Panic,
            format!("tessera-capi: internal panic (a bug, please report it): {what}"),
        ))
    });
    match outcome {
        Ok(()) => {
            set_last_error(None);
            TsraStatus::Ok
        }
        Err(e) => {
            set_last_error(Some(&e.message));
            e.status
        }
    }
}
