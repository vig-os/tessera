//! Derived views over an **array** block — the structured data the CLI `stats` (and later the TUI /
//! serve) render. Decode stays in [`tessera_io::array`]; this module computes summaries over the
//! decoded [`ArrayData`], returning typed structs rather than writing text.

use std::sync::Arc;

use arrow::array::{Float64Array, UInt64Array};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use tessera_core::block::array::ArraySpec;
use tessera_core::Result;
use tessera_io::array::ArrayData;

/// A numeric summary of a decoded array block: value range and central tendency over every element,
/// plus the element count. Raw (stored) units; physical rescale (CT→HU, PET→Bq/mL) is applied by the
/// renderer from the block's `ArraySpec`, not here.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArrayStats {
    /// Minimum element value (`0.0` for an empty block).
    pub min: f64,
    /// Maximum element value (`0.0` for an empty block).
    pub max: f64,
    /// Arithmetic mean of all elements.
    pub mean: f64,
    /// Population standard deviation of all elements (never negative).
    pub std: f64,
    /// Number of elements reduced.
    pub count: usize,
}

/// Reduce a decoded array to its [`ArrayStats`] in a single pass, dispatching over the element type.
///
/// One streaming pass computes min/max/mean/std together; an empty block yields all-zero stats. This
/// is the pure compute behind `tessera stats` — extracted so the TUI and `serve` can reuse it without
/// the CLI's text formatting.
pub fn array_stats(data: &ArrayData) -> ArrayStats {
    macro_rules! reduce {
        ($v:expr) => {{
            let n = $v.len();
            if n == 0 {
                ArrayStats {
                    min: 0.0,
                    max: 0.0,
                    mean: 0.0,
                    std: 0.0,
                    count: 0,
                }
            } else {
                let mut mn = f64::INFINITY;
                let mut mx = f64::NEG_INFINITY;
                let mut sum = 0.0f64;
                let mut sumsq = 0.0f64;
                for &x in $v.iter() {
                    let x = x as f64;
                    mn = mn.min(x);
                    mx = mx.max(x);
                    sum += x;
                    sumsq += x * x;
                }
                let mean = sum / n as f64;
                let var = (sumsq / n as f64) - mean * mean;
                ArrayStats {
                    min: mn,
                    max: mx,
                    mean,
                    std: var.max(0.0).sqrt(),
                    count: n,
                }
            }
        }};
    }
    match data {
        ArrayData::I16(v) => reduce!(v),
        ArrayData::I32(v) => reduce!(v),
        ArrayData::I64(v) => reduce!(v),
        ArrayData::U16(v) => reduce!(v),
        ArrayData::U32(v) => reduce!(v),
        ArrayData::U64(v) => reduce!(v),
        ArrayData::F32(v) => reduce!(v),
        ArrayData::F64(v) => reduce!(v),
    }
}

/// Flatten a decoded array (or a decoded sub-region) to `f64`, optionally applying the block's
/// `(slope, intercept)` rescale to recover physical units (CT→HU, PET→Bq/mL). The renderer decides
/// whether to pass `rescale` (raw vs `--physical`); this is the pure element conversion behind
/// `tessera slice`/`project`.
pub fn region_to_f64(data: &ArrayData, rescale: Option<(f64, f64)>) -> Vec<f64> {
    macro_rules! conv {
        ($v:expr) => {
            $v.iter()
                .map(|&x| {
                    let x = x as f64;
                    match rescale {
                        Some((s, i)) => s * x + i,
                        None => x,
                    }
                })
                .collect()
        };
    }
    match data {
        ArrayData::I16(v) => conv!(v),
        ArrayData::I32(v) => conv!(v),
        ArrayData::I64(v) => conv!(v),
        ArrayData::U16(v) => conv!(v),
        ArrayData::U32(v) => conv!(v),
        ArrayData::U64(v) => conv!(v),
        ArrayData::F32(v) => conv!(v),
        ArrayData::F64(v) => conv!(v),
    }
}

/// A decoded rectangular sub-region of an array as `f64` values, row-major, with its per-axis lengths.
/// The renderer decides how to lay it out (the CLI treats the last axis as columns).
#[derive(Debug, Clone, PartialEq)]
pub struct ArrayRegion {
    /// Region element values (physical or raw, per the `rescale` passed to [`slice_region`]).
    pub values: Vec<f64>,
    /// The region's per-axis lengths (its shape).
    pub shape: Vec<u64>,
}

