//! Shared tracing setup + event capture for this crate's tests.
//!
//! # Why a process-wide subscriber is required (#356)
//!
//! `tracing` caches each callsite's [`Interest`] process-wide the first time it is
//! reached, and that registration consults only the **global** default dispatcher —
//! never a thread-local one installed by [`tracing::subscriber::with_default`].
//!
//! A test binary installs no global subscriber, so the global default is
//! `NoSubscriber`, which reports `Interest::never`. Whichever thread reaches a
//! callsite first therefore latches `never` for the whole process, and a capturing
//! test running concurrently on another thread receives nothing — its subscriber is
//! live and its level filter permits the event, but the callsite was already ruled
//! out. Under `cargo test` that is a race against every other test that exercises
//! the same code path.
//!
//! [`init`] closes it by installing a global default that records nothing but keeps
//! every callsite registered as `sometimes`-interesting at `TRACE`. The cached
//! verdict can then never be `never`, and `sometimes` (rather than `always`) means
//! each event is still dispatched through the `enabled()` of whichever subscriber is
//! current on the emitting thread — so a test's own level filter keeps working and
//! threads without a subscriber stay silent.

use std::io;
use std::sync::{Arc, Mutex, Once};

use tracing::level_filters::LevelFilter;
use tracing::span;
use tracing::subscriber::Interest;
use tracing::{Event, Id, Metadata, Subscriber};
use tracing_subscriber::fmt::MakeWriter;

/// Records nothing; exists only to keep callsite interest permissive process-wide.
struct InterestFloor;

impl Subscriber for InterestFloor {
    /// `sometimes`, never `always`: an `always` verdict would let events bypass the
    /// current subscriber's `enabled()` and defeat each test's level filter.
    fn register_callsite(&self, _: &Metadata<'_>) -> Interest {
        Interest::sometimes()
    }

    /// Keeps the global max-level at `TRACE` so a `debug!`/`trace!` callsite is not
    /// short-circuited by the level check that precedes the interest lookup.
    fn max_level_hint(&self) -> Option<LevelFilter> {
        Some(LevelFilter::TRACE)
    }

    /// This subscriber itself is never interested — a thread with no capturing
    /// subscriber of its own stays silent.
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        false
    }

    fn new_span(&self, _: &span::Attributes<'_>) -> Id {
        Id::from_u64(1)
    }
    fn record(&self, _: &Id, _: &span::Record<'_>) {}
    fn record_follows_from(&self, _: &Id, _: &Id) {}
    fn event(&self, _: &Event<'_>) {}
    fn enter(&self, _: &Id) {}
    fn exit(&self, _: &Id) {}
}

static INIT: Once = Once::new();

/// Install the process-wide interest floor. Idempotent, and safe to call from any
/// thread; [`capture`] calls it, so tests normally do not need to.
pub(crate) fn init() {
    INIT.call_once(|| {
        // Errors only if a global default is already installed, which is equally fine.
        let _ = tracing::subscriber::set_global_default(InterestFloor);
    });
}

/// A `MakeWriter` over a shared buffer — captures tracing fmt output for assertions.
#[derive(Clone)]
pub(crate) struct CaptureWriter(Arc<Mutex<Vec<u8>>>);

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
/// Installs the [`init`] interest floor first, so the captured output does not depend
/// on which test happened to touch a callsite first.
pub(crate) fn capture<T>(level: tracing::Level, f: impl FnOnce() -> T) -> String {
    init();
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
