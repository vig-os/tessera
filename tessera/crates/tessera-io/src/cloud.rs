//! Cloud range-read (#225 SPIKE — feature `cloud`).
//!
//! Premise to prove: a sealed `.tsra` is fully **prune-before-fetch** over S3 — opening + reading
//! one block of an 8-block product touches well under the whole archive, not because the local code
//! got lucky, but because the wire actually fetched fewer bytes. [`ObjectStoreReader`] adapts an
//! `object_store::ObjectStore` to the `Read + Seek` source the existing [`crate::Reader`] already
//! accepts (via [`crate::Reader::from_reader`]) — so the same range-read code path that serves a
//! local file in `range.rs` serves an S3 bucket here, with no public surface change in the read API.
//!
//! `object_store/aws` is reqwest-backed, so an async runtime is non-negotiable; we hold ONE
//! current-thread Tokio runtime per reader (no globals, no `lazy_static`) and `block_on` each
//! range fetch — the inline `unit` test below cross-checks the assertions against a real local
//! MinIO when the flake check provides one, and `eprintln!`-skips otherwise.

#![cfg(feature = "cloud")]
// The struct + `new` are only constructed from the in-module unit test (kept crate-internal — the
// spike must NOT widen the public read-API surface). Without the allow, a non-test build under the
// `cloud` feature trips `dead_code` (the gate runs `clippy -D warnings`).
#![allow(dead_code)]

use std::io::{Error as IoError, ErrorKind, Read, Result as IoResult, Seek, SeekFrom};
use std::sync::Arc;

use object_store::path::Path as ObjPath;
use object_store::{ObjectStore, ObjectStoreExt};
use tokio::runtime::{Builder, Runtime};

/// Adapts an `object_store::ObjectStore` into a synchronous `Read + Seek` source so the existing
/// [`crate::Reader::from_reader`] code path serves an S3 object as-is. One per-reader Tokio
/// current-thread runtime drives reqwest under the hood.
pub(crate) struct ObjectStoreReader {
    store: Arc<dyn ObjectStore>,
    path: ObjPath,
    len: u64,
    pos: u64,
    rt: Runtime,
}

impl ObjectStoreReader {
    /// `HEAD` the object once to learn its length, then return a reader positioned at byte 0.
    pub(crate) fn new(store: Arc<dyn ObjectStore>, path: ObjPath) -> IoResult<Self> {
        let rt = Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| IoError::other(format!("tokio runtime build: {e}")))?;
        let meta = rt
            .block_on(store.head(&path))
            .map_err(|e| IoError::other(format!("object_store head({path}): {e}")))?;
        Ok(ObjectStoreReader {
            store,
            path,
            len: meta.size,
            pos: 0,
            rt,
        })
    }

    /// Total object length learned from the initial `HEAD`. Used by tests.
    #[cfg(test)]
    pub(crate) fn len(&self) -> u64 {
        self.len
    }
}

impl Read for ObjectStoreReader {
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
        if buf.is_empty() || self.pos >= self.len {
            return Ok(0);
        }
        let want = u64::try_from(buf.len()).map_err(IoError::other)?;
        let end = self.pos.saturating_add(want).min(self.len);
        let range = self.pos..end;
        let bytes = self
            .rt
            .block_on(self.store.get_range(&self.path, range))
            .map_err(|e| IoError::other(format!("object_store get_range: {e}")))?;
        let n = bytes.len();
        buf[..n].copy_from_slice(&bytes);
        self.pos = self
            .pos
            .checked_add(u64::try_from(n).map_err(IoError::other)?)
            .ok_or_else(|| IoError::other("position overflow"))?;
        Ok(n)
    }
}

