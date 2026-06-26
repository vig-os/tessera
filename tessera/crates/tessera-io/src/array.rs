//! Array block backend — dense N-D arrays as a deterministic single-blob Zarr v3 store
//! (zarrs + selectable per-block codec). This is the real codec behind an [`ArraySpec`] block:
//! it turns a typed buffer of samples into the exact bytes stored at `blocks/<name>` in a
//! `.tsra`, and back.
//!
//! ## Why a serialized store, not loose Zarr files
//! A Zarr array is a *set* of keys (`zarr.json` + chunk objects). The `.tsra` container stores one
//! payload per block, so we serialize the whole store into ONE deterministic blob: keys sorted,
//! each framed `len|key|len|value`. Reconstructing the store and decoding is lossless, and because
//! the framing is canonical and the chosen codec + the Zarr v3 metadata are themselves
//! deterministic, the same array always encodes to byte-identical bytes — the writer-determinism
//! release gate. The blob is a faithful Zarr store, so the exploded form can later materialize it
//! as real OME-Zarr files.
//!
//! ## Dtypes
//! pcodec operates on ≥16-bit numbers, which covers every dtype real imaging volumes use (int16
//! CT/PET, float32 µ-maps/SUV, uint16/32 counts). The eight pcodec-native dtypes are supported via
//! [`ArrayData`]; 8-bit (`int8`/`uint8`) and `float16` are intentionally out of scope (the zstd
//! backend uses the same [`ArrayData`] surface, so its dtype envelope matches pcodec's).
//!
//! ## Codecs (`ArraySpec.codec`)
//! Three values are accepted on encode:
//! - `"pcodec"` (**default**) — the settled imaging-volume codec (lossless, −21% CT / −33% PET
//!   vs zstd). Recon/imaging/volume products default here and **stay** here unless a caller
//!   explicitly opts a block into a different codec.
//! - `"zstd"` — Zarr v3 `bytes` + `zstd` bytes-to-bytes codec, at the FIXED level
//!   [`ZSTD_LEVEL`] = 3 (deterministic; level is hard-coded so the bytes are a pure function of
//!   the data — never the build profile or a writer knob).
//! - `"auto"` — a *write-time selector*: encode with both pcodec and zstd, keep the smaller
//!   payload, and record the **concrete** codec in the produced [`BlockRef`]/spec. The
//!   manifest thus declares `"pcodec"` or `"zstd"`, never `"auto"`; readers never see `"auto"`.
//!
//! Decode is **codec-agnostic** — zarrs reads the codec chain from the array's `zarr.json` — so
//! [`decode`] and [`decode_subset`] need no dispatch; the only requirement is that the relevant
//! codec features are compiled in (they are, see workspace `Cargo.toml`).
//!
//! ### Codec choice does NOT affect slice / ROI access
//! Slice and sub-cube locality come from the **64³ chunk grid** that `chunks` defines, not from
//! the codec: both `"pcodec"` and `"zstd"` are per-chunk codecs, so [`decode_subset`] (and any
//! z-slice / ROI read above it) decodes only the intersecting chunks regardless of which codec
//! was used to write them. This is the reason `"zstd"` / `"auto"` are safe to expose for
//! dense / high-entropy auxiliary arrays without harming main-volume slice-viewing performance.
//!
//! ### When to reach for which codec
//! - **Recon / imaging / volume blocks** → leave as `"pcodec"` (the default). On real CT/PET it
//!   compresses ~3.8× vs zstd ~3.3× — pcodec wins, so `"auto"` would naturally land on `"pcodec"`
//!   for an imaging volume anyway; there's no reason to pay `"auto"`'s double-encode cost there.
//! - **Dense / high-entropy tabular or auxiliary arrays** (lookup tables, sinogram-like noise,
//!   packed bitfields) → consider `"zstd"` explicitly, or `"auto"` if you want the writer to pick
//!   per block — these are the workloads where pcodec's numerical-pattern heuristics gain little
//!   and zstd's LZ + entropy coding wins.

use std::ops::Range;
use std::sync::Arc;

use bytes::Bytes;
use tessera_core::block::array::ArraySpec;
use tessera_core::block::table::{Column, TableSpec};
use tessera_core::block::{BlockKind, BlockRef};
use tessera_core::chunk_index::{ChunkIndex, ChunkStats};
use tessera_core::hash::digest;
use tessera_core::{Error, Result};

use crate::table::{ColumnData, TableData};
use zarrs::array::builder::ArrayBuilderFillValue;
use zarrs::array::codec::array_to_bytes::pcodec::{PcodecCodec, PcodecCodecConfiguration};
use zarrs::array::codec::ZstdCodec;
use zarrs::array::{data_type, Array, ArrayBuilder, DataType, Element, ElementOwned};
use zarrs::storage::store::MemoryStore;
use zarrs::storage::{ReadableWritableListableStorage, StoreKey};

