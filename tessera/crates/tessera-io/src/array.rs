//! Array block backend — dense N-D arrays as a deterministic single-blob Zarr v3 store
//! (zarrs + **pcodec**). This is the real codec behind an [`ArraySpec`] block: it turns a typed
//! buffer of samples into the exact bytes stored at `blocks/<name>` in a `.tsra`, and back.
//!
//! ## Why a serialized store, not loose Zarr files
//! A Zarr array is a *set* of keys (`zarr.json` + chunk objects). The `.tsra` container stores one
//! payload per block, so we serialize the whole store into ONE deterministic blob: keys sorted,
//! each framed `len|key|len|value`. Reconstructing the store and decoding is lossless, and because
//! the framing is canonical and pcodec + the Zarr metadata are themselves deterministic, the same
//! array always encodes to byte-identical bytes — the writer-determinism release gate. The blob is
//! a faithful Zarr store, so the exploded form can later materialize it as real OME-Zarr files.
//!
//! ## Dtypes
//! pcodec operates on ≥16-bit numbers, which covers every dtype real imaging volumes use (int16
//! CT/PET, float32 µ-maps/SUV, uint16/32 counts). The eight pcodec-native dtypes are supported via
//! [`ArrayData`]; 8-bit (`int8`/`uint8`) and `float16` are intentionally out of scope for the
//! pcodec backend (a zstd fallback can be added later without changing the container contract).

use std::ops::Range;
use std::sync::Arc;

use bytes::Bytes;
use tessera_core::block::array::ArraySpec;
use tessera_core::block::{BlockKind, BlockRef};
use tessera_core::{Error, Result};
use zarrs::array::builder::ArrayBuilderFillValue;
use zarrs::array::codec::array_to_bytes::pcodec::{PcodecCodec, PcodecCodecConfiguration};
use zarrs::array::{data_type, Array, ArrayBuilder, DataType, Element, ElementOwned};
use zarrs::storage::store::MemoryStore;
use zarrs::storage::{ReadableWritableListableStorage, StoreKey};

use crate::BlockPayload;

/// The single array's node path inside the (block-private) Zarr store.
const ARRAY_PATH: &str = "/array";

/// A typed, in-memory array buffer — the decoded form of an array block's samples, in C
/// (row-major) order matching the spec's `shape`/`axes`. One variant per pcodec-native dtype.
#[derive(Debug, Clone, PartialEq)]
pub enum ArrayData {
    I16(Vec<i16>),
    I32(Vec<i32>),
    I64(Vec<i64>),
    U16(Vec<u16>),
    U32(Vec<u32>),
    U64(Vec<u64>),
    F32(Vec<f32>),
    F64(Vec<f64>),
}

impl ArrayData {
    /// The Tessera dtype name (matches [`tessera_core::dtype::DType::as_str`]).
    pub fn dtype(&self) -> &'static str {
        match self {
            ArrayData::I16(_) => "int16",
            ArrayData::I32(_) => "int32",
            ArrayData::I64(_) => "int64",
            ArrayData::U16(_) => "uint16",
            ArrayData::U32(_) => "uint32",
            ArrayData::U64(_) => "uint64",
            ArrayData::F32(_) => "float32",
            ArrayData::F64(_) => "float64",
        }
    }

    /// Number of samples held.
    pub fn len(&self) -> usize {
        match self {
            ArrayData::I16(v) => v.len(),
            ArrayData::I32(v) => v.len(),
            ArrayData::I64(v) => v.len(),
            ArrayData::U16(v) => v.len(),
            ArrayData::U32(v) => v.len(),
            ArrayData::U64(v) => v.len(),
            ArrayData::F32(v) => v.len(),
            ArrayData::F64(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The numpy-style dtype code (no byte-order prefix), matching the fd5 table codes
    /// (`i2/i4/i8`, `u2/u4/u8`, `f4/f8`). Pair with little-endian [`Self::to_le_bytes`].
    pub fn numpy_code(&self) -> &'static str {
        match self {
            ArrayData::I16(_) => "i2",
            ArrayData::I32(_) => "i4",
            ArrayData::I64(_) => "i8",
            ArrayData::U16(_) => "u2",
            ArrayData::U32(_) => "u4",
            ArrayData::U64(_) => "u8",
            ArrayData::F32(_) => "f4",
            ArrayData::F64(_) => "f8",
        }
    }

    /// Flatten the samples to little-endian bytes (C-order) for zero-copy reconstruction in another
    /// runtime — e.g. `numpy.frombuffer(buf, "<" + numpy_code).reshape(shape)`.
    pub fn to_le_bytes(&self) -> Vec<u8> {
        match self {
            ArrayData::I16(v) => le_bytes(v, i16::to_le_bytes),
            ArrayData::I32(v) => le_bytes(v, i32::to_le_bytes),
            ArrayData::I64(v) => le_bytes(v, i64::to_le_bytes),
            ArrayData::U16(v) => le_bytes(v, u16::to_le_bytes),
            ArrayData::U32(v) => le_bytes(v, u32::to_le_bytes),
            ArrayData::U64(v) => le_bytes(v, u64::to_le_bytes),
            ArrayData::F32(v) => le_bytes(v, f32::to_le_bytes),
            ArrayData::F64(v) => le_bytes(v, f64::to_le_bytes),
        }
    }
}

/// Pack a typed slice into little-endian bytes via the element's `to_le_bytes`.
pub(crate) fn le_bytes<T: Copy, const N: usize>(v: &[T], f: fn(T) -> [u8; N]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * N);
    for &x in v {
        out.extend_from_slice(&f(x));
    }
    out
}

