//! Regression test for #356 — the encode trace event must reach a capturing subscriber even when
//! another thread registered the callsite first.
//!
//! **This lives in its own test binary on purpose.** `tracing` caches a callsite's `Interest` once
//! per process, so the outcome depends on which thread reaches the callsite first. Inside the lib's
//! unit-test binary any earlier test that calls `encode` registers it before this test runs, and the
//! assertion then passes whether or not the bug is present — a false green (verified: with the fix
//! removed, the full `--lib` suite is green at `--test-threads=1`). A dedicated binary contains
//! exactly one test, so this is the process's first hit and the interleaving below is the only one
//! that can happen.
//!
//! The mechanism, and why `test_support::init` fixes it, is documented on
//! `tessera_io::test_support`.

use std::io;
use std::sync::{Arc, Mutex};
use std::thread;

use tessera_core::block::array::ArraySpec;
use tessera_io::{encode, ArrayData};
use tracing_subscriber::fmt::MakeWriter;

/// A local `MakeWriter` over a shared buffer. The lib has an equivalent in its `#[cfg(test)]`
/// `test_trace` module, which a separate test binary cannot reach, and `tracing-subscriber` is a
/// dev-dependency so the helper cannot move into the library either.
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

fn spec() -> ArraySpec {
    let mut s = ArraySpec::new(vec![8, 8, 8], "int16");
    s.codec = "pcodec".into();
    s
}

fn data() -> ArrayData {
    ArrayData::I16((0..512).map(|k| (k % 97) as i16).collect())
}

#[test]
fn encode_trace_survives_a_concurrent_first_hit_from_an_unsubscribed_thread() {
    tessera_io::test_support::init();

    let buf = Arc::new(Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::fmt()
        .with_writer(CaptureWriter(buf.clone()))
        .with_max_level(tracing::Level::DEBUG)
        .with_ansi(false)
        .finish();

    tracing::subscriber::with_default(subscriber, || {
        // The callsite's first-ever hit happens on a thread with no subscriber of its own, while
        // this thread holds a capturing one. Without the interest floor the verdict cached here is
        // `never` (the registering thread's default is `NoSubscriber`), and the encode below then
        // emits nothing.
        thread::spawn(|| encode(&spec(), &data()).unwrap())
            .join()
            .unwrap();
        encode(&spec(), &data()).unwrap();
    });

    let log = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
    assert!(
        log.contains("encoded array block"),
        "a concurrent unsubscribed first hit disabled the callsite: {log}"
    );
}
