//! Cloud range-read (#225 — feature `cloud`).
//!
//! Public read surface for sealed `.tsra` products on object storage. [`ObjectStoreReader`] adapts
//! an `object_store::ObjectStore` to the synchronous `Read + Seek` source the existing
//! [`crate::Reader`] already accepts (via [`crate::Reader::from_reader`]), so the SAME range-read
//! code path that serves a local file in `range.rs` serves an S3 bucket or HTTP origin here — no
//! public-API split between local and cloud reads. [`open_url`] is the convenience entry point.
//!
//! **Tail-prefetch.** A `.tsra` is a STORED zip64 whose **central directory + EOCD live at the
//! tail**, so opening + reading the manifest entry would otherwise issue several small
//! range-GETs to scan the EOCD, then read the central directory, then read the local file header
//! and payload of the `mimetype` and `manifest.json` entries. We fetch the final
//! [`TAIL_PREFETCH`] bytes ONCE in [`ObjectStoreReader::new`] and serve any read that lies wholly
//! inside that cached suffix from memory — opening a typical product drops from many GETs to one
//! HEAD plus one tail-GET (plus payload GETs for whichever blocks the caller actually reads).
//! Verified by [`ObjectStoreReader::get_count`] in the unit tests below.
//!
//! **Runtime.** `object_store/aws` is reqwest-backed and async; we hold ONE current-thread Tokio
//! runtime per reader (no globals, no `lazy_static`) and `block_on` each fetch — the public API
//! stays sync per ADR-0002, and tokio enters only here (the legitimate read-side use per
//! ADR-0034 §4: latency-bound network I/O behind a sync surface).
//!
//! **Prune-before-fetch capstone.** With [`crate::LogicalTableView::select_blocks_overlapping`]
//! on top of the cloud reader, a query that doesn't overlap a product's stat range never fetches
//! that product's data block — proven by the `cohort_*` test below across two products in the
//! same bucket.

#![cfg(feature = "cloud")]

use std::io::{Error as IoError, ErrorKind, Read, Result as IoResult, Seek, SeekFrom};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use object_store::aws::AmazonS3Builder;
use object_store::http::HttpBuilder;
use object_store::path::Path as ObjPath;
use object_store::{ClientOptions, ObjectStore, ObjectStoreExt};
use tessera_core::{Error, Result};
use tokio::runtime::{Builder, Runtime};

use crate::container::Reader;

/// Suffix of the object eagerly fetched on [`ObjectStoreReader::new`]. Sized at 64 KiB — large
/// enough to cover the zip64 EOCD + central directory of a multi-block product (one central-dir
/// entry is ~80 B; 64 KiB ≫ central-dir for any realistic block count) plus a small manifest +
/// the trailing blocks for tiny products, so most opens reduce to 1 HEAD + 1 tail-GET.
pub const TAIL_PREFETCH: u64 = 64 * 1024;

/// Adapts an `object_store::ObjectStore` into a synchronous `Read + Seek` source so the existing
/// [`Reader::from_reader`] code path serves an S3 / HTTP object as-is. One per-reader Tokio
/// current-thread runtime drives reqwest under the hood.
pub struct ObjectStoreReader {
    store: Arc<dyn ObjectStore>,
    path: ObjPath,
    len: u64,
    pos: u64,
    /// Cached object suffix `[tail_start, len)`. The zip EOCD + central directory live here, so
    /// open + manifest-read is normally served from memory after the single tail-GET.
    tail: Vec<u8>,
    tail_start: u64,
    rt: Runtime,
    /// Shared count of `get_range` requests issued (NOT counting the HEAD or the eager tail-GET).
    /// Behind an `Arc<AtomicU64>` so test observers can clone a handle and watch the live count
    /// after the reader has been moved into a [`Reader`].
    get_count: Arc<AtomicU64>,
}