/// Inverse of [`le_bytes`] — parse a little-endian byte buffer into a typed `Vec` (errors if the
/// length isn't a multiple of the element width).
pub(crate) fn from_le<T, const N: usize>(bytes: &[u8], f: fn([u8; N]) -> T) -> Result<Vec<T>> {
    if !bytes.len().is_multiple_of(N) {
        return Err(Error::Codec(format!(
            "buffer of {} bytes is not a multiple of the {N}-byte element width",
            bytes.len()
        )));
    }
    Ok(bytes
        .chunks_exact(N)
        .map(|c| f(c.try_into().unwrap()))
        .collect())
}

impl ArrayData {
    /// Build an [`ArrayData`] from a little-endian buffer + numpy code (`i2/u4/f4/…`) — the inverse
    /// of [`Self::to_le_bytes`] + [`Self::numpy_code`]. Used by writers that receive raw buffers
    /// (e.g. the Python bindings packing a numpy array).
    pub fn from_le_bytes(numpy_code: &str, bytes: &[u8]) -> Result<ArrayData> {
        Ok(match numpy_code {
            "i2" => ArrayData::I16(from_le(bytes, i16::from_le_bytes)?),
            "i4" => ArrayData::I32(from_le(bytes, i32::from_le_bytes)?),
            "i8" => ArrayData::I64(from_le(bytes, i64::from_le_bytes)?),
            "u2" => ArrayData::U16(from_le(bytes, u16::from_le_bytes)?),
            "u4" => ArrayData::U32(from_le(bytes, u32::from_le_bytes)?),
            "u8" => ArrayData::U64(from_le(bytes, u64::from_le_bytes)?),
            "f4" => ArrayData::F32(from_le(bytes, f32::from_le_bytes)?),
            "f8" => ArrayData::F64(from_le(bytes, f64::from_le_bytes)?),
            other => {
                return Err(Error::Codec(format!(
                    "unsupported array dtype code '{other}' (pcodec backend needs ≥16-bit: i2/i4/i8, u2/u4/u8, f4/f8)"
                )))
            }
        })
    }
}

fn ze(e: impl std::fmt::Display) -> Error {
    Error::Codec(e.to_string())
}

fn pcodec() -> Arc<PcodecCodec> {
    // Default pcodec configuration: deterministic, lossless, auto delta/mode detection.
    let cfg = PcodecCodecConfiguration::default();
    Arc::new(PcodecCodec::new_with_configuration(&cfg).expect("default pcodec config is valid"))
}

