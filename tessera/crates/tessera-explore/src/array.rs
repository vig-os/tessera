//! Derived views over an **array** block — the structured data the CLI `stats` (and later the TUI /
//! serve) render. Decode stays in [`tessera_io::array`]; this module computes summaries over the
//! decoded [`ArrayData`], returning typed structs rather than writing text.

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

#[cfg(test)]
mod tests {
    use super::*;

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
