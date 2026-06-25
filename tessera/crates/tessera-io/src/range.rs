//! Range-read validation (ROADMAP P2 finish / S6, #196).
//!
//! The cloud premise of the `.tsra` container (ADR-0022): because every block is STORED
//! (uncompressed) and indexed by the zip central directory, a reader fetches the central directory
//! plus only the block byte-ranges it needs — never the whole archive. [`Reader`] is already generic
//! over any `Read + Seek` source ([`Reader::from_reader`]), which is exactly the seam an
//! `object_store`/HTTP ranged-GET backend plugs into; the same code path serves a local file today.
//!
//! [`CountingReader`] wraps any seekable source and tallies the bytes actually read (a proxy for
//! cloud GET volume), so we can *prove* the property: opening a multi-block product and reading one
//! block touches far fewer bytes than the whole archive.
//!
//! [`Reader`]: crate::Reader
//! [`Reader::from_reader`]: crate::Reader::from_reader

use std::io::{Read, Result as IoResult, Seek, SeekFrom};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// A `Read + Seek` wrapper that tallies the bytes actually read from the inner source. The tally
/// lives behind a shared [`Arc`] so it can be observed after the reader has been moved into a
/// [`Reader`].
pub struct CountingReader<R> {
    inner: R,
    counter: Arc<AtomicU64>,
}

impl<R> CountingReader<R> {
    pub fn new(inner: R) -> Self {
        CountingReader {
            inner,
            counter: Arc::new(AtomicU64::new(0)),
        }
    }

    /// A shared handle to the running byte tally — clone it before moving the reader away.
    pub fn counter(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.counter)
    }

    /// Total bytes read from the inner source so far.
    pub fn bytes_read(&self) -> u64 {
        self.counter.load(Ordering::Relaxed)
    }
}

impl<R: Read> Read for CountingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
        let n = self.inner.read(buf)?;
        self.counter.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

impl<R: Seek> Seek for CountingReader<R> {
    fn seek(&mut self, pos: SeekFrom) -> IoResult<u64> {
        self.inner.seek(pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::array::{self, ArrayData};
    use crate::{pack, Reader};
    use std::io::Cursor;
    use tessera_core::block::array::ArraySpec;
    use tessera_core::ProductBuilder;

    /// Pack an N-block product into in-memory `.tsra` bytes. Each block holds high-entropy data (a
    /// LCG) so pcodec cannot shrink it to near-nothing — making the range-read byte ratio meaningful.
    fn multiblock_tsra(n_blocks: usize, elems: usize) -> (Vec<u8>, Vec<String>) {
        let mut bb = ProductBuilder::new("recon", "rangetest", "d", "2024-01-01T00:00:00Z");
        let mut payloads = Vec::new();
        let mut names = Vec::new();
        for b in 0..n_blocks {
            let mut spec = ArraySpec::new(vec![elems as u64], "int32");
            spec.codec = "pcodec".into();
            let mut state = (b as u32).wrapping_mul(2_654_435_761).wrapping_add(1);
            let data = ArrayData::I32(
                (0..elems)
                    .map(|_| {
                        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                        state as i32
                    })
                    .collect(),
            );
            let name = format!("blk{b}");
            let (r, pl) = array::array_block(&name, &spec, &data).unwrap();
            bb.add_block_ref(r);
            payloads.push(pl);
            names.push(name);
        }
        let sealed = bb.seal().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("m.tsra");
        pack(&sealed, &payloads, &path).unwrap();
        (std::fs::read(&path).unwrap(), names)
    }

    #[test]
    fn reading_one_block_does_not_read_the_whole_archive() {
        let (bytes, names) = multiblock_tsra(8, 4096); // 8 blocks × 16 KiB raw int32
        let total = bytes.len() as u64;

        let counter = CountingReader::new(Cursor::new(bytes));
        let tally = counter.counter();
        // Opening verifies magic + manifest seal — reads the central directory, mimetype, and
        // manifest.json, but NOT any block payload (the seal is recomputed from recorded digests).
        let mut r = Reader::from_reader(counter).unwrap();
        let after_open = tally.load(Ordering::Relaxed);

        // Read exactly one of the eight blocks.
        let block = r.read_block(&names[3]).unwrap();
        let after_read = tally.load(Ordering::Relaxed);
        assert!(!block.is_empty());

        // Range-read, not scan: one of eight sizeable blocks must cost well under the whole archive,
        // and reading that block adds roughly one block — not the other seven.
        assert!(
            after_read < total / 2,
            "1/8 blocks should read < half the archive: {after_read} of {total}"
        );
        assert!(
            after_read - after_open < total / 4,
            "reading one block must not scan the archive: +{} of {total}",
            after_read - after_open
        );
    }
}
