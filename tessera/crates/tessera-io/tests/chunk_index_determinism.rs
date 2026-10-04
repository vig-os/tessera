//! ADR-0059 determinism gate for the `{hash, stats}` chunk index.
//!
//! The schema rests on one property: **the sealed `.cidx` bytes are a pure function of the input**,
//! independent of worker count, build profile and architecture. M3 — the per-chunk digest over
//! native little-endian element bytes — cannot be revised after the default-on seal, so it is
//! pinned here rather than trusted.
//!
//! Three axes, covered deliberately:
//!
//!  - **architecture** and **build profile**: the pinned literals below. CI runs this file on
//!    `x86_64-linux` AND `aarch64-linux`, under the dev-profile `ingest-gate-a` check and the
//!    release-profile `workspace-test` check, so a byte difference on any of those four
//!    combinations fails the gate. #472 is why this is checked and not assumed: a persisted float
//!    NaN diverged across architectures by a single bit.
//!  - **worker count**: asserted directly, by sealing the same data through the streaming writer at
//!    1, 4 and 16 encode threads and comparing the resulting sidecar bytes.
//!  - **the encoding itself**: re-derived inside the test from first principles, so the literals
//!    cannot drift into agreeing with a wrong implementation.

use tessera_core::block::array::ArraySpec;
use tessera_core::chunk_index::ChunkIndex;
use tessera_core::hash::digest;
use tessera_io::array::{array_chunk_index, ArrayData};
use tessera_io::stream::{array_job_indexed, StreamWriter};
use tessera_io::{Reader, WriteSession};

const TS: &str = "2024-01-01T00:00:00Z";

/// A fixture with the properties that break careless encodings: negative values (two's complement),
/// a chunk grid that does NOT tile the shape (edge chunks clip), and a dtype narrower than `i64`
/// (so an `i64`-widened digest differs from a native-LE one).
fn fixture() -> (ArraySpec, ArrayData) {
    let mut spec = ArraySpec::new(vec![5, 7, 3], "int16");
    spec.chunks = vec![4, 4, 4];
    let n = 5 * 7 * 3;
    let vals: Vec<i16> = (0..n).map(|k| (k as i16 * 37) - 512).collect();
    (spec, ArrayData::I16(vals))
}

/// The encoding is re-derived here from the ADR's definition, independently of the builder.
///
/// This is the test that gives the pinned literals their meaning: a literal alone would still pass
/// if the implementation and the literal were updated together in the wrong direction. Deriving the
/// expected bytes from the spec — C-order odometer, last axis fastest, edge chunks clipped, each
/// element in its native 2-byte little-endian width — and hashing *that* proves the digest is over
/// the data's own bytes.
///
/// Cross-checked once more outside Rust entirely: the same byte sequence, generated in Python and
/// hashed with `b3sum`, gives `57df27cd…` (and the old `i64`-widened buffer gives `52403b87…`, a
/// different value, which is the evidence that M3 actually changed what is hashed).
#[test]
fn the_per_chunk_digest_is_over_native_little_endian_elements() {
    let (spec, data) = fixture();
    let idx = array_chunk_index(&spec, &data).unwrap().expect("indexable");

    let shape = [5usize, 7, 3];
    let chunks = [4usize, 4, 4];
    let strides = [21usize, 3, 1]; // C-order
    let ArrayData::I16(vals) = &data else {
        panic!("fixture is i16")
    };

    // Chunk 0 spans x,y in 0..4 and z in 0..3 (clipped by the shape).
    let hi = [
        chunks[0].min(shape[0]),
        chunks[1].min(shape[1]),
        chunks[2].min(shape[2]),
    ];
    let mut expect = Vec::new();
    for x in 0..hi[0] {
        for y in 0..hi[1] {
            for z in 0..hi[2] {
                let flat = x * strides[0] + y * strides[1] + z * strides[2];
                // NATIVE width, little-endian — two bytes for int16, not eight.
                expect.extend_from_slice(&vals[flat].to_le_bytes());
            }
        }
    }
    assert_eq!(expect.len(), 4 * 4 * 3 * 2, "48 elements, 2 bytes each");
    assert_eq!(
        idx.entries[0].digest,
        digest(&expect),
        "chunk digest must be over the chunk's NATIVE little-endian element bytes (ADR-0059 M3)"
    );

    // And it is NOT the old i64-widened encoding.
    let widened: Vec<u8> = {
        let mut w = Vec::new();
        for x in 0..hi[0] {
            for y in 0..hi[1] {
                for z in 0..hi[2] {
                    let flat = x * strides[0] + y * strides[1] + z * strides[2];
                    w.extend_from_slice(&(vals[flat] as i64).to_le_bytes());
                }
            }
        }
        w
    };
    assert_ne!(
        idx.entries[0].digest,
        digest(&widened),
        "the i64-widened buffer must no longer be what is hashed"
    );
}