/// Decode a rectangular sub-region of an array (only the intersecting chunks) and flatten it to `f64`,
/// applying the optional physical `rescale`. The single view-model entry the CLI `slice` (and the TUI /
/// serve) render — index/world resolution into `(start, shape)` is the caller's job.
pub fn slice_region(
    spec: &ArraySpec,
    blob: &[u8],
    start: &[u64],
    shape: &[u64],
    rescale: Option<(f64, f64)>,
) -> Result<ArrayRegion> {
    let region = tessera_io::array::decode_subset(spec, blob, start, shape)?;
    Ok(ArrayRegion {
        values: region_to_f64(&region, rescale),
        shape: shape.to_vec(),
    })
}

/// Reduction mode for [`project_axis`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjMode {
    /// Maximum-intensity projection (MIP) — the classic PET/CT overview.
    Max,
    /// Mean along the axis.
    Mean,
    /// Sum along the axis.
    Sum,
}

/// A projection image: a row-major array reduced along one axis, with its (lower-D) shape.
#[derive(Debug, Clone, PartialEq)]
pub struct Projection {
    /// The surviving axes' lengths (the projected axis is dropped).
    pub shape: Vec<u64>,
    /// Reduced element values, row-major over `shape`.
    pub values: Vec<f64>,
}

/// Reduce a row-major N-D array `values` (of `shape`) along `axis` by `mode`, dropping that axis — a
/// 3-D volume → a 2-D projection (MIP / mean / sum). The pure compute behind `tessera project`.
pub fn project_axis(values: &[f64], shape: &[u64], axis: usize, mode: ProjMode) -> Projection {
    let n = shape.len();
    let mut strides = vec![1usize; n];
    for i in (0..n.saturating_sub(1)).rev() {
        strides[i] = strides[i + 1] * shape[i + 1] as usize;
    }
    let ax_len = shape[axis].max(1) as usize;
    let out_shape: Vec<u64> = shape
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != axis)
        .map(|(_, &d)| d)
        .collect();
    let out_n: usize = out_shape.iter().map(|&d| d as usize).product();
    let init = match mode {
        ProjMode::Max => f64::NEG_INFINITY,
        _ => 0.0,
    };
    let mut out = vec![init; out_n.max(1)];
    for (flat, &v) in values.iter().enumerate() {
        // Output flat index = input coords with the projected axis removed (row-major).
        let mut of = 0usize;
        let mut os = 1usize;
        for i in (0..n).rev() {
            if i == axis {
                continue;
            }
            let coord = (flat / strides[i]) % shape[i] as usize;
            of += coord * os;
            os *= shape[i] as usize;
        }
        match mode {
            ProjMode::Max => out[of] = out[of].max(v),
            ProjMode::Mean | ProjMode::Sum => out[of] += v,
        }
    }
    if matches!(mode, ProjMode::Mean) {
        for o in &mut out {
            *o /= ax_len as f64;
        }
    }
    Projection {
        shape: out_shape,
        values: out,
    }
}

/// Apply the optional physical `rescale` to each element of a decoded array and hand it to `f` as an
/// `f64`. Bounded-memory (no intermediate buffer) — dispatches over the element type once per call.
fn for_each_f64(data: &ArrayData, rescale: Option<(f64, f64)>, mut f: impl FnMut(f64)) {
    let rs = |x: f64| match rescale {
        Some((s, i)) => s * x + i,
        None => x,
    };
    macro_rules! go {
        ($v:expr) => {
            for &x in $v.iter() {
                f(rs(x as f64));
            }
        };
    }
    match data {
        ArrayData::I16(v) => go!(v),
        ArrayData::I32(v) => go!(v),
        ArrayData::I64(v) => go!(v),
        ArrayData::U16(v) => go!(v),
        ArrayData::U32(v) => go!(v),
        ArrayData::U64(v) => go!(v),
        ArrayData::F32(v) => go!(v),
        ArrayData::F64(v) => go!(v),
    }
}