/// Serialize an entire Zarr store into one deterministic blob: keys sorted, `u32 len | key | u64
/// len | value`. Sorting + fixed framing makes the bytes a pure function of the store contents.
fn serialize_store(store: &ReadableWritableListableStorage) -> Result<Vec<u8>> {
    let mut keys: Vec<StoreKey> = store.list().map_err(ze)?.into_iter().collect();
    keys.sort();
    let mut out = Vec::new();
    for k in &keys {
        let ks = k.as_str().as_bytes();
        let v = store
            .get(k)
            .map_err(ze)?
            .ok_or_else(|| Error::Codec(format!("listed key '{}' had no value", k.as_str())))?;
        out.extend_from_slice(&(ks.len() as u32).to_le_bytes());
        out.extend_from_slice(ks);
        out.extend_from_slice(&(v.len() as u64).to_le_bytes());
        out.extend_from_slice(&v);
    }
    Ok(out)
}

/// Inverse of [`serialize_store`] — rebuild an in-memory store from the framed blob.
fn deserialize_store(blob: &[u8]) -> Result<ReadableWritableListableStorage> {
    let store: ReadableWritableListableStorage = Arc::new(MemoryStore::new());
    let mut i = 0usize;
    let bad = || Error::Codec("truncated array store blob".into());
    while i < blob.len() {
        let klen =
            u32::from_le_bytes(blob.get(i..i + 4).ok_or_else(bad)?.try_into().unwrap()) as usize;
        i += 4;
        let key = std::str::from_utf8(blob.get(i..i + klen).ok_or_else(bad)?)
            .map_err(ze)?
            .to_string();
        i += klen;
        let vlen =
            u64::from_le_bytes(blob.get(i..i + 8).ok_or_else(bad)?.try_into().unwrap()) as usize;
        i += 8;
        let val = Bytes::copy_from_slice(blob.get(i..i + vlen).ok_or_else(bad)?);
        i += vlen;
        store
            .set(&StoreKey::new(key).map_err(ze)?, val)
            .map_err(ze)?;
    }
    Ok(store)
}

/// Build a pcodec-coded Zarr array in `store` and write `data` into it (the whole array).
fn build_filled<T: Element, F: Into<ArrayBuilderFillValue>>(
    store: &ReadableWritableListableStorage,
    spec: &ArraySpec,
    dt: DataType,
    fill: F,
    data: &[T],
) -> Result<()> {
    let array = ArrayBuilder::new(spec.shape.clone(), spec.chunks.clone(), dt, fill)
        .array_to_bytes_codec(pcodec())
        .build(store.clone(), ARRAY_PATH)
        .map_err(ze)?;
    array.store_metadata().map_err(ze)?;
    array
        .store_array_subset(&array.subset_all(), data)
        .map_err(ze)?;
    Ok(())
}

fn retrieve_all<T: ElementOwned>(store: ReadableWritableListableStorage) -> Result<Vec<T>> {
    let array = Array::open(store, ARRAY_PATH).map_err(ze)?;
    array
        .retrieve_array_subset::<Vec<T>>(&array.subset_all())
        .map_err(ze)
}

fn retrieve_region<T: ElementOwned>(
    store: ReadableWritableListableStorage,
    start: &[u64],
    shape: &[u64],
) -> Result<Vec<T>> {
    let array = Array::open(store, ARRAY_PATH).map_err(ze)?;
    let ranges: Vec<Range<u64>> = start
        .iter()
        .zip(shape)
        .map(|(s, n)| *s..(*s + *n))
        .collect();
    array
        .retrieve_array_subset::<Vec<T>>(&ranges.as_slice())
        .map_err(ze)
}

/// Validate that `data` is encodable under `spec` (codec, dtype, and element count all agree).
fn validate(spec: &ArraySpec, data: &ArrayData) -> Result<()> {
    spec.validate()?;
    if spec.codec != "pcodec" {
        return Err(Error::Codec(format!(
            "array codec '{}' is not supported by the pcodec backend",
            spec.codec
        )));
    }
    if spec.dtype != data.dtype() {
        return Err(Error::Codec(format!(
            "array data dtype '{}' != spec dtype '{}'",
            data.dtype(),
            spec.dtype
        )));
    }
    let expected: u64 = spec.shape.iter().product();
    if data.len() as u64 != expected {
        return Err(Error::Codec(format!(
            "array data length {} != shape product {}",
            data.len(),
            expected
        )));
    }
    Ok(())
}