/// Pinned per-chunk digests and index root — the cross-architecture / cross-profile assertion.
#[test]
fn per_chunk_digests_and_root_are_pinned() {
    let (spec, data) = fixture();
    let idx = array_chunk_index(&spec, &data).unwrap().expect("indexable");

    // ceil(5/4) * ceil(7/4) * ceil(3/4) = 2 * 2 * 1 = 4 chunks.
    assert_eq!(
        idx.len(),
        4,
        "a non-divisible grid must still tile the array"
    );

    let digests: Vec<&str> = idx.entries.iter().map(|e| e.digest.as_str()).collect();
    assert_eq!(
        digests,
        vec![
            "blake3:57df27cd32a202c5f8725531ea18a54f1f0c350b183dd8a346671bf562e9b6b2",
            "blake3:6c04f82e52363096667fa47fc8afef11af11ae1960a1dc1e1849299e693c2a72",
            "blake3:c61dd86e2857116a58b3818766073a93a20a912174c75eb308a27da4edd8ea47",
            "blake3:0f34cf844ecf5f82124a82af66180b543f3f0ee62887f7ba0cc7999cc6bbff62",
        ],
    );
    assert_eq!(
        idx.root(),
        "blake3:1da716a6fcb33adaa96653e9a678bf847c50b57b7d62804db4889afd3db82721",
    );
}

/// The whole serialized sidecar — statistics, counters and histogram — is byte-stable.
#[test]
fn serialized_index_bytes_are_pinned_and_round_trip() {
    let (spec, data) = fixture();
    let idx = array_chunk_index(&spec, &data).unwrap().expect("indexable");
    let bytes = idx.to_bytes().unwrap();

    assert_eq!(
        digest(&bytes),
        "blake3:761a1e41593bbc0e17286b6b82b70ac9c61f94ec358b30b35b4b262ba626162c",
        "the sealed .cidx bytes must be a pure function of the input"
    );

    let back = ChunkIndex::from_bytes(&bytes).unwrap();
    assert_eq!(back.entries, idx.entries);
    assert_eq!(
        back.histogram, idx.histogram,
        "the histogram must survive serialization"
    );

    // The fixture's span is small enough for one bin per value, so the histogram is EXACT and
    // describes every sample.
    let h = idx.histogram.as_ref().expect("histogram built by default");
    assert!(h.exact, "one bin per value for this span");
    assert_eq!(h.total(), 105, "every voxel counted exactly once");
    assert_eq!(h.counts.len() as i128, h.hi as i128 - h.lo as i128 + 1);
}

/// WORKER COUNT MUST NOT CHANGE A SINGLE BYTE.
///
/// The index is built per chunk and rolled up with integer monoids, so the merge is associative AND
/// commutative — order-independent by algebra rather than by a fixed fold order. This drives the
/// real streaming writer at 1, 4 and 16 encode threads and compares the sealed sidecar bytes, so the
/// claim is tested through the production path rather than asserted about the arithmetic.
#[test]
fn worker_count_does_not_change_the_sidecar_bytes() {
    let mut sealed_bytes: Vec<(usize, Vec<u8>)> = Vec::new();
    let mut content_hashes: Vec<(usize, String)> = Vec::new();

    for workers in [1usize, 4, 16] {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("s.tsra");
        let ws = WriteSession::create(&dir.path().join("stage"), "recon", "det", "determinism", TS)
            .unwrap();
        let mut sw = StreamWriter::new(ws, workers, 3);
        // Several blocks, so the committer really has concurrent work to order.
        for i in 0..6 {
            let mut spec = ArraySpec::new(vec![5, 7, 3], "int16");
            spec.chunks = vec![4, 4, 4];
            let n = 5 * 7 * 3;
            let vals: Vec<i16> = (0..n).map(|k| (k as i16 * 37) - 512 + i as i16).collect();
            sw.push(array_job_indexed(
                format!("b{i:03}"),
                spec,
                ArrayData::I16(vals),
            ))
            .unwrap();
        }
        let sealed = sw.finish(&out).unwrap();
        content_hashes.push((workers, sealed.content_hash.clone().unwrap()));

        // Pull every sidecar's bytes back out of the sealed product.
        let mut r = Reader::open(&out).unwrap();
        let names: Vec<String> = r
            .manifest()
            .blocks
            .iter()
            .filter(|b| b.name.ends_with(".cidx"))
            .map(|b| b.name.clone())
            .collect();
        assert_eq!(names.len(), 6, "every integer block gets a sidecar");
        let mut all = Vec::new();
        for n in &names {
            all.extend_from_slice(&r.read_block(n).unwrap());
        }
        sealed_bytes.push((workers, all));
    }

    let (_, reference) = &sealed_bytes[0];
    for (workers, bytes) in &sealed_bytes[1..] {
        assert_eq!(
            bytes, reference,
            "the .cidx bytes changed at {workers} workers — the index is not worker-independent"
        );
    }
    let (_, h0) = &content_hashes[0];
    for (workers, h) in &content_hashes[1..] {
        assert_eq!(
            h, h0,
            "content_hash changed at {workers} workers — the seal is not worker-independent"
        );
    }
}