impl Seek for ObjectStoreReader {
    fn seek(&mut self, pos: SeekFrom) -> IoResult<u64> {
        // Match `std::io::Cursor`/`File`: SeekFrom::End/Current may go past EOF (`zip` seeks +30
        // past the local file header on the EOCD scan); only a negative absolute is an error.
        let new_pos: i128 = match pos {
            SeekFrom::Start(n) => i128::from(n),
            SeekFrom::End(d) => i128::from(self.len) + i128::from(d),
            SeekFrom::Current(d) => i128::from(self.pos) + i128::from(d),
        };
        if new_pos < 0 {
            return Err(IoError::new(
                ErrorKind::InvalidInput,
                "seek to negative position",
            ));
        }
        let new_u64 = u64::try_from(new_pos)
            .map_err(|_| IoError::new(ErrorKind::InvalidInput, "seek position exceeds u64::MAX"))?;
        self.pos = new_u64;
        Ok(self.pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::array::{self, ArrayData};
    use crate::range::CountingReader;
    use crate::{pack, Reader};
    use bytes::Bytes;
    use object_store::aws::AmazonS3Builder;
    use object_store::PutPayload;
    use std::io::Cursor;
    use std::sync::atomic::Ordering;
    use tessera_core::block::array::ArraySpec;
    use tessera_core::ProductBuilder;

    /// Same pattern as `range::tests::multiblock_tsra` (range.rs:72-99) — N high-entropy int32
    /// blocks so the byte ratio is meaningful. Returns the sealed .tsra bytes and block names.
    fn multiblock_tsra(n_blocks: usize, elems: usize) -> (Vec<u8>, Vec<String>) {
        let mut bb = ProductBuilder::new("recon", "cloudtest", "d", "2024-01-01T00:00:00Z");
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

    /// Range-read an 8-block .tsra straight from MinIO — mirrors `range.rs:101-129` over the wire.
    /// Skipped when `TESSERA_S3_ENDPOINT` is unset so local `cargo test --features cloud` without a
    /// running MinIO doesn't fail; the flake check `minio-range-read` is the authoritative runner.
    #[test]
    fn s3_range_read_does_not_fetch_whole_archive() {
        let endpoint = match std::env::var("TESSERA_S3_ENDPOINT") {
            Ok(v) => v,
            Err(_) => {
                eprintln!("skipping: TESSERA_S3_ENDPOINT not set (run under flake check)"); // guardrails-ok: deliberate conditional-skip notice, not a debug leftover
                return;
            }
        };
        let bucket = std::env::var("TESSERA_S3_BUCKET").unwrap();
        let access_key = std::env::var("AWS_ACCESS_KEY_ID").unwrap();
        let secret_key = std::env::var("AWS_SECRET_ACCESS_KEY").unwrap();
        let region = std::env::var("AWS_REGION").unwrap_or_else(|_| "us-east-1".into());

        // MinIO speaks path-style S3 (default for AmazonS3Builder — we leave virtual-hosted off).
        // `with_allow_http(true)` is required for plain-http loopback.
        let store: Arc<dyn ObjectStore> = Arc::new(
            AmazonS3Builder::new()
                .with_endpoint(endpoint)
                .with_bucket_name(&bucket)
                .with_region(region)
                .with_access_key_id(access_key)
                .with_secret_access_key(secret_key)
                .with_allow_http(true)
                .build()
                .unwrap(),
        );

        let (bytes, names) = multiblock_tsra(8, 4096); // 8 × 16 KiB raw int32
        let total = bytes.len() as u64;

        // Build a runtime just for the upload (the reader holds its own).
        let upload_rt = Builder::new_current_thread().enable_all().build().unwrap();
        let obj_path = ObjPath::from("range-read.tsra");
        upload_rt
            .block_on(store.put(&obj_path, PutPayload::from(Bytes::from(bytes.clone()))))
            .unwrap();

        // Open via the same code path local readers use — the CountingReader sits between
        // `zip::ZipArchive` and our S3-backed `Read+Seek`, so the byte tally IS network bytes.
        let inner = ObjectStoreReader::new(Arc::clone(&store), obj_path).unwrap();
        assert_eq!(inner.len(), total);
        let counter = CountingReader::new(inner);
        let tally = counter.counter();
        let mut r = Reader::from_reader(counter).unwrap();
        let after_open = tally.load(Ordering::Relaxed);

        let block = r.read_block(&names[3]).unwrap();
        let after_read = tally.load(Ordering::Relaxed);
        assert!(!block.is_empty());

        // (a) Header + manifest parse must NOT have pulled the payloads.
        assert!(
            after_open < total,
            "open should not have fetched the whole archive: {after_open} of {total}"
        );
        // (b) Reading 1/8 blocks costs less than the other 7 combined.
        let one_block_cost = after_read - after_open;
        let others = total - (total / 8); // 7/8 of the archive, an upper bound on "the rest"
        assert!(
            one_block_cost < others,
            "one block fetch should be < combined size of the other blocks: \
             +{one_block_cost} of {total} (other-7≈{others})"
        );
        assert!(
            after_read < total / 2,
            "1/8 blocks should read < half the archive: {after_read} of {total}"
        );

        // (c) Seal survives the network path: the block bytes match a local Reader of the same
        //     fixture byte-for-byte — proves digest verification ran on the S3-sourced payload.
        let mut local = Reader::from_reader(Cursor::new(bytes)).unwrap();
        let local_block = local.read_block(&names[3]).unwrap();
        assert_eq!(block, local_block, "S3 block bytes must equal local bytes");
    }
}
