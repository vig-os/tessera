//! Bounded-memory streaming write pipeline (ROADMAP P3 / #203) — the DAQ-facing front of
//! [`WriteSession`].
//!
//! The DAQ rule (S17): never encode on the producer's hot path. [`StreamWriter`] decouples the
//! producer from encoding with a bounded pipeline:
//!
//! ```text
//!   push(job) ──▶ bounded channel ──▶ N encode workers ──▶ ordered committer ──▶ WriteSession
//!   (producer)    (backpressure =      (parallel encode)    (reorder by seq,      (durable frag +
//!                  RAM cap)                                   append in push order) journal + Merkle)
//! ```
//!
//! - **Bounded RAM:** the input channel has a fixed capacity; `push` blocks when full, so memory is
//!   capped at ~`capacity` in-flight blocks regardless of how fast the producer runs.
//! - **Parallel encode:** `workers` threads encode concurrently (the expensive pcodec/Vortex step).
//! - **Deterministic output:** the committer commits blocks in **push order** (a reorder buffer holds
//!   out-of-order completions), so the running Merkle — and therefore the sealed `content_hash` /
//!   `id` / `manifest_hash` — is **byte-identical to a batch writer** that added the same blocks in
//!   the same order. Streaming changes *how* the bytes are produced, never *which* bytes.
//!
//! The >RAM single-huge-block case (chunked Vortex compaction, which would change bytes) is out of
//! scope here and tracked in ADR-0026.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::thread::{self, JoinHandle};

use tessera_core::block::BlockRef;
use tessera_core::manifest::Manifest;
use tessera_core::{Error, Result};

use crate::write::WriteSession;
use crate::BlockPayload;

/// A unit of work the pipeline encodes off the hot path. The producer hands the *raw* data captured
/// in a closure; the encode (pcodec/Vortex) runs on a worker thread. Use [`array_job`]/[`table_job`].
pub type EncodeJob = Box<dyn FnOnce() -> Result<(BlockRef, BlockPayload)> + Send + 'static>;

/// Wrap an array block as an [`EncodeJob`] (encoding deferred to a worker).
pub fn array_job(
    name: impl Into<String>,
    spec: tessera_core::block::array::ArraySpec,
    data: crate::array::ArrayData,
) -> EncodeJob {
    let name = name.into();
    Box::new(move || crate::array::array_block(&name, &spec, &data))
}

/// Wrap a table block as an [`EncodeJob`].
pub fn table_job(
    name: impl Into<String>,
    spec: tessera_core::block::table::TableSpec,
    data: crate::table::TableData,
) -> EncodeJob {
    let name = name.into();
    Box::new(move || crate::table::table_block(&name, &spec, &data))
}

type Encoded = (usize, Result<(BlockRef, BlockPayload)>);

/// A bounded-memory, parallel-encode streaming writer over a [`WriteSession`].
pub struct StreamWriter {
    input: Option<SyncSender<(usize, EncodeJob)>>,
    workers: Vec<JoinHandle<()>>,
    committer: Option<JoinHandle<Result<WriteSession>>>,
    seq: usize,
}

impl StreamWriter {
    /// Build a pipeline over `session` with `workers` encode threads and an in-flight `capacity`
    /// (the backpressure / RAM bound, in blocks). Both are clamped to ≥ 1.
    pub fn new(session: WriteSession, workers: usize, capacity: usize) -> Self {
        let workers = workers.max(1);
        let capacity = capacity.max(1);
        let (in_tx, in_rx) = sync_channel::<(usize, EncodeJob)>(capacity);
        let in_rx = std::sync::Arc::new(std::sync::Mutex::new(in_rx));
        let (out_tx, out_rx) = sync_channel::<Encoded>(capacity);

        let worker_handles = (0..workers)
            .map(|_| {
                let rx = std::sync::Arc::clone(&in_rx);
                let tx = out_tx.clone();
                thread::spawn(move || loop {
                    // Hold the lock only to dequeue, then encode without it (true parallelism).
                    let job = {
                        let guard = rx.lock().expect("input mutex");
                        guard.recv()
                    };
                    match job {
                        Ok((seq, job)) => {
                            if tx.send((seq, job())).is_err() {
                                break; // committer gone
                            }
                        }
                        Err(_) => break, // input closed + drained
                    }
                })
            })
            .collect();
        drop(out_tx); // only workers hold senders now → out_rx closes when all workers exit

        let committer = thread::spawn(move || commit_loop(session, out_rx));

        StreamWriter {
            input: Some(in_tx),
            workers: worker_handles,
            committer: Some(committer),
            seq: 0,
        }
    }