impl ObjectStoreReader {
    /// `HEAD` the object once to learn its length, eagerly fetch the final [`TAIL_PREFETCH`] bytes
    /// (or the whole object if smaller), then return a reader positioned at byte 0.
    pub fn new(store: Arc<dyn ObjectStore>, path: ObjPath) -> IoResult<Self> {
        let rt = Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| IoError::other(format!("tokio runtime build: {e}")))?;
        let meta = rt
            .block_on(store.head(&path))
            .map_err(|e| IoError::other(format!("object_store head({path}): {e}")))?;
        let len = meta.size;
        let tail_size = len.min(TAIL_PREFETCH);
        let tail_start = len - tail_size;
        let tail = if tail_size == 0 {
            Vec::new()
        } else {
            let bytes = rt
                .block_on(store.get_range(&path, tail_start..len))
                .map_err(|e| IoError::other(format!("object_store get_range tail: {e}")))?;
            bytes.to_vec()
        };
        Ok(ObjectStoreReader {
            store,
            path,
            len,
            pos: 0,
            tail,
            tail_start,
            rt,
            get_count: Arc::new(AtomicU64::new(0)),
        })
    }

    /// Total object length learned from the initial `HEAD`.
    pub fn len(&self) -> u64 {
        self.len
    }

    /// True when [`Self::len`] is zero (mirrors `Vec::is_empty` for clippy).
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Number of `get_range` GETs issued so far (excludes the HEAD + the single eager tail-GET).
    /// Drops to zero when the tail-cache serves every read; rises by one per payload GET that
    /// misses the cache. Observable from the outside via [`Self::get_counter`].
    pub fn get_count(&self) -> u64 {
        self.get_count.load(Ordering::Relaxed)
    }

    /// Shared handle to the live GET counter — clone before moving the reader into a
    /// [`Reader::from_reader`] to keep observing it.
    pub fn get_counter(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.get_count)
    }
}

