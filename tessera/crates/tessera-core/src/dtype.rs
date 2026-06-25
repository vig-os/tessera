//! Supported array element dtypes.
//!
//! **int16 is the *recommended* representation for scanner-reconstructed CT/PET** (native
//! precision, lossless, ~2.6× smaller than float32 — see fd5 #192) — it is **not** the only
//! allowed dtype. Any dtype below is permitted. Computed float products (SUV / parametric /
//! TOFPET-lifetime / μ-maps) use `float32`/`float64`; counts/labels use the unsigned ints.

use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DType {
    Int8,
    Int16,
    Int32,
    Int64,
    UInt8,
    UInt16,
    UInt32,
    UInt64,
    Float16,
    Float32,
    Float64,
}

impl DType {
    /// Every supported dtype (the allowlist).
    pub const ALL: [DType; 11] = [
        DType::Int8,
        DType::Int16,
        DType::Int32,
        DType::Int64,
        DType::UInt8,
        DType::UInt16,
        DType::UInt32,
        DType::UInt64,
        DType::Float16,
        DType::Float32,
        DType::Float64,
    ];

    /// Canonical name as stored in the manifest (numpy-style, matches Zarr conventions).
    pub fn as_str(&self) -> &'static str {
        match self {
            DType::Int8 => "int8",
            DType::Int16 => "int16",
            DType::Int32 => "int32",
            DType::Int64 => "int64",
            DType::UInt8 => "uint8",
            DType::UInt16 => "uint16",
            DType::UInt32 => "uint32",
            DType::UInt64 => "uint64",
            DType::Float16 => "float16",
            DType::Float32 => "float32",
            DType::Float64 => "float64",
        }
    }

    pub fn is_integer(&self) -> bool {
        !matches!(self, DType::Float16 | DType::Float32 | DType::Float64)
    }

    pub fn byte_width(&self) -> usize {
        match self {
            DType::Int8 | DType::UInt8 => 1,
            DType::Int16 | DType::UInt16 | DType::Float16 => 2,
            DType::Int32 | DType::UInt32 | DType::Float32 => 4,
            DType::Int64 | DType::UInt64 | DType::Float64 => 8,
        }
    }

    /// True if `s` names a supported dtype.
    pub fn is_supported(s: &str) -> bool {
        DType::from_str(s).is_ok()
    }
}

impl FromStr for DType {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, ()> {
        DType::ALL.iter().copied().find(|d| d.as_str() == s).ok_or(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_recommended_and_rejection() {
        for d in DType::ALL {
            assert!(DType::is_supported(d.as_str()));
            assert_eq!(d.as_str().parse::<DType>().unwrap(), d);
        }
        assert!(DType::is_supported("int16")); // recommended for CT/PET
        assert!(DType::is_supported("float32")); // computed maps
        assert!(!DType::is_supported("float24")); // junk rejected
        assert_eq!(DType::Int16.byte_width(), 2);
        assert!(DType::Int16.is_integer());
        assert!(!DType::Float32.is_integer());
    }
}