    /// Enqueue a block for encode+commit. Blocks (backpressure) when `capacity` blocks are in flight.
    /// Blocks are committed in call order, so push in the order you want the manifest to record them.
    pub fn push(&mut self, job: EncodeJob) -> Result<()> {
        let tx = self
            .input
            .as_ref()
            .ok_or_else(|| Error::Container("stream writer already finished".into()))?;
        tx.send((self.seq, job)).map_err(|_| {
            Error::Container("stream pipeline closed (a worker/committer died)".into())
        })?;
        self.seq += 1;
        Ok(())
    }

    /// Drain the pipeline, then compact + seal to `out`. Returns the sealed manifest (identical to a
    /// batch seal of the same blocks in push order).
    pub fn finish(mut self, out: &Path) -> Result<Manifest> {
        drop(self.input.take()); // close input → workers drain + exit → out_rx closes → committer returns
        for w in self.workers.drain(..) {
            w.join()
                .map_err(|_| Error::Container("encode worker panicked".into()))?;
        }
        let session = self
            .committer
            .take()
            .expect("committer present")
            .join()
            .map_err(|_| Error::Container("committer thread panicked".into()))??;
        session.seal(out)
    }
}

/// The single committer: receives encoded blocks, commits them to the session in `seq` order (a
/// reorder buffer holds completions that arrive early), and returns the session for sealing.
fn commit_loop(mut session: WriteSession, out_rx: Receiver<Encoded>) -> Result<WriteSession> {
    let mut pending: BTreeMap<usize, (BlockRef, BlockPayload)> = BTreeMap::new();
    let mut next = 0usize;
    while let Ok((seq, res)) = out_rx.recv() {
        pending.insert(seq, res?);
        while let Some((bref, payload)) = pending.remove(&next) {
            session.append_block(bref, &payload.bytes)?;
            next += 1;
        }
    }
    // Channel closed: all workers exited → every pushed block has arrived → drain the contiguous rest.
    while let Some((bref, payload)) = pending.remove(&next) {
        session.append_block(bref, &payload.bytes)?;
        next += 1;
    }
    Ok(session)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::array::{self, ArrayData};
    use crate::Reader;
    use tessera_core::block::array::ArraySpec;
    use tessera_core::ProductBuilder;

    const TS: &str = "2024-01-01T00:00:00Z";

    fn ablock(name: &str, n: i16) -> (BlockRef, ArraySpec, ArrayData) {
        let mut spec = ArraySpec::new(vec![8, 8, 8], "int16");
        spec.codec = "pcodec".into();
        let data = ArrayData::I16((0..512).map(|k| (k as i16).wrapping_mul(3) + n).collect());
        let (r, _p) = array::array_block(name, &spec, &data).unwrap();
        (r, spec, data)
    }

    #[test]
    fn streamed_equals_batch_for_many_blocks() {
        let dir = tempfile::tempdir().unwrap();
        let n = 50;
        let blocks: Vec<_> = (0..n)
            .map(|i| ablock(&format!("b{i:03}"), i as i16))
            .collect();

        // streamed: small capacity + several workers → forces reorder + backpressure
        let ws = WriteSession::create(&dir.path().join("stage"), "recon", "p", "d", TS).unwrap();
        let mut sw = StreamWriter::new(ws, 4, 3);
        for (i, (_, spec, data)) in blocks.iter().enumerate() {
            sw.push(array_job(format!("b{i:03}"), spec.clone(), data.clone()))
                .unwrap();
        }
        let out = dir.path().join("stream.tsra");
        let sealed = sw.finish(&out).unwrap();

        // batch equivalent in the same (push) order
        let mut bb = ProductBuilder::new("recon", "p", "d", TS);
        for (r, _, _) in &blocks {
            bb.add_block_ref(r.clone());
        }
        let batch = bb.seal().unwrap();
        assert_eq!(sealed.id, batch.id);
        assert_eq!(sealed.content_hash, batch.content_hash);
        assert_eq!(
            sealed.manifest_hash, batch.manifest_hash,
            "streamed seal != batch"
        );

        let mut rdr = Reader::open(&out).unwrap();
        assert_eq!(rdr.block_names().len(), n);
        for nm in rdr.block_names() {
            rdr.read_block(&nm).unwrap();
        }
    }
}