/// Encode `data` into the deterministic array-block payload bytes for `spec`.
pub fn encode(spec: &ArraySpec, data: &ArrayData) -> Result<Vec<u8>> {
    validate(spec, data)?;
    let store: ReadableWritableListableStorage = Arc::new(MemoryStore::new());
    match data {
        ArrayData::I16(v) => build_filled(&store, spec, data_type::int16(), 0i16, v)?,
        ArrayData::I32(v) => build_filled(&store, spec, data_type::int32(), 0i32, v)?,
        ArrayData::I64(v) => build_filled(&store, spec, data_type::int64(), 0i64, v)?,
        ArrayData::U16(v) => build_filled(&store, spec, data_type::uint16(), 0u16, v)?,
        ArrayData::U32(v) => build_filled(&store, spec, data_type::uint32(), 0u32, v)?,
        ArrayData::U64(v) => build_filled(&store, spec, data_type::uint64(), 0u64, v)?,
        ArrayData::F32(v) => build_filled(&store, spec, data_type::float32(), 0.0f32, v)?,
        ArrayData::F64(v) => build_filled(&store, spec, data_type::float64(), 0.0f64, v)?,
    }
    serialize_store(&store)
}

/// Decode the whole array from a block payload (inverse of [`encode`]).
pub fn decode(spec: &ArraySpec, blob: &[u8]) -> Result<ArrayData> {
    let store = deserialize_store(blob)?;
    Ok(match spec.dtype.as_str() {
        "int16" => ArrayData::I16(retrieve_all(store)?),
        "int32" => ArrayData::I32(retrieve_all(store)?),
        "int64" => ArrayData::I64(retrieve_all(store)?),
        "uint16" => ArrayData::U16(retrieve_all(store)?),
        "uint32" => ArrayData::U32(retrieve_all(store)?),
        "uint64" => ArrayData::U64(retrieve_all(store)?),
        "float32" => ArrayData::F32(retrieve_all(store)?),
        "float64" => ArrayData::F64(retrieve_all(store)?),
        other => {
            return Err(Error::Codec(format!(
                "array decode: dtype '{other}' is not supported by the pcodec backend"
            )))
        }
    })
}

/// Decode a rectangular sub-region (ROI / orthogonal slab) without materializing the whole array.
/// `start` and `shape` are per-axis (C order); the result is that region in row-major order. Only
/// the intersecting chunks are decoded — the orthogonal/ROI-read value of cubic chunking.
pub fn decode_subset(
    spec: &ArraySpec,
    blob: &[u8],
    start: &[u64],
    shape: &[u64],
) -> Result<ArrayData> {
    if start.len() != spec.shape.len() || shape.len() != spec.shape.len() {
        return Err(Error::Codec(format!(
            "subset rank {}/{} != array rank {}",
            start.len(),
            shape.len(),
            spec.shape.len()
        )));
    }
    let store = deserialize_store(blob)?;
    Ok(match spec.dtype.as_str() {
        "int16" => ArrayData::I16(retrieve_region(store, start, shape)?),
        "int32" => ArrayData::I32(retrieve_region(store, start, shape)?),
        "int64" => ArrayData::I64(retrieve_region(store, start, shape)?),
        "uint16" => ArrayData::U16(retrieve_region(store, start, shape)?),
        "uint32" => ArrayData::U32(retrieve_region(store, start, shape)?),
        "uint64" => ArrayData::U64(retrieve_region(store, start, shape)?),
        "float32" => ArrayData::F32(retrieve_region(store, start, shape)?),
        "float64" => ArrayData::F64(retrieve_region(store, start, shape)?),
        other => {
            return Err(Error::Codec(format!(
                "array decode: dtype '{other}' is not supported by the pcodec backend"
            )))
        }
    })
}