impl Read for ObjectStoreReader {
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
        if buf.is_empty() || self.pos >= self.len {
            return Ok(0);
        }
        let want = u64::try_from(buf.len()).map_err(IoError::other)?;
        let end = self.pos.saturating_add(want).min(self.len);
        // Tail-cache short-circuit: if the *entire* requested range lies within the cached suffix,
        // serve from memory without a GET. A read that straddles the boundary falls through to the
        // network path — object_store's range get already handles the full request in one call,
        // so partial-overlap reads don't multiply round-trips.
        if self.pos >= self.tail_start {
            let off = usize::try_from(self.pos - self.tail_start).map_err(IoError::other)?;
            let take = usize::try_from(end - self.pos).map_err(IoError::other)?;
            buf[..take].copy_from_slice(&self.tail[off..off + take]);
            self.pos = end;
            return Ok(take);
        }
        let bytes = self
            .rt
            .block_on(self.store.get_range(&self.path, self.pos..end))
            .map_err(|e| IoError::other(format!("object_store get_range: {e}")))?;
        self.get_count.fetch_add(1, Ordering::Relaxed);
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

/// Open a sealed `.tsra` directly from object storage by URL — the public cloud-read entry point.
///
/// Supported schemes:
/// - `s3://<bucket>/<key>`  → [`AmazonS3Builder::from_env`] (`AWS_ACCESS_KEY_ID` /
///   `AWS_SECRET_ACCESS_KEY` / `AWS_REGION` / `AWS_SESSION_TOKEN`). When
///   `TESSERA_S3_ENDPOINT` is set the builder is pointed at it (for local MinIO / dev
///   stand-ins); plain-`http://` endpoints additionally enable `with_allow_http(true)`.
/// - `http://<host>/<key>`  → [`HttpBuilder`] with `allow_http=true`.
/// - `https://<host>/<key>` → [`HttpBuilder`] (TLS).
///
/// Returns a typed [`Error::Invalid`] for any other scheme so callers can branch cleanly on
/// "this isn't a cloud URL → fall back to local-path read". The reader is fully verified on open
/// (magic + manifest seal); see [`Reader::from_reader`] for the per-call guarantees.
pub fn open_url(url: &str) -> Result<Reader<ObjectStoreReader>> {
    let (store, path) = build_store_for_url(url)?;
    let inner = ObjectStoreReader::new(store, path).map_err(Error::from)?;
    Reader::from_reader(inner)
}

/// Parse a cloud URL, build the matching `object_store::ObjectStore`, and extract the object key.
/// Pure helper so callers (CLI / tests) can construct the store explicitly when they need to.
fn build_store_for_url(url: &str) -> Result<(Arc<dyn ObjectStore>, ObjPath)> {
    if let Some(rest) = url.strip_prefix("s3://") {
        let (bucket, key) = rest
            .split_once('/')
            .filter(|(b, k)| !b.is_empty() && !k.is_empty())
            .ok_or_else(|| {
                Error::Invalid(format!(
                    "cloud: s3 URL must have the form s3://<bucket>/<key>: {url:?}"
                ))
            })?;
        let mut b = AmazonS3Builder::from_env().with_bucket_name(bucket);
        if let Ok(ep) = std::env::var("TESSERA_S3_ENDPOINT") {
            let allow_http = ep.starts_with("http://");
            b = b.with_endpoint(ep);
            if allow_http {
                b = b.with_allow_http(true);
            }
        }
        let store = b
            .build()
            .map_err(|e| Error::Invalid(format!("cloud: build s3 store: {e}")))?;
        let store: Arc<dyn ObjectStore> = Arc::new(store);
        let path =
            ObjPath::parse(key).map_err(|e| Error::Invalid(format!("cloud: s3 key parse: {e}")))?;
        return Ok((store, path));
    }
    if url.starts_with("http://") || url.starts_with("https://") {
        let scheme = if url.starts_with("https://") {
            "https"
        } else {
            "http"
        };
        // Strip scheme then split host from key — HttpStore addresses paths *relative* to its
        // base, so the store must be built per-host.
        let after_scheme = &url[scheme.len() + 3..];
        let (host, key) = after_scheme
            .split_once('/')
            .filter(|(h, k)| !h.is_empty() && !k.is_empty())
            .ok_or_else(|| {
                Error::Invalid(format!(
                    "cloud: http URL must have the form {scheme}://<host>/<key>: {url:?}"
                ))
            })?;
        let base = format!("{scheme}://{host}");
        let mut b = HttpBuilder::new().with_url(base);
        if scheme == "http" {
            b = b.with_client_options(ClientOptions::new().with_allow_http(true));
        }
        let store = b
            .build()
            .map_err(|e| Error::Invalid(format!("cloud: build http store: {e}")))?;
        let store: Arc<dyn ObjectStore> = Arc::new(store);
        let path = ObjPath::parse(key)
            .map_err(|e| Error::Invalid(format!("cloud: http path parse: {e}")))?;
        return Ok((store, path));
    }
    Err(Error::Invalid(format!(
        "cloud: unsupported URL scheme (expected s3:// or http(s)://): {url:?}"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::array::{self, ArrayData};
    use crate::pack;
    use crate::range::CountingReader;
    use crate::table::{table_block_with_index, ColumnData, TableData};
    use bytes::Bytes;
    use object_store::aws::AmazonS3Builder;
    use object_store::PutPayload;
    use std::io::Cursor;
    use tessera_core::block::array::ArraySpec;
    use tessera_core::block::table::{Column, TableSpec};
    use tessera_core::ProductBuilder;

    /// `open_url` accepts only `s3://` and `http(s)://` schemes — anything else is a typed
    /// [`Error::Invalid`] the caller can branch on (fall back to a local-path open).
    #[test]
    fn open_url_rejects_unsupported_schemes_with_typed_error() {
        for url in ["file:///tmp/x.tsra", "ftp://h/x", "gs://b/x", "x"] {
            match open_url(url) {
                Err(Error::Invalid(msg)) => assert!(
                    msg.contains("unsupported URL scheme") || msg.contains("must have the form"),
                    "expected scheme/shape error for {url:?}, got {msg:?}",
                ),
                Err(other) => panic!("expected typed Invalid for {url:?}, got {other:?}"),
                Ok(_) => panic!("expected error for unsupported scheme {url:?}"),
            }
        }
        // s3 URLs missing bucket/key are also rejected (typed Invalid, not a panic on `unwrap`).
        for url in ["s3://", "s3://bucket", "s3:///key"] {
            assert!(matches!(open_url(url), Err(Error::Invalid(_))));
        }
    }

    /// Tail-prefetch end-to-end correctness, no MinIO required: serve a real `.tsra` from
    /// `object_store::memory::InMemory` and verify the GET count drops as advertised.
    ///
    /// Two regimes:
    /// 1. **Small archive (`len ≤ TAIL_PREFETCH`)** — the *entire* archive lands in the cached
    ///    tail on `new()`, so open + manifest parse + every block read costs ZERO extra GETs.
    ///    This is the cleanest possible witness that the tail cache works.
    /// 2. **Large archive (`len > TAIL_PREFETCH`)** — the tail covers the EOCD + central
    ///    directory but the front (`mimetype`, `manifest.json`, the first few blocks) lies
    ///    outside the cache. Open + manifest still issues some GETs, but the tail cache strips
    ///    out the EOCD-scan/central-dir reads that would otherwise dominate. We assert the
    ///    front-loaded read costs survive AND that the block bytes still round-trip.
    #[test]
    fn tail_prefetch_serves_small_archive_with_zero_get_ranges() {
        use object_store::memory::InMemory;

        // 2 small blocks → archive comfortably under TAIL_PREFETCH (64 KiB). After the eager
        // tail-GET in `new()`, every subsequent zip read must come from the cached buffer.
        let (bytes, names) = multiblock_tsra(2, 512);
        assert!(
            (bytes.len() as u64) <= TAIL_PREFETCH,
            "fixture too big for the small-archive regime: {} > {TAIL_PREFETCH}",
            bytes.len()
        );

        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let key = ObjPath::from("small.tsra");
        let upload_rt = Builder::new_current_thread().enable_all().build().unwrap();
        upload_rt
            .block_on(store.put(&key, PutPayload::from(Bytes::from(bytes.clone()))))
            .unwrap();

        let inner = ObjectStoreReader::new(Arc::clone(&store), key).unwrap();
        assert_eq!(inner.len(), bytes.len() as u64);
        let get_counter = inner.get_counter();
        let mut r = Reader::from_reader(inner).unwrap();
        // The entire archive is cached → open issues zero extra GETs.
        assert_eq!(
            get_counter.load(Ordering::Relaxed),
            0,
            "open() of a tail-sized archive must issue zero extra get_range (saw {})",
            get_counter.load(Ordering::Relaxed)
        );
        // …and so does reading every block.
        for name in &names {
            let _ = r.read_block(name).unwrap();
        }
        assert_eq!(
            get_counter.load(Ordering::Relaxed),
            0,
            "reading all blocks of a tail-sized archive must issue zero extra get_range \
             (saw {})",
            get_counter.load(Ordering::Relaxed)
        );

        // Bytes match the local reference reader: tail-prefetch is invisible to the verifier.
        let mut local = Reader::from_reader(Cursor::new(bytes)).unwrap();
        for name in &names {
            assert_eq!(r.read_block(name).unwrap(), local.read_block(name).unwrap());
        }
    }

    /// Large-archive regime: an archive bigger than [`TAIL_PREFETCH`] still wins by serving the
    /// EOCD/central-directory reads from the cache. We compare against a baseline reader that
    /// holds NO tail cache (built by re-pointing the same store at a temporarily-cleared cache)
    /// — actually, we just witness that the GET count stays well below "one per zip seek".
    #[test]
    fn tail_prefetch_reduces_get_count_on_a_large_archive() {
        use object_store::memory::InMemory;

        let (bytes, names) = multiblock_tsra(8, 4096); // ~140 KiB > TAIL_PREFETCH
        assert!((bytes.len() as u64) > TAIL_PREFETCH);

        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let key = ObjPath::from("big.tsra");
        let upload_rt = Builder::new_current_thread().enable_all().build().unwrap();
        upload_rt
            .block_on(store.put(&key, PutPayload::from(Bytes::from(bytes.clone()))))
            .unwrap();

        let inner = ObjectStoreReader::new(Arc::clone(&store), key).unwrap();
        let get_counter = inner.get_counter();
        let mut r = Reader::from_reader(inner).unwrap();
        let gets_after_open = get_counter.load(Ordering::Relaxed);
        // The naïve baseline (no tail cache) issues at least one GET per `Read::read` zip makes
        // — that is many tens of reads (EOCD scan + central-directory + the front entries). With
        // the cache the EOCD/central-dir reads vanish; what survives are the small reads to the
        // mimetype + manifest entries at the front (outside the cache). We give a generous floor
        // so this stays robust across zip-crate iteration patterns: well below 1 GET per entry.
        assert!(
            gets_after_open < 20,
            "tail-cached open should issue far fewer GETs than a per-zip-read baseline \
             (saw {gets_after_open} for {} entries)",
            r.manifest().blocks.len() + 2,
        );

        // Reading one block goes to the network (front-loaded blocks aren't in the tail) and
        // round-trips byte-for-byte against the local reference.
        let block = r.read_block(&names[3]).unwrap();
        let mut local = Reader::from_reader(Cursor::new(bytes)).unwrap();
        assert_eq!(block, local.read_block(&names[3]).unwrap());
    }

    /// Same fixture as `range::tests::multiblock_tsra` (range.rs:72-99) — N high-entropy int32
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

    /// Build a MinIO-backed S3 store from the env vars the flake check sets — `None` (with a
    /// printed skip notice) when `TESSERA_S3_ENDPOINT` is unset, so local
    /// `cargo test --features cloud` without a running MinIO doesn't fail. The
    /// `minio-range-read` flake check IS the authoritative runner.
    fn minio_store_or_skip(test: &str) -> Option<(Arc<dyn ObjectStore>, Runtime)> {
        let endpoint = match std::env::var("TESSERA_S3_ENDPOINT") {
            Ok(v) => v,
            Err(_) => {
                eprintln!("skipping {test}: TESSERA_S3_ENDPOINT not set (run under flake check)"); // guardrails-ok: deliberate conditional-skip notice, not a debug leftover
                return None;
            }
        };
        let bucket = std::env::var("TESSERA_S3_BUCKET").unwrap();
        let access_key = std::env::var("AWS_ACCESS_KEY_ID").unwrap();
        let secret_key = std::env::var("AWS_SECRET_ACCESS_KEY").unwrap();
        let region = std::env::var("AWS_REGION").unwrap_or_else(|_| "us-east-1".into());
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
        let rt = Builder::new_current_thread().enable_all().build().unwrap();
        Some((store, rt))
    }

    /// Range-read an 8-block .tsra straight from MinIO — mirrors `range.rs:101-129` over the wire.
    #[test]
    fn s3_range_read_does_not_fetch_whole_archive() {
        let (store, upload_rt) = match minio_store_or_skip("s3_range_read") {
            Some(s) => s,
            None => return,
        };

        let (bytes, names) = multiblock_tsra(8, 4096); // 8 × 16 KiB raw int32
        let total = bytes.len() as u64;

        let obj_path = ObjPath::from("range-read.tsra");
        upload_rt
            .block_on(store.put(&obj_path, PutPayload::from(Bytes::from(bytes.clone()))))
            .unwrap();

        // Open via the same code path local readers use — the CountingReader sits between
        // `zip::ZipArchive` and our S3-backed `Read+Seek`, so the byte tally IS network bytes.
        let inner = ObjectStoreReader::new(Arc::clone(&store), obj_path).unwrap();
        assert_eq!(inner.len(), total);
        let get_counter = inner.get_counter();
        let counter = CountingReader::new(inner);
        let tally = counter.counter();
        let mut r = Reader::from_reader(counter).unwrap();
        let after_open = tally.load(Ordering::Relaxed);
        let gets_after_open = get_counter.load(Ordering::Relaxed);

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

        // (c) **Tail-prefetch verification**: the EOCD scan + central directory lie in the cached
        //     64 KiB suffix → zero GETs for them. The few GETs that remain are the *front-of-file*
        //     mimetype magic + manifest.json, which a tail cache can't cover for this ~140 KiB
        //     archive — so a small bounded count, not 0. (The 0-GET case is the ≤ TAIL_PREFETCH
        //     archive — see `tail_prefetch_serves_small_archive_with_zero_get_ranges`.)
        assert!(
            gets_after_open < 20,
            "tail prefetch should keep open() GETs small (front mimetype+manifest only); saw {gets_after_open}"
        );

        // (d) Seal survives the network path: the block bytes match a local Reader of the same
        //     fixture byte-for-byte — proves digest verification ran on the S3-sourced payload.
        let mut local = Reader::from_reader(Cursor::new(bytes)).unwrap();
        let local_block = local.read_block(&names[3]).unwrap();
        assert_eq!(block, local_block, "S3 block bytes must equal local bytes");
    }

    /// Build a single-block listmode-style product whose `events` block + `events.cidx` sidecar
    /// pin a known `ms` range. The product seals through [`pack`] over the fused emit from
    /// [`table_block_with_index`], so [`crate::LogicalTableView::select_blocks_overlapping`]
    /// answers the range query from the sidecar — no events-block bytes touched.
    fn listmode_product_with_ms_range(name: &str, ms_lo: u32, ms_hi: u32) -> Vec<u8> {
        assert!(ms_lo < ms_hi);
        let rows = 4096usize;
        let span = ms_hi - ms_lo;
        // 4096 u64 ms values + 4096 f32 e values = ~48 KiB raw → well above the tiny manifest /
        // sidecar overhead, so the prune-fetch byte assertion is meaningful.
        let ms: Vec<u64> = (0..rows)
            .map(|k| u64::from(ms_lo) + (u64::from(span) * k as u64) / rows as u64)
            .collect();
        let e: Vec<f32> = (0..rows).map(|k| 511.0 + (k % 13) as f32).collect();
        let data: TableData = vec![
            ("ms".into(), ColumnData::U64(ms)),
            ("e".into(), ColumnData::F32(e)),
        ];
        let columns = vec![
            Column {
                name: "ms".into(),
                dtype: "u8".into(),
                codec: None,
            },
            Column {
                name: "e".into(),
                dtype: "f4".into(),
                codec: None,
            },
        ];
        let spec = TableSpec {
            columns,
            rows: rows as u64,
            row_index: Some("ms".into()),
        };
        // Fused emit: data block + chunk-index sidecar over `ms`. Stats come from the sidecar at
        // read time when (sidecar_present AND requested column == row_index), which holds here.
        let ((data_ref, data_payload), sidecar) =
            table_block_with_index("events", &spec, &data, Some("ms")).unwrap();
        let (sidecar_ref, sidecar_payload) =
            sidecar.expect("integer ms column must yield a sidecar");
        let mut b = ProductBuilder::new("listmode", name, "d", "2024-01-01T00:00:00Z");
        b.add_block_ref(data_ref);
        b.add_block_ref(sidecar_ref);
        let sealed = b.seal().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(format!("{name}.tsra"));
        pack(&sealed, &[data_payload, sidecar_payload], &path).unwrap();
        std::fs::read(&path).unwrap()
    }

    /// **Cohort prune-before-fetch capstone (#225).** Two `.tsra` products in the same MinIO
    /// prefix, each with an `events` block whose `ms` range is disjoint from the other; a query
    /// window matches product A and misses product B. Open both via [`open_url`], drive the
    /// pruner over each — and assert (a) A's events block IS fetched (the prune kept it), and
    /// (b) B's events block is NEVER fetched (the prune skipped it). This is the vision
    /// capstone: prune-before-fetch across a cohort over the wire, not just within one product.
    #[test]
    fn cohort_prune_before_fetch_skips_non_matching_product() {
        let (store, upload_rt) = match minio_store_or_skip("cohort_prune") {
            Some(s) => s,
            None => return,
        };

        // A matches the query window [50, 60]; B does not (its range starts at 10_000).
        let a_bytes = listmode_product_with_ms_range("cohort-a", 0, 1_000);
        let b_bytes = listmode_product_with_ms_range("cohort-b", 10_000, 11_000);
        // Snapshot each product's `events` block size so the no-fetch assertion can name a real
        // floor: B's bytes_read MUST stay well below B's events-block size.
        let b_events_bytes = {
            let mut local = Reader::from_reader(Cursor::new(b_bytes.clone())).unwrap();
            local.read_block("events").unwrap().len()
        };
        // Seed both into the same MinIO bucket under distinct keys.
        let a_key = ObjPath::from("cohort/a.tsra");
        let b_key = ObjPath::from("cohort/b.tsra");
        upload_rt
            .block_on(store.put(&a_key, PutPayload::from(Bytes::from(a_bytes))))
            .unwrap();
        upload_rt
            .block_on(store.put(&b_key, PutPayload::from(Bytes::from(b_bytes))))
            .unwrap();

        let (lo, hi) = (50i64, 60i64); // a query window

        // ── Product A: matches → prune keeps block 0 → we fetch events.
        let a_data_bytes_read: u64 = {
            let inner = ObjectStoreReader::new(Arc::clone(&store), a_key.clone()).unwrap();
            let counter = CountingReader::new(inner);
            let tally = counter.counter();
            let mut r = Reader::from_reader(counter).unwrap();
            let view = r.logical_table("events").unwrap();
            let kept = view
                .select_blocks_overlapping(&mut r, "ms", lo, hi)
                .unwrap();
            assert_eq!(
                kept,
                vec![0],
                "product A's ms range [0,1000) MUST overlap [{lo},{hi}]"
            );
            // Now fetch the kept block — this is the "fetch only the matching product's data" half.
            let block_name = view.block_names()[kept[0]].clone();
            let blk = r.read_block(&block_name).unwrap();
            assert!(!blk.is_empty());
            tally.load(Ordering::Relaxed)
        };

        // ── Product B: does NOT match → prune returns empty → we do NOT fetch events.
        let b_data_bytes_read: u64 = {
            let inner = ObjectStoreReader::new(Arc::clone(&store), b_key.clone()).unwrap();
            let counter = CountingReader::new(inner);
            let tally = counter.counter();
            let mut r = Reader::from_reader(counter).unwrap();
            let view = r.logical_table("events").unwrap();
            let kept = view
                .select_blocks_overlapping(&mut r, "ms", lo, hi)
                .unwrap();
            assert!(
                kept.is_empty(),
                "product B's ms range [10000,11000) MUST NOT overlap [{lo},{hi}]"
            );
            // The honest claim: we did NOT read B's events payload. Sidecar + manifest + central
            // directory reads are fine — and most should be served from the tail prefetch — but
            // the events bytes themselves (`b_events_bytes`) are off-limits.
            tally.load(Ordering::Relaxed)
        };

        // Cohort prune-before-fetch assertion: for the non-matching product, the bytes the wire
        // touched must be FAR less than the events-block payload (we only ever read manifest +
        // central directory + the small sidecar; we never read the events data block).
        assert!(
            b_data_bytes_read < b_events_bytes as u64,
            "non-matching product B fetched {b_data_bytes_read} bytes — MUST be < its events \
             block size {b_events_bytes} (we should never have pulled the events payload)"
        );
        // And the matching product A *did* pay the cost of its events block — sanity check that
        // the prune isn't trivially "skip everything".
        assert!(
            a_data_bytes_read > b_data_bytes_read,
            "matching product A ({a_data_bytes_read}B) must read more than non-match B \
             ({b_data_bytes_read}B) — A pulled events, B did not"
        );
    }
}