use crate::BlockPayload;

/// The single array's node path inside the (block-private) Zarr store.
const ARRAY_PATH: &str = "/array";

/// Fixed zstd compression level for the `"zstd"` codec path. Hard-coded so the encoded bytes are
/// a pure function of the input data — never the writer's build profile, host, or a runtime knob
/// — which is what `auto`-selection's "pick smaller of pcodec/zstd" + the writer-determinism gate
/// rely on. Level 3 = zstd's library default: strong ratio at low CPU cost (fine for the dense
/// medical-volume regime where zstd is only a fallback to pcodec).
pub const ZSTD_LEVEL: i32 = 3;

/// The set of array codec ids understood by the encoder. `"auto"` is a write-time selector that
/// resolves at [`array_block`] time; the stored manifest only ever records a *concrete* codec.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Codec {
    Pcodec,
    Zstd,
}

impl Codec {
    fn as_str(self) -> &'static str {
        match self {
            Codec::Pcodec => "pcodec",
            Codec::Zstd => "zstd",
        }
    }
}

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

    /// The array's values as `i64` for chunk-statistics (ADR-0028 §3), if it is an integer array that
    /// fits losslessly: `int16/int32/int64`, `uint16/uint32` always, and `uint64` only when every value
    /// ≤ `i64::MAX` (a monotonic cast → `min`/`max` stay exact). Float arrays return `None` (they need
    /// canonical reduction before stats — ADR-0024). Mirrors [`crate::table::ColumnData::as_i64`].
    pub fn as_i64(&self) -> Option<Vec<i64>> {
        match self {
            ArrayData::I16(v) => Some(v.iter().map(|&x| x as i64).collect()),
            ArrayData::I32(v) => Some(v.iter().map(|&x| x as i64).collect()),
            ArrayData::I64(v) => Some(v.clone()),
            ArrayData::U16(v) => Some(v.iter().map(|&x| x as i64).collect()),
            ArrayData::U32(v) => Some(v.iter().map(|&x| x as i64).collect()),
            ArrayData::U64(v) => v
                .iter()
                .all(|&x| x <= i64::MAX as u64)
                .then(|| v.iter().map(|&x| x as i64).collect()),
            ArrayData::F32(_) | ArrayData::F64(_) => None,
        }
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

fn zstd_codec() -> Arc<ZstdCodec> {
    // checksum=false → no trailing zstd checksum frame (the BlockRef digest already covers
    // payload integrity end-to-end). Fixed level keeps bytes a pure function of the data.
    Arc::new(ZstdCodec::new(ZSTD_LEVEL, false))
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

/// Build a Zarr array in `store` with the chosen codec and write `data` into it (the whole array).
/// `pcodec` is plugged as the `array_to_bytes` codec (it produces bytes directly); `zstd` rides on
/// top of zarrs' default `bytes` array_to_bytes codec as a `bytes_to_bytes` step.
fn build_filled<T: Element, F: Into<ArrayBuilderFillValue>>(
    store: &ReadableWritableListableStorage,
    spec: &ArraySpec,
    dt: DataType,
    fill: F,
    data: &[T],
    codec: Codec,
) -> Result<()> {
    let mut builder = ArrayBuilder::new(spec.shape.clone(), spec.chunks.clone(), dt, fill);
    match codec {
        Codec::Pcodec => {
            builder.array_to_bytes_codec(pcodec());
        }
        Codec::Zstd => {
            // Leave array_to_bytes as the zarrs default (`bytes` codec) and add zstd as the
            // bytes_to_bytes step. Cf. zarrs ArrayBuilder docstring example.
            builder.bytes_to_bytes_codecs(vec![zstd_codec()]);
        }
    }
    let array = builder.build(store.clone(), ARRAY_PATH).map_err(ze)?;
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
/// The encoder accepts the three documented codec ids: `"pcodec"`, `"zstd"`, `"auto"`. Any other
/// value is a writer-side bug and is rejected here, before any encode work happens.
fn validate(spec: &ArraySpec, data: &ArrayData) -> Result<()> {
    spec.validate()?;
    match spec.codec.as_str() {
        "pcodec" | "zstd" | "auto" => {}
        other => {
            return Err(Error::Codec(format!(
                "array codec '{other}' is not supported (expected 'pcodec', 'zstd', or 'auto')"
            )))
        }
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

/// Encode `data` under one *concrete* codec (no `"auto"` resolution here). Internal helper —
/// callers go through [`encode`] or [`encode_resolved`].
fn encode_with(spec: &ArraySpec, data: &ArrayData, codec: Codec) -> Result<Vec<u8>> {
    let store: ReadableWritableListableStorage = Arc::new(MemoryStore::new());
    match data {
        ArrayData::I16(v) => build_filled(&store, spec, data_type::int16(), 0i16, v, codec)?,
        ArrayData::I32(v) => build_filled(&store, spec, data_type::int32(), 0i32, v, codec)?,
        ArrayData::I64(v) => build_filled(&store, spec, data_type::int64(), 0i64, v, codec)?,
        ArrayData::U16(v) => build_filled(&store, spec, data_type::uint16(), 0u16, v, codec)?,
        ArrayData::U32(v) => build_filled(&store, spec, data_type::uint32(), 0u32, v, codec)?,
        ArrayData::U64(v) => build_filled(&store, spec, data_type::uint64(), 0u64, v, codec)?,
        ArrayData::F32(v) => build_filled(&store, spec, data_type::float32(), 0.0f32, v, codec)?,
        ArrayData::F64(v) => build_filled(&store, spec, data_type::float64(), 0.0f64, v, codec)?,
    }
    serialize_store(&store)
}

/// Encode `data` into the deterministic array-block payload bytes for `spec` and report the
/// *concrete* codec the bytes were produced with. For `spec.codec ∈ {"pcodec","zstd"}` this is a
/// straight passthrough; for `spec.codec == "auto"` the encoder runs both codecs and keeps the
/// smaller payload (ties resolve to `"pcodec"`, the project default) — a pure function of the
/// data, so writer-determinism is preserved.
pub fn encode_resolved(spec: &ArraySpec, data: &ArrayData) -> Result<(&'static str, Vec<u8>)> {
    validate(spec, data)?;
    match spec.codec.as_str() {
        "pcodec" => Ok((
            Codec::Pcodec.as_str(),
            encode_with(spec, data, Codec::Pcodec)?,
        )),
        "zstd" => Ok((Codec::Zstd.as_str(), encode_with(spec, data, Codec::Zstd)?)),
        "auto" => {
            let p = encode_with(spec, data, Codec::Pcodec)?;
            let z = encode_with(spec, data, Codec::Zstd)?;
            // strict `<` → on tie, pcodec wins (the deterministic, documented default).
            if z.len() < p.len() {
                Ok((Codec::Zstd.as_str(), z))
            } else {
                Ok((Codec::Pcodec.as_str(), p))
            }
        }
        // unreachable: `validate` already rejected anything else.
        other => Err(Error::Codec(format!("unsupported array codec '{other}'"))),
    }
}

/// Encode `data` into the deterministic array-block payload bytes for `spec`. Thin wrapper
/// around [`encode_resolved`] for callers that only need the payload bytes (e.g. determinism
/// re-encodes in tests). The full pipeline goes through [`array_block`] so the resolved-codec
/// name lands in the [`BlockRef`].
pub fn encode(spec: &ArraySpec, data: &ArrayData) -> Result<Vec<u8>> {
    Ok(encode_resolved(spec, data)?.1)
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
///
/// If `spec.codec == "auto"`, the resolved concrete codec (`"pcodec"` or `"zstd"`) is written
/// back into the `BlockRef`'s stored spec so the manifest never declares `"auto"` (readers see a
/// concrete codec and the conformance contract holds). The caller's input `spec` is not mutated.
pub fn array_block(
    name: &str,
    spec: &ArraySpec,
    data: &ArrayData,
) -> Result<(BlockRef, BlockPayload)> {
    let (resolved, payload) = encode_resolved(spec, data)?;
    let digest = tessera_core::hash::digest(&payload);
    // Record the concrete codec in the manifest's BlockRef.spec — readers never see "auto".
    let mut stored_spec = spec.clone();
    stored_spec.codec = resolved.to_string();
    let block_ref = BlockRef {
        name: name.to_string(),
        kind: BlockKind::Array,
        digest: Some(digest),
        spec: serde_json::to_value(&stored_spec)?,
    };
    Ok((block_ref, BlockPayload::new(name, payload)))
}

/// Build the `{hash, stats}` chunk-index (ADR-0028 §3) for an array block over its **N-D chunk grid**
/// (`spec.chunks`, default 64³). Each entry is one chunk: a content digest (BLAKE3 over the chunk's
/// C-order little-endian `i64` values — recomputable from the decoded array, independent of the Zarr
/// byte layout) and the chunk's [`ChunkStats`]. `index.root()` is the block's sub-block Merkle root and
/// `index.prune()` skips chunks a ranged read can't hit. Integer arrays only (see [`ArrayData::as_i64`]).
///
/// Chunks are visited in **C-order over the chunk grid** (last axis fastest), matching the array's
/// storage order, so the index entry order is deterministic and reproducible.
///
/// ```
/// use tessera_core::block::array::ArraySpec;
/// use tessera_io::array::{array_chunk_index, ArrayData};
///
/// let mut spec = ArraySpec::new(vec![4, 4, 4], "int16");
/// spec.chunks = vec![2, 2, 2]; // 8 chunks of 2³ over a 4³ volume
/// let idx = array_chunk_index(&spec, &ArrayData::I16((0..64).map(|k| k as i16).collect())).unwrap();
///
/// assert_eq!(idx.len(), 8);
/// assert_eq!(idx.aggregate().max, Some(63)); // stats roll up to the whole array
/// assert_eq!(idx.prune(0, 0), vec![0]); // value 0 lives only in the first chunk
/// assert!(idx.root().starts_with("blake3:")); // sub-block MMR root (ADR-0028 §1)
/// ```
pub fn array_chunk_index(spec: &ArraySpec, data: &ArrayData) -> Result<ChunkIndex> {
    let vals = data
        .as_i64()
        .ok_or_else(|| Error::Codec("array dtype is not integer for chunk stats".into()))?;
    let shape: Vec<usize> = spec.shape.iter().map(|&d| d as usize).collect();
    let chunks: Vec<usize> = spec.chunks.iter().map(|&c| (c as usize).max(1)).collect();
    if shape.len() != chunks.len() {
        return Err(Error::Codec(format!(
            "array rank mismatch: shape rank {} != chunks rank {}",
            shape.len(),
            chunks.len()
        )));
    }
    let total: usize = shape.iter().product();
    if vals.len() != total {
        return Err(Error::Codec(format!(
            "array has {} values, shape implies {total}",
            vals.len()
        )));
    }
    let rank = shape.len();
    // C-order strides: stride[i] = product of trailing dims.
    let mut strides = vec![1usize; rank];
    for i in (0..rank.saturating_sub(1)).rev() {
        strides[i] = strides[i + 1] * shape[i + 1];
    }
    let nchunks: Vec<usize> = shape
        .iter()
        .zip(&chunks)
        .map(|(&d, &c)| d.div_ceil(c).max(1))
        .collect();
    let total_chunks: usize = nchunks.iter().product();

    let mut idx = ChunkIndex::new();
    let mut cidx = vec![0usize; rank]; // chunk multi-index
    for _ in 0..total_chunks {
        let lo: Vec<usize> = cidx.iter().zip(&chunks).map(|(&g, &c)| g * c).collect();
        let hi: Vec<usize> = lo
            .iter()
            .zip(&chunks)
            .zip(&shape)
            .map(|((&l, &c), &d)| (l + c).min(d))
            .collect();
        let count: usize = lo.iter().zip(&hi).map(|(&l, &h)| h - l).product();

        // gather this chunk's values by an odometer over voxel coords in [lo, hi) (last axis fastest).
        let mut bytes = Vec::with_capacity(count * 8);
        let mut chunk_vals = Vec::with_capacity(count);
        let mut v = lo.clone();
        for _ in 0..count {
            let flat: usize = v.iter().zip(&strides).map(|(&x, &s)| x * s).sum();
            let val = vals[flat];
            bytes.extend_from_slice(&val.to_le_bytes());
            chunk_vals.push(val);
            for ax in (0..rank).rev() {
                v[ax] += 1;
                if v[ax] < hi[ax] {
                    break;
                }
                v[ax] = lo[ax];
            }
        }
        idx.push_entry(digest(&bytes), ChunkStats::from_values(&chunk_vals));

        // advance the chunk multi-index in C-order (last axis fastest).
        for ax in (0..rank).rev() {
            cidx[ax] += 1;
            if cidx[ax] < nchunks[ax] {
                break;
            }
            cidx[ax] = 0;
        }
    }
    Ok(idx)
}

/// Convert a dense **integer** array to its **COO** (coordinate-list) table form (ADR-0031 §2): one row
/// per voxel whose value differs from `fill`, columns `idx` (linear C-order index, `u64`) + `v` (value,
/// `i64`). The absent (un-stored) voxels are the `fill` value, so the dense array reconstructs from
/// `(shape, fill, rows)` — `fill` is part of identity (ADR-0031 §4), hence an explicit argument rather
/// than a hard-coded 0. `None` for a float array (needs canonical reduction, ADR-0024). The *scatter*-
/// sparse substrate-by-nature path — chosen only when the ambient grid is unstorable or access is
/// selective-nnz (per #221-A, dense+pcodec wins on disk at storable scales, so this is **not** the
/// default). The columns ride the ordinary Vortex table encoder (no new primitive).
pub fn to_coo(data: &ArrayData, fill: i64) -> Option<(TableSpec, TableData)> {
    let vals = data.as_i64()?;
    let (mut idx, mut v) = (Vec::new(), Vec::new());
    for (k, &x) in vals.iter().enumerate() {
        if x != fill {
            idx.push(k as u64);
            v.push(x);
        }
    }
    let col = |name: &str, dt: &str| Column {
        name: name.into(),
        dtype: dt.into(),
        codec: None,
    };
    let spec = TableSpec {
        columns: vec![col("idx", "u8"), col("v", "i8")],
        rows: v.len() as u64,
        row_index: Some("idx".into()),
    };
    let tdata: TableData = vec![
        ("idx".into(), ColumnData::U64(idx)),
        ("v".into(), ColumnData::I64(v)),
    ];
    Some((spec, tdata))
}

/// Downsample a dense **3-D integer** array by 2× per axis via **max** reduction over each (up-to)
/// 2×2×2 block — one multiscale pyramid level (ADR-0028 §7, the array overview; MIP-style max preserves
/// "is anything here", and pairs with [`tessera_core::block::array::WorldFrame::at_level`] for the
/// level's geometry). Output shape is `ceil(d/2)` per axis (edge blocks clamp); the level keeps the base
/// dtype (max stays in range). `None` for non-3-D or float arrays (float reduction needs ADR-0024
/// canonicalisation). Deterministic.
pub fn downsample_max_3d(spec: &ArraySpec, data: &ArrayData) -> Option<(ArraySpec, ArrayData)> {
    if spec.shape.len() != 3 {
        return None;
    }
    let vals = data.as_i64()?;
    let (d0, d1, d2) = (
        spec.shape[0] as usize,
        spec.shape[1] as usize,
        spec.shape[2] as usize,
    );
    if vals.len() != d0 * d1 * d2 {
        return None;
    }
    let (o0, o1, o2) = (d0.div_ceil(2), d1.div_ceil(2), d2.div_ceil(2));
    let mut out = Vec::with_capacity(o0 * o1 * o2);
    for oz in 0..o0 {
        for oy in 0..o1 {
            for ox in 0..o2 {
                let mut m = i64::MIN;
                for z in (2 * oz)..(2 * oz + 2).min(d0) {
                    for y in (2 * oy)..(2 * oy + 2).min(d1) {
                        for x in (2 * ox)..(2 * ox + 2).min(d2) {
                            m = m.max(vals[(z * d1 + y) * d2 + x]);
                        }
                    }
                }
                out.push(m);
            }
        }
    }
    // rebuild in the base dtype — max of in-range values is in-range, so the cast is lossless.
    let data_out = match data {
        ArrayData::I16(_) => ArrayData::I16(out.iter().map(|&x| x as i16).collect()),
        ArrayData::I32(_) => ArrayData::I32(out.iter().map(|&x| x as i32).collect()),
        ArrayData::I64(_) => ArrayData::I64(out),
        ArrayData::U16(_) => ArrayData::U16(out.iter().map(|&x| x as u16).collect()),
        ArrayData::U32(_) => ArrayData::U32(out.iter().map(|&x| x as u32).collect()),
        ArrayData::U64(_) => ArrayData::U64(out.iter().map(|&x| x as u64).collect()),
        ArrayData::F32(_) | ArrayData::F64(_) => return None, // unreachable: as_i64 already returned None
    };
    let mut out_spec = spec.clone();
    out_spec.shape = vec![o0 as u64, o1 as u64, o2 as u64];
    out_spec.chunks = out_spec
        .chunks
        .iter()
        .zip(&out_spec.shape)
        .map(|(&c, &d)| c.min(d).max(1))
        .collect();
    Some((out_spec, data_out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pcodec_spec(shape: Vec<u64>, dtype: &str) -> ArraySpec {
        // `ArraySpec::new` already defaults `codec` to "pcodec" (the volume-codec winner); spell
        // it out here so the test's intent is explicit independent of the core default.
        let mut s = ArraySpec::new(shape, dtype);
        s.codec = "pcodec".into();
        s
    }

    fn spec_with(shape: Vec<u64>, dtype: &str, codec: &str) -> ArraySpec {
        let mut s = ArraySpec::new(shape, dtype);
        s.codec = codec.into();
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
        let mut bogus_spec = ArraySpec::new(vec![2, 2, 2], "int16");
        bogus_spec.codec = "lz4hc-experimental".into(); // not in {pcodec,zstd,auto}
        assert!(matches!(
            encode(&bogus_spec, &ArrayData::I16(vec![0; 8])),
            Err(Error::Codec(_))
        )); // unsupported codec id is rejected
    }

    // ── zstd codec (parity with pcodec on the eight supported dtypes) ──────────────────────────

    #[test]
    fn zstd_roundtrip_every_dtype() {
        // Same shape/data shape as the pcodec roundtrip test; if zstd is plumbed correctly the
        // decode is fully codec-agnostic (zarrs reads the codec from zarr.json) so the back-side
        // is identical to the pcodec path.
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
            let spec = spec_with(shape.clone(), data.dtype(), "zstd");
            let blob = encode(&spec, &data).unwrap();
            let back = decode(&spec, &blob).unwrap();
            assert_eq!(back, data, "zstd roundtrip mismatch for {}", data.dtype());
            assert_eq!(
                encode(&spec, &data).unwrap(),
                blob,
                "zstd non-deterministic for {}",
                data.dtype()
            );
        }
    }

    #[test]
    fn zstd_float_bit_patterns_survive_exactly() {
        // Clinical-grade float-edge gate, mirroring `float_bit_patterns_survive_exactly` but for
        // the zstd codec. `bytes` array_to_bytes is byte-exact; zstd is lossless byte-to-byte.
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
        let spec = spec_with(vec![4, 4, 4], "float32", "zstd");
        let data = ArrayData::F32(f32v.clone());
        let back = decode(&spec, &encode(&spec, &data).unwrap()).unwrap();
        let ArrayData::F32(got) = back else {
            panic!("dtype")
        };
        for (a, b) in got.iter().zip(&f32v) {
            assert_eq!(
                a.to_bits(),
                b.to_bits(),
                "zstd f32 bit pattern diverged: {a} vs {b}"
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
        let spec = spec_with(vec![4, 4, 4], "float64", "zstd");
        let data = ArrayData::F64(f64v.clone());
        let back = decode(&spec, &encode(&spec, &data).unwrap()).unwrap();
        let ArrayData::F64(got) = back else {
            panic!("dtype")
        };
        for (a, b) in got.iter().zip(&f64v) {
            assert_eq!(a.to_bits(), b.to_bits(), "zstd f64 bit pattern diverged");
        }
    }

    // ── auto codec (write-time selector; records a concrete codec) ─────────────────────────────

    #[test]
    fn auto_picks_zstd_for_high_entropy_data() {
        // Random-looking int32 noise: pcodec's delta/numerical-pattern heuristics gain ~nothing,
        // while zstd's LZ + entropy coding still finds short repeats — zstd should win. The
        // exact winner doesn't matter for correctness, only that `auto` records a concrete codec
        // AND the recorded codec equals whichever path actually produced the stored bytes.
        let shape = vec![8u64, 8, 8];
        let data = ArrayData::I32(
            (0..512)
                .map(|k| {
                    // splitmix64-style PRNG, deterministic
                    let mut z = (k as u64).wrapping_mul(0x9E3779B97F4A7C15);
                    z ^= z >> 30;
                    z = z.wrapping_mul(0xBF58476D1CE4E5B9);
                    z ^= z >> 27;
                    (z & 0xFFFF_FFFF) as i32
                })
                .collect(),
        );
        let auto_spec = spec_with(shape.clone(), "int32", "auto");
        let (block_ref, payload) = array_block("noisy", &auto_spec, &data).unwrap();
        let recorded = block_ref
            .spec
            .get("codec")
            .and_then(|v| v.as_str())
            .unwrap();
        assert!(
            recorded == "pcodec" || recorded == "zstd",
            "auto must resolve to a concrete codec, got {recorded:?}"
        );
        // The recorded codec must be the one whose bytes were actually stored.
        let p_spec = spec_with(shape.clone(), "int32", "pcodec");
        let z_spec = spec_with(shape.clone(), "int32", "zstd");
        let p_bytes = encode(&p_spec, &data).unwrap();
        let z_bytes = encode(&z_spec, &data).unwrap();
        let winner = if z_bytes.len() < p_bytes.len() {
            "zstd"
        } else {
            "pcodec"
        };
        assert_eq!(recorded, winner, "auto recorded the wrong codec");
        assert_eq!(
            payload.bytes.as_slice(),
            if winner == "zstd" {
                z_bytes.as_slice()
            } else {
                p_bytes.as_slice()
            },
            "auto must store the bytes of the winning codec"
        );
        // And whatever it picked must decode back to the input.
        let decoded = decode(
            &spec_with(shape.clone(), "int32", recorded),
            payload.bytes.as_slice(),
        )
        .unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn auto_picks_pcodec_for_smooth_data() {
        // Smooth-gradient int16: pcodec's delta encoding crushes it well below what zstd can do
        // on the raw little-endian bytes. `auto` should record `"pcodec"` here — the same
        // workload `pcodec_compresses_smooth_int16_volume` exercises.
        let shape = vec![64u64, 64, 64];
        let data = ArrayData::I16(
            (0..64 * 64 * 64)
                .map(|k| {
                    let z = k / (64 * 64);
                    let y = (k / 64) % 64;
                    (z * 16 + y * 2 - 1024) as i16
                })
                .collect(),
        );
        let auto_spec = spec_with(shape, "int16", "auto");
        let (block_ref, _payload) = array_block("smooth", &auto_spec, &data).unwrap();
        let recorded = block_ref
            .spec
            .get("codec")
            .and_then(|v| v.as_str())
            .unwrap();
        assert_eq!(
            recorded, "pcodec",
            "auto should pick pcodec on smooth int16; got {recorded:?}"
        );
        // `auto` is never the recorded codec — readers always see a concrete one.
        assert_ne!(recorded, "auto");
    }

    #[test]
    fn auto_resolution_is_deterministic_across_runs() {
        // Two independent `array_block` runs over the same input must produce the SAME concrete
        // codec AND byte-identical payloads. This is the writer-determinism gate for `auto`.
        let shape = vec![8u64, 8, 8];
        let data = ArrayData::I32((0..512i32).map(|k| (k * 17) ^ 0x12345).collect());
        let auto_spec = spec_with(shape, "int32", "auto");
        let (r1, p1) = array_block("v", &auto_spec, &data).unwrap();
        let (r2, p2) = array_block("v", &auto_spec, &data).unwrap();
        assert_eq!(r1.spec.get("codec"), r2.spec.get("codec"));
        assert_eq!(p1.bytes.as_slice(), p2.bytes.as_slice());
        assert_eq!(r1.digest, r2.digest);
    }

    #[test]
    fn zstd_encode_is_byte_deterministic() {
        // Direct byte-determinism check on the zstd path (independent of `auto`). Same input →
        // same payload twice in a row.
        let spec = spec_with(vec![16, 16, 16], "int16", "zstd");
        let data = ArrayData::I16(
            (0..16 * 16 * 16)
                .map(|k| (k as i16).wrapping_mul(7))
                .collect(),
        );
        let a = encode(&spec, &data).unwrap();
        let b = encode(&spec, &data).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn array_block_pcodec_does_not_rewrite_codec() {
        // Explicit `"pcodec"` must round-trip through `array_block` unchanged (regression guard
        // against accidentally re-resolving non-auto codecs).
        let spec = pcodec_spec(vec![4, 4, 4], "int16");
        let data = ArrayData::I16((0..64).map(|k| k as i16).collect());
        let (block_ref, _) = array_block("vol", &spec, &data).unwrap();
        assert_eq!(
            block_ref.spec.get("codec").and_then(|v| v.as_str()),
            Some("pcodec")
        );
    }

    #[test]
    fn array_block_zstd_does_not_rewrite_codec() {
        let spec = spec_with(vec![4, 4, 4], "int16", "zstd");
        let data = ArrayData::I16((0..64).map(|k| k as i16).collect());
        let (block_ref, payload) = array_block("vol", &spec, &data).unwrap();
        assert_eq!(
            block_ref.spec.get("codec").and_then(|v| v.as_str()),
            Some("zstd")
        );
        // And the stored bytes decode back to the input — the decode path is codec-agnostic.
        assert_eq!(decode(&spec, payload.bytes.as_slice()).unwrap(), data);
    }

    #[test]
    fn as_i64_lossless_integer_else_none() {
        // integer dtypes lift to i64 (monotonic → min/max exact)
        assert_eq!(
            ArrayData::I16(vec![-3, 0, 7]).as_i64(),
            Some(vec![-3i64, 0, 7])
        );
        assert_eq!(
            ArrayData::U32(vec![1, 2, 3]).as_i64(),
            Some(vec![1i64, 2, 3])
        );
        // u64 within i64 range lifts; beyond i64::MAX returns None (avoids a wrapping sign flip)
        assert_eq!(ArrayData::U64(vec![5, 9]).as_i64(), Some(vec![5i64, 9]));
        assert_eq!(ArrayData::U64(vec![u64::MAX]).as_i64(), None);
        // floats need canonical reduction first (ADR-0024) → not offered here
        assert_eq!(ArrayData::F32(vec![1.0, 2.0]).as_i64(), None);
        assert_eq!(ArrayData::F64(vec![1.0]).as_i64(), None);
    }

    #[test]
    fn downsample_max_3d_halves_and_maxes() {
        // 4×4×4, value = flat C-order index 0..63 → max-downsample 2× → 2×2×2.
        let spec = ArraySpec::new(vec![4, 4, 4], "int16");
        let data = ArrayData::I16((0..64).map(|k| k as i16).collect());
        let (s2, d2) = downsample_max_3d(&spec, &data).unwrap();
        assert_eq!(s2.shape, vec![2, 2, 2]);
        let ArrayData::I16(out) = d2 else {
            panic!("level keeps base dtype")
        };
        // output (0,0,0) = max over input block {0,1,4,5,16,17,20,21} = 21
        assert_eq!(out[0], 21);
        // output (1,1,1) = max over [2,4)³, which includes flat 63 = 63
        assert_eq!(out[7], 63);

        // odd shape 5×5×5 → 3×3×3 (edge blocks clamp), no panic.
        let (s3, _) = downsample_max_3d(
            &ArraySpec::new(vec![5, 5, 5], "int16"),
            &ArrayData::I16(vec![1; 125]),
        )
        .unwrap();
        assert_eq!(s3.shape, vec![3, 3, 3]);

        // non-3-D and float → None.
        assert!(downsample_max_3d(
            &ArraySpec::new(vec![8], "int16"),
            &ArrayData::I16(vec![0; 8])
        )
        .is_none());
        assert!(downsample_max_3d(&spec, &ArrayData::F32(vec![0.0; 64])).is_none());
    }

    #[test]
    fn to_coo_emits_one_row_per_non_fill_value() {
        // 2×2×2, fill = 0: non-fill voxels at flat indices 0 and 5.
        let mut v = vec![0i16; 8];
        v[0] = 3;
        v[5] = -7;
        let (spec, data) = to_coo(&ArrayData::I16(v), 0).unwrap();
        assert_eq!(spec.rows, 2);
        assert_eq!(spec.row_index.as_deref(), Some("idx"));
        let ColumnData::U64(idx) = &data[0].1 else {
            panic!("idx column should be u64")
        };
        let ColumnData::I64(val) = &data[1].1 else {
            panic!("v column should be i64")
        };
        assert_eq!(idx, &[0, 5]);
        assert_eq!(val, &[3, -7]);

        // non-zero fill (audit fix): a grid that is mostly `5` stores only the voxels that differ.
        let mut w = vec![5i16; 8];
        w[2] = 9;
        let (spec5, data5) = to_coo(&ArrayData::I16(w), 5).unwrap();
        assert_eq!(spec5.rows, 1);
        let ColumnData::U64(i5) = &data5[0].1 else {
            panic!()
        };
        let ColumnData::I64(v5) = &data5[1].1 else {
            panic!()
        };
        assert_eq!(i5, &[2]);
        assert_eq!(v5, &[9]);

        // a float array can't supply integer COO (needs canonical reduction).
        assert!(to_coo(&ArrayData::F32(vec![1.0, 0.0]), 0).is_none());
    }

    #[test]
    fn array_chunk_index_walks_the_3d_chunk_grid() {
        // 4×4×4 C-order 0..63, chunks 2×2×2 → 8 chunks of 8 voxels each.
        let mut spec = ArraySpec::new(vec![4, 4, 4], "int16");
        spec.chunks = vec![2, 2, 2];
        let data = ArrayData::I16((0..64).map(|k| k as i16).collect());
        let idx = array_chunk_index(&spec, &data).unwrap();

        assert_eq!(idx.len(), 8);
        // stats roll up to the whole array [0, 63]
        let agg = idx.aggregate();
        assert_eq!(agg.count, 64);
        assert_eq!(agg.min, Some(0));
        assert_eq!(agg.max, Some(63));
        // first chunk (z,y,x ∈ [0,2)) holds {0,1,4,5,16,17,20,21}: count 8, min 0, max 21
        assert_eq!(idx.entries[0].stats.count, 8);
        assert_eq!(idx.entries[0].stats.min, Some(0));
        assert_eq!(idx.entries[0].stats.max, Some(21));
        // value 0 lives only in the first chunk; 63 only in the last (C-order chunk 7)
        assert_eq!(idx.prune(0, 0), vec![0]);
        assert_eq!(idx.prune(63, 63), vec![7]);
        // deterministic sub-block MMR root over the per-chunk digests
        assert!(idx.root().starts_with("blake3:"));
        assert_eq!(array_chunk_index(&spec, &data).unwrap().root(), idx.root());
        // a float array can't supply integer stats
        assert!(array_chunk_index(&spec, &ArrayData::F32(vec![0.0; 64])).is_err());
    }

    #[test]
    fn array_chunk_index_handles_non_divisible_edge_chunks() {
        // 5×5×5 with 2×2×2 chunks → 3³ = 27 chunks, the edge ones only 1 voxel wide. Exercises the
        // partial-chunk odometer path (hi-lo < chunk_size) the divisible 4³/2³ case never hits.
        let mut spec = ArraySpec::new(vec![5, 5, 5], "int32");
        spec.chunks = vec![2, 2, 2];
        let data = ArrayData::I32((0..125).collect());
        let idx = array_chunk_index(&spec, &data).unwrap();
        assert_eq!(idx.len(), 27);
        // every voxel is gathered exactly once → counts sum to 125, stats span the whole array.
        let agg = idx.aggregate();
        assert_eq!(agg.count, 125);
        assert_eq!(agg.min, Some(0));
        assert_eq!(agg.max, Some(124));
        assert_eq!(idx.entries.iter().map(|e| e.stats.count).sum::<u64>(), 125);
        // the far-corner chunk is a 1³ edge chunk holding only voxel (4,4,4) = flat 124.
        let last = idx.entries.last().unwrap();
        assert_eq!(last.stats.count, 1);
        assert_eq!(last.stats.min, Some(124));
        assert_eq!(last.stats.max, Some(124));
        // value 124 lives only in that corner chunk.
        assert_eq!(idx.prune(124, 124), vec![26]);
    }
}