/// Encode an array block and produce both the sealed-manifest [`BlockRef`] (digest over the real
/// payload bytes) and the [`BlockPayload`] to pack. The digest property the container relies on —
/// `payload bytes == what the digest was computed over` — holds by construction.
pub fn array_block(
    name: &str,
    spec: &ArraySpec,
    data: &ArrayData,
) -> Result<(BlockRef, BlockPayload)> {
    let payload = encode(spec, data)?;
    let digest = tessera_core::hash::digest(&payload);
    let block_ref = BlockRef {
        name: name.to_string(),
        kind: BlockKind::Array,
        digest: Some(digest),
        spec: serde_json::to_value(spec)?,
    };
    Ok((block_ref, BlockPayload::new(name, payload)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pcodec_spec(shape: Vec<u64>, dtype: &str) -> ArraySpec {
        // The backend speaks pcodec; ArraySpec::new still defaults codec to zstd in core, so set it.
        let mut s = ArraySpec::new(shape, dtype);
        s.codec = "pcodec".into();
        s
    }

    #[test]
    fn le_bytes_match_numpy_code_layout() {
        // i2 little-endian: 1, -2 → [01 00, fe ff]; numpy_code drives element width.
        let d = ArrayData::I16(vec![1, -2, 256]);
        assert_eq!(d.numpy_code(), "i2");
        assert_eq!(d.to_le_bytes(), vec![0x01, 0x00, 0xfe, 0xff, 0x00, 0x01]);
        // f4 round-trips bit-exactly through the LE byte view.
        let f = ArrayData::F32(vec![1.5, -0.0, f32::INFINITY]);
        assert_eq!(f.numpy_code(), "f4");
        let bytes = f.to_le_bytes();
        assert_eq!(bytes.len(), 3 * 4);
        let mut chunks = bytes.chunks_exact(4);
        assert_eq!(
            f32::from_le_bytes(chunks.next().unwrap().try_into().unwrap()),
            1.5
        );
    }

    #[test]
    fn from_le_bytes_inverts_to_le_bytes() {
        let cases = [
            ArrayData::I16(vec![1, -2, 300]),
            ArrayData::U32(vec![0, 7, 4_000_000_000]),
            ArrayData::F64(vec![1.5, -0.0, 1e300]),
        ];
        for d in cases {
            let back = ArrayData::from_le_bytes(d.numpy_code(), &d.to_le_bytes()).unwrap();
            assert_eq!(back, d);
        }
        assert!(ArrayData::from_le_bytes("u1", &[1, 2, 3]).is_err()); // 8-bit out of pcodec scope
        assert!(ArrayData::from_le_bytes("i2", &[1, 2, 3]).is_err()); // not a multiple of width
    }

    #[test]
    fn roundtrip_every_dtype() {
        let shape = vec![4u64, 4, 4];
        let n = 64usize;
        let cases = [
            ArrayData::I16((0..n).map(|k| k as i16 - 32).collect()),
            ArrayData::I32((0..n).map(|k| k as i32 * 7 - 100).collect()),
            ArrayData::I64((0..n).map(|k| k as i64 * 1_000_003 - 5).collect()),
            ArrayData::U16((0..n).map(|k| (k * 513) as u16).collect()),
            ArrayData::U32((0..n).map(|k| (k * 999) as u32).collect()),
            ArrayData::U64((0..n).map(|k| (k as u64) << 40).collect()),
            ArrayData::F32((0..n).map(|k| k as f32 * 0.25 - 8.0).collect()),
            ArrayData::F64((0..n).map(|k| k as f64 * 1.5 - 48.0).collect()),
        ];
        for data in cases {
            let spec = pcodec_spec(shape.clone(), data.dtype());
            let blob = encode(&spec, &data).unwrap();
            let back = decode(&spec, &blob).unwrap();
            assert_eq!(back, data, "roundtrip mismatch for {}", data.dtype());
            // determinism: same input → byte-identical payload
            assert_eq!(
                encode(&spec, &data).unwrap(),
                blob,
                "non-deterministic {}",
                data.dtype()
            );
        }
    }

    #[test]
    fn float_bit_patterns_survive_exactly() {
        // NaN / ±inf / −0.0 / denormal must roundtrip bit-for-bit (the S13 clinical gate).
        let mut f32v: Vec<f32> = (0..64).map(|k| k as f32 * 0.5 - 8.0).collect();
        for (j, v) in [
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            -0.0,
            0.0,
            f32::MIN_POSITIVE,
            f32::MIN_POSITIVE / 2.0,
            f32::MIN,
            f32::MAX,
        ]
        .into_iter()
        .enumerate()
        {
            f32v[j] = v;
        }
        let spec = pcodec_spec(vec![4, 4, 4], "float32");
        let data = ArrayData::F32(f32v.clone());
        let back = decode(&spec, &encode(&spec, &data).unwrap()).unwrap();
        let ArrayData::F32(got) = back else {
            panic!("dtype")
        };
        for (a, b) in got.iter().zip(&f32v) {
            assert_eq!(
                a.to_bits(),
                b.to_bits(),
                "f32 bit pattern diverged: {a} vs {b}"
            );
        }

        let mut f64v: Vec<f64> = (0..64).map(|k| k as f64).collect();
        for (j, v) in [
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            -0.0,
            f64::MIN_POSITIVE / 2.0,
        ]
        .into_iter()
        .enumerate()
        {
            f64v[j] = v;
        }
        let spec = pcodec_spec(vec![4, 4, 4], "float64");
        let data = ArrayData::F64(f64v.clone());
        let back = decode(&spec, &encode(&spec, &data).unwrap()).unwrap();
        let ArrayData::F64(got) = back else {
            panic!("dtype")
        };
        for (a, b) in got.iter().zip(&f64v) {
            assert_eq!(a.to_bits(), b.to_bits(), "f64 bit pattern diverged");
        }
    }

    #[test]
    fn roi_subset_matches_full_decode() {
        // 4³ int32 ramp where value == flat C-order index, so the expected ROI is computable.
        let shape = vec![4u64, 4, 4];
        let data = ArrayData::I32((0..64).collect());
        let spec = pcodec_spec(shape, "int32");
        let blob = encode(&spec, &data).unwrap();

        let start = [1u64, 1, 1];
        let sub = [2u64, 2, 2];
        let ArrayData::I32(got) = decode_subset(&spec, &blob, &start, &sub).unwrap() else {
            panic!("dtype")
        };
        let mut expected: Vec<i32> = Vec::new();
        for z in 1..3 {
            for y in 1..3 {
                for x in 1..3 {
                    expected.push(z * 16 + y * 4 + x);
                }
            }
        }
        assert_eq!(got, expected);
    }

    #[test]
    fn pcodec_compresses_smooth_int16_volume() {
        // perf-SLA §D, machine-INDEPENDENT floor: pcodec must shrink a CT-like smooth int16 volume
        // well below raw (the wall-clock floors are benched separately, not gated). Deterministic.
        let spec = pcodec_spec(vec![64, 64, 64], "int16");
        let data = ArrayData::I16(
            (0..64 * 64 * 64)
                .map(|k| {
                    let z = k / (64 * 64);
                    let y = (k / 64) % 64;
                    (z * 16 + y * 2 - 1024) as i16
                })
                .collect(),
        );
        let raw = 64 * 64 * 64 * 2;
        let blob = encode(&spec, &data).unwrap();
        assert!(
            blob.len() * 4 < raw,
            "pcodec should achieve >4x on smooth int16: {} vs raw {raw}",
            blob.len()
        );
    }

    #[test]
    fn array_block_digest_is_over_real_payload() {
        let spec = pcodec_spec(vec![8, 8, 8], "int16");
        let data = ArrayData::I16((0..512).map(|k| (k % 4096) as i16 - 1024).collect());
        let (block_ref, payload) = array_block("volume", &spec, &data).unwrap();
        assert_eq!(
            block_ref.digest.unwrap(),
            tessera_core::hash::digest(&payload.bytes)
        );
        // and the payload actually decodes back to the data
        assert_eq!(decode(&spec, &payload.bytes).unwrap(), data);
    }

    #[test]
    fn rejects_dtype_len_and_codec_mismatches() {
        let spec = pcodec_spec(vec![2, 2, 2], "int16"); // expects 8 int16 samples
        assert!(matches!(
            encode(&spec, &ArrayData::F32(vec![0.0; 8])),
            Err(Error::Codec(_))
        )); // wrong dtype
        assert!(matches!(
            encode(&spec, &ArrayData::I16(vec![0; 7])),
            Err(Error::Codec(_))
        )); // wrong length
        let mut zstd_spec = ArraySpec::new(vec![2, 2, 2], "int16"); // codec defaults to zstd
        zstd_spec.codec = "zstd".into();
        assert!(matches!(
            encode(&zstd_spec, &ArrayData::I16(vec![0; 8])),
            Err(Error::Codec(_))
        )); // unsupported codec
    }
}
