//! Event capture for this crate's unit tests.
//!
//! The process-wide setup this depends on, and why it is needed, lives in
//! [`crate::test_support`]. [`capture`] installs it, so a test that goes through this helper cannot
//! forget it — which is the reason no test should hand-roll
//! [`tracing::subscriber::with_default`] plus its own writer.

use std::io;
use std::sync::{Arc, Mutex};

use tracing_subscriber::fmt::MakeWriter;

/// A `MakeWriter` over a shared buffer — captures tracing fmt output for assertions.
#[derive(Clone)]
struct CaptureWriter(Arc<Mutex<Vec<u8>>>);

impl io::Write for CaptureWriter {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for CaptureWriter {
    type Writer = CaptureWriter;
    fn make_writer(&'a self) -> CaptureWriter {
        self.clone()
    }
}

/// Capture, as text, every event at `level` or above that `f` emits on this thread.
///
/// Installs [`crate::test_support::init`] first, so the captured output cannot depend on which test
/// happened to reach a callsite first.
pub(crate) fn capture<T>(level: tracing::Level, f: impl FnOnce() -> T) -> String {
    crate::test_support::init();
    let buf = Arc::new(Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::fmt()
        .with_writer(CaptureWriter(buf.clone()))
        .with_max_level(level)
        .with_ansi(false)
        .finish();
    tracing::subscriber::with_default(subscriber, f);
    let bytes = buf.lock().unwrap().clone();
    String::from_utf8(bytes).unwrap()
}