/// A rescale-aware **dense histogram** of an array block into `bins` equal-width bins over the value
/// range, as an Arrow [`RecordBatch`] with columns `bin_lo`, `bin_hi` (`Float64`) and `count`
/// (`UInt64`). Physical `rescale` `(slope, intercept)` is applied per element (raw if `None`). Two
/// bounded-memory passes over the decoded array (min/max, then bin) — no intermediate `f64` buffer.
/// An empty block yields all-zero counts; all-equal values fall in bin 0.
pub fn histogram(data: &ArrayData, bins: usize, rescale: Option<(f64, f64)>) -> RecordBatch {
    let bins = bins.max(1);
    let (mut mn, mut mx, mut n) = (f64::INFINITY, f64::NEG_INFINITY, 0u64);
    for_each_f64(data, rescale, |x| {
        if x < mn {
            mn = x;
        }
        if x > mx {
            mx = x;
        }
        n += 1;
    });
    if n == 0 {
        mn = 0.0;
        mx = 0.0;
    }
    // Avoid a zero-width bin when all values are equal (or the block is empty): everything in bin 0.
    let width = if mx > mn {
        (mx - mn) / bins as f64
    } else {
        1.0
    };
    let mut counts = vec![0u64; bins];
    if n > 0 {
        for_each_f64(data, rescale, |x| {
            let b = (((x - mn) / width).floor() as isize).clamp(0, bins as isize - 1) as usize;
            counts[b] += 1;
        });
    }
    let lo: Vec<f64> = (0..bins).map(|k| mn + k as f64 * width).collect();
    let hi: Vec<f64> = (0..bins).map(|k| mn + (k + 1) as f64 * width).collect();
    let schema = Arc::new(Schema::new(vec![
        Field::new("bin_lo", DataType::Float64, false),
        Field::new("bin_hi", DataType::Float64, false),
        Field::new("count", DataType::UInt64, false),
    ]));
    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(Float64Array::from(lo)),
            Arc::new(Float64Array::from(hi)),
            Arc::new(UInt64Array::from(counts)),
        ],
    )
    .expect("histogram columns are equal length by construction")
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{Array, UInt64Array};

    #[test]
    fn histogram_bins_and_counts_with_rescale() {
        // 0,1,2,3 into 2 bins over [0,3]: width 1.5 → bin0=[0,1.5)={0,1}, bin1=[1.5,3]={2,3}.
        let d = ArrayData::I16(vec![0, 1, 2, 3]);
        let h = histogram(&d, 2, None);
        assert_eq!(h.num_rows(), 2);
        let count = h.column(2).as_any().downcast_ref::<UInt64Array>().unwrap();
        assert_eq!(count.values(), &[2, 2]);
        // Rescale shifts the range but not the counts.
        let hr = histogram(&d, 2, Some((1.0, -1024.0)));
        let cr = hr.column(2).as_any().downcast_ref::<UInt64Array>().unwrap();
        assert_eq!(cr.values(), &[2, 2]);
    }

    #[test]
    fn histogram_empty_is_all_zero() {
        let h = histogram(&ArrayData::F32(vec![]), 3, None);
        assert_eq!(h.num_rows(), 3);
        let count = h.column(2).as_any().downcast_ref::<UInt64Array>().unwrap();
        assert_eq!(count.values(), &[0, 0, 0]);
    }

    #[test]
    fn project_axis_reduces_along_an_axis() {
        // 2×3 array [[0,1,2],[10,11,12]] (row-major, shape [2,3]).
        let v = vec![0.0, 1.0, 2.0, 10.0, 11.0, 12.0];
        let shape = [2u64, 3];
        // Max over axis 0 (rows) → the max of each column: [10,11,12].
        let p = project_axis(&v, &shape, 0, ProjMode::Max);
        assert_eq!(p.shape, vec![3]);
        assert_eq!(p.values, vec![10.0, 11.0, 12.0]);
        // Sum over axis 1 (cols) → row sums: [3, 33].
        let p = project_axis(&v, &shape, 1, ProjMode::Sum);
        assert_eq!(p.shape, vec![2]);
        assert_eq!(p.values, vec![3.0, 33.0]);
        // Mean over axis 1 → [1, 11].
        let p = project_axis(&v, &shape, 1, ProjMode::Mean);
        assert_eq!(p.values, vec![1.0, 11.0]);
    }

    #[test]
    fn region_to_f64_applies_optional_rescale() {
        let d = ArrayData::I16(vec![0, 10, 20]);
        assert_eq!(region_to_f64(&d, None), vec![0.0, 10.0, 20.0]);
        // HU-style rescale: slope 1, intercept -1024.
        assert_eq!(
            region_to_f64(&d, Some((1.0, -1024.0))),
            vec![-1024.0, -1014.0, -1004.0]
        );
    }

    #[test]
    fn array_stats_reduces_min_max_mean_std() {
        let s = array_stats(&ArrayData::I16(vec![0, 2, 4, 6]));
        assert_eq!((s.min, s.max, s.count), (0.0, 6.0, 4));
        assert!((s.mean - 3.0).abs() < 1e-9 && (s.std - 5f64.sqrt()).abs() < 1e-9);
    }

    #[test]
    fn array_stats_empty_is_all_zero() {
        let s = array_stats(&ArrayData::F32(vec![]));
        assert_eq!(
            s,
            ArrayStats {
                min: 0.0,
                max: 0.0,
                mean: 0.0,
                std: 0.0,
                count: 0
            }
        );
    }
}
