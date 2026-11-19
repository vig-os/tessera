//! The number rules a metadata [`FieldSpec`] declares (#895; ADR-0020 and ADR-0060 amendments of
//! 2026-11-19, ADR-0066 rule 7): a string **encoding** for a 64-bit integer, and an inclusive
//! **range**.
//!
//! The I-JSON bound ±(2^53−1) ([`MAX_EXACT_INTEGER`], SPEC §1) is a **format** invariant, never a
//! schema setting. A schema states how its fields meet it:
//! - A field whose `dtype` can hold a value beyond the bound (`int64`, `uint64`) declares
//!   `"encoding": "string"`, and its values are the integer's canonical decimal digits in a JSON
//!   string (the JSON-LD `xsd:long` pattern). The rule is the field's type, never a value's
//!   magnitude: `"7"` and `"1730000000000000000"` alike.
//! - A numeric field may declare `minimum` and/or `maximum` (inclusive, within the bound), and a
//!   value outside them is refused where it enters, naming the field and where the value came from.
//!
//! [`FieldSpec::check_definition`] checks a declaration, [`FieldSpec::check_value`] a value.

use std::str::FromStr;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::FieldSpec;
use crate::canonical::MAX_EXACT_INTEGER;
use crate::dtype::DType;

/// How a metadata field's values are spelled in the sealed JSON, when not as a JSON number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Encoding {
    /// The value is an integer of the field's `dtype` written as its canonical decimal digits in a
    /// JSON string: an optional `-`, then `0` or a digit `1`–`9` followed by digits. No `+`, no
    /// leading zero, no `-0`, no whitespace, so each integer has exactly one spelling. Declared by
    /// a field whose dtype can exceed ±(2^53−1) (`int64`, `uint64`), and only by such a field.
    String,
}

/// Why a metadata [`FieldSpec`] declaration is refused ([`FieldSpec::check_definition`]).
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum FieldDefinitionViolation {
    /// `dtype` is neither a [`DType`] name nor one of the metadata kinds `string`, `coded`, `json`,
    /// so nothing can be said about the values it allows.
    #[error(
        "dtype '{dtype}' is not a dtype (one of {}, or string, coded, json)",
        dtype_names()
    )]
    UnknownDtype { dtype: String },
    /// `dtype` can hold an integer beyond ±(2^53−1), which a sealed JSON number cannot hold
    /// exactly, and the field declares no string encoding.
    #[error(
        "dtype {dtype} can hold integers beyond ±(2^53−1), the range a sealed JSON number holds \
         exactly (SPEC §1, I-JSON RFC 7493 §2.2); declare \"encoding\": \"string\" to carry its \
         values as decimal strings"
    )]
    ExceedsBound { dtype: String },
    /// A string encoding on a dtype a JSON number already holds exactly.
    #[error(
        "declares \"encoding\": \"string\", which only a 64-bit integer dtype (int64, uint64) \
         takes; dtype {dtype} is held exactly by a JSON number"
    )]
    EncodingNotNeeded { dtype: String },
    /// A range on a field whose dtype is not numeric.
    #[error("declares a range, which only a numeric dtype takes; dtype is {dtype}")]
    RangeNotNumeric { dtype: String },
    /// A bound outside ±(2^53−1): the bound is the format's, and a schema cannot widen it.
    #[error(
        "declares {bound} = {value}, outside ±(2^53−1), the range a sealed JSON number holds \
         exactly (SPEC §1)"
    )]
    BoundBeyondFormat { bound: &'static str, value: f64 },
    /// `minimum` above `maximum`: no value could satisfy the field.
    #[error("declares minimum {minimum} above maximum {maximum}, which no value satisfies")]
    EmptyRange { minimum: f64, maximum: f64 },
    /// The field's `default` breaks its own encoding or range, so a product relying on it could
    /// never seal.
    #[error("declares a default that its own rules refuse: {violation}")]
    DefaultRefused { violation: FieldValueViolation },
    /// Two fields share an `id`: a value under it would answer to whichever rule a reader looks up
    /// first ([`crate::ProductSchema::check_definition`]).
    #[error("is declared more than once; a field id names one field")]
    DuplicateId,
}

/// Why a metadata value is refused by its [`FieldSpec`] ([`FieldSpec::check_value`]). `found` and
/// `value` are the value as compact JSON.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum FieldValueViolation {
    /// The field declares `"encoding": "string"` and the value is not the canonical decimal string
    /// of an integer of its dtype (nor a non-empty array of them).
    #[error(
        "{found} is not a decimal string of {dtype}: the field declares \"encoding\": \"string\", \
         so a value is a JSON string holding the integer's canonical decimal form (\"-12\", \
         \"1730000000000000000\"; no '+', no leading zero, no \"-0\")"
    )]
    NotDecimalString { dtype: String, found: String },
    /// The field declares a range and the value is not a number (nor a non-empty array of
    /// numbers), so it can't be checked against it.
    #[error(
        "{found} is not a number (or a non-empty array of numbers), which the field's declared \
         range requires"
    )]
    NotANumber { found: String },
    /// Below the field's inclusive `minimum`.
    #[error("{value} is below its minimum {minimum}")]
    BelowMinimum { value: String, minimum: f64 },
    /// Above the field's inclusive `maximum`.
    #[error("{value} is above its maximum {maximum}")]
    AboveMaximum { value: String, maximum: f64 },
}

/// The metadata kinds a [`FieldSpec::dtype`] may name besides a [`DType`].
const METADATA_KINDS: [&str; 3] = ["string", "coded", "json"];

fn dtype_names() -> String {
    DType::ALL
        .iter()
        .map(DType::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

/// What a field's dtype says about its values.
#[derive(Clone, Copy)]
enum Kind {
    /// A metadata kind (`string`, `coded`, `json`) or `bool`: no number rule applies.
    NotNumeric,
    /// A float dtype, or an integer dtype a double holds exactly.
    Number,
    /// `int64` / `uint64`: integers beyond the bound.
    Wide(DType),
}

fn kind_of(dtype: &str) -> Option<Kind> {
    if METADATA_KINDS.contains(&dtype) {
        return Some(Kind::NotNumeric);
    }
    Some(match DType::from_str(dtype).ok()? {
        DType::Bool => Kind::NotNumeric,
        d @ (DType::Int64 | DType::UInt64) => Kind::Wide(d),
        _ => Kind::Number,
    })
}

/// The exact `f64` of the bound (2^53 − 1 is a double).
const BOUND: f64 = MAX_EXACT_INTEGER as f64;

impl FieldSpec {
    /// Check this field's declaration against the number rules (#895).
    ///
    /// # Errors
    /// The first of: [`UnknownDtype`](FieldDefinitionViolation::UnknownDtype);
    /// [`ExceedsBound`](FieldDefinitionViolation::ExceedsBound) for `int64`/`uint64` without
    /// `"encoding": "string"`; [`EncodingNotNeeded`](FieldDefinitionViolation::EncodingNotNeeded)
    /// for an encoding on any other dtype; [`RangeNotNumeric`](FieldDefinitionViolation::RangeNotNumeric);
    /// [`BoundBeyondFormat`](FieldDefinitionViolation::BoundBeyondFormat) (minimum checked first);
    /// [`EmptyRange`](FieldDefinitionViolation::EmptyRange);
    /// [`DefaultRefused`](FieldDefinitionViolation::DefaultRefused) for a `default` the field's own
    /// encoding or range refuses.
    pub fn check_definition(&self) -> Result<(), FieldDefinitionViolation> {
        let dtype = || self.dtype.clone();
        let kind = kind_of(&self.dtype)
            .ok_or_else(|| FieldDefinitionViolation::UnknownDtype { dtype: dtype() })?;
        match (kind, self.encoding) {
            (Kind::Wide(_), None) => {
                return Err(FieldDefinitionViolation::ExceedsBound { dtype: dtype() })
            }
            (Kind::NotNumeric | Kind::Number, Some(Encoding::String)) => {
                return Err(FieldDefinitionViolation::EncodingNotNeeded { dtype: dtype() })
            }
            _ => {}
        }
        if self.minimum.is_none() && self.maximum.is_none() {
            return self.check_default();
        }
        if matches!(kind, Kind::NotNumeric) {
            return Err(FieldDefinitionViolation::RangeNotNumeric { dtype: dtype() });
        }
        for (bound, value) in [("minimum", self.minimum), ("maximum", self.maximum)] {
            if let Some(value) = value.filter(|v| !v.is_finite() || v.abs() > BOUND) {
                return Err(FieldDefinitionViolation::BoundBeyondFormat { bound, value });
            }
        }
        if let (Some(minimum), Some(maximum)) = (self.minimum, self.maximum) {
            if minimum > maximum {
                return Err(FieldDefinitionViolation::EmptyRange { minimum, maximum });
            }
        }
        self.check_default()
    }

    fn check_default(&self) -> Result<(), FieldDefinitionViolation> {
        match &self.default {
            Some(d) => self
                .check_value(d)
                .map_err(|violation| FieldDefinitionViolation::DefaultRefused { violation }),
            None => Ok(()),
        }
    }

    /// Check a metadata `value` against this field's encoding and range (#895). A field that
    /// declares neither accepts every value: what the dtype alone says is not checked here.
    ///
    /// With `"encoding": "string"`, `value` is the canonical decimal string of an integer of the
    /// dtype, or a non-empty array of them. With a range, `value` is a number, or a non-empty array
    /// of numbers, each within `[minimum, maximum]`; under a string encoding the integers are
    /// compared exactly. Call it on a field whose [`check_definition`](Self::check_definition)
    /// passed.
    ///
    /// # Errors
    /// [`NotDecimalString`](FieldValueViolation::NotDecimalString),
    /// [`NotANumber`](FieldValueViolation::NotANumber),
    /// [`BelowMinimum`](FieldValueViolation::BelowMinimum) or
    /// [`AboveMaximum`](FieldValueViolation::AboveMaximum), for the first offending element.
    pub fn check_value(&self, value: &Value) -> Result<(), FieldValueViolation> {
        let wide = match kind_of(&self.dtype) {
            Some(Kind::Wide(d)) if self.encoding == Some(Encoding::String) => Some(d),
            _ => None,
        };
        if wide.is_none() && self.minimum.is_none() && self.maximum.is_none() {
            return Ok(());
        }
        let items: &[Value] = match value {
            Value::Array(a) if !a.is_empty() => a,
            Value::Array(_) => return Err(self.not_this_kind(wide, value)),
            one => std::slice::from_ref(one),
        };
        for item in items {
            let n = match wide {
                Some(d) => {
                    Num::Int(decimal_of(d, item).ok_or_else(|| self.not_this_kind(wide, value))?)
                }
                None => Num::of(item).ok_or_else(|| self.not_this_kind(wide, value))?,
            };
            self.check_range(n, item)?;
        }
        Ok(())
    }

    fn not_this_kind(&self, wide: Option<DType>, value: &Value) -> FieldValueViolation {
        let found = shown(value);
        match wide {
            Some(_) => FieldValueViolation::NotDecimalString {
                dtype: self.dtype.clone(),
                found,
            },
            None => FieldValueViolation::NotANumber { found },
        }
    }

    fn check_range(&self, n: Num, item: &Value) -> Result<(), FieldValueViolation> {
        if let Some(minimum) = self.minimum.filter(|&m| n.below(m)) {
            return Err(FieldValueViolation::BelowMinimum {
                value: shown(item),
                minimum,
            });
        }
        if let Some(maximum) = self.maximum.filter(|&m| n.above(m)) {
            return Err(FieldValueViolation::AboveMaximum {
                value: shown(item),
                maximum,
            });
        }
        Ok(())
    }
}

/// `v` as the seal spells it (RFC 8785: `5000`, never `5000.0`), so a refusal shows the value the
/// person would find in the sealed manifest.
fn shown(v: &Value) -> String {
    serde_jcs::to_string(v).unwrap_or_else(|_| v.to_string())
}

/// A value to compare against a bound: an exact integer, or a double.
#[derive(Clone, Copy)]
enum Num {
    Int(i128),
    Float(f64),
}

impl Num {
    /// A JSON number as an exact integer when it is one (`as_i64`/`as_u64`), else its double.
    fn of(v: &Value) -> Option<Self> {
        let n = v.as_number()?;
        if let Some(i) = n.as_i64() {
            Some(Num::Int(i128::from(i)))
        } else if let Some(u) = n.as_u64() {
            Some(Num::Int(i128::from(u)))
        } else {
            n.as_f64().map(Num::Float)
        }
    }

    /// `self < bound`, exactly. A bound is within ±(2^53−1), so `ceil(bound)` is an exact integer
    /// and an integer is below `bound` exactly when it is below `ceil(bound)`.
    fn below(self, bound: f64) -> bool {
        match self {
            Num::Int(i) => i < bound.ceil() as i128,
            Num::Float(f) => f < bound,
        }
    }

    /// `self > bound`, exactly (`floor(bound)`, as in [`below`](Self::below)).
    fn above(self, bound: f64) -> bool {
        match self {
            Num::Int(i) => i > bound.floor() as i128,
            Num::Float(f) => f > bound,
        }
    }
}

/// The integer a canonical decimal string of `dtype` spells, or `None` when `v` is not one.
fn decimal_of(dtype: DType, v: &Value) -> Option<i128> {
    let s = v.as_str()?;
    let digits = s.strip_prefix('-').unwrap_or(s);
    let canonical = match digits.as_bytes() {
        [b'0'] => s.len() == 1, // "0" yes, "-0" no
        [b'1'..=b'9', rest @ ..] => rest.iter().all(u8::is_ascii_digit),
        _ => false,
    };
    if !canonical {
        return None;
    }
    // i128 parses up to 39 digits; `fits` then bounds the value to the dtype.
    let n: i128 = s.parse().ok()?;
    let fits = match dtype {
        DType::Int64 => i64::try_from(n).is_ok(),
        DType::UInt64 => u64::try_from(n).is_ok(),
        _ => false,
    };
    fits.then_some(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn field(dtype: &str) -> FieldSpec {
        FieldSpec::optional("f", "d", dtype)
    }

    #[test]
    fn a_bare_64_bit_field_is_refused_and_a_string_encoding_is_accepted() {
        for dtype in ["int64", "uint64"] {
            assert_eq!(
                field(dtype).check_definition(),
                Err(FieldDefinitionViolation::ExceedsBound {
                    dtype: dtype.into()
                })
            );
            assert_eq!(field(dtype).string_encoded().check_definition(), Ok(()));
        }
        for dtype in [
            "int32", "uint32", "float64", "string", "coded", "json", "bool",
        ] {
            assert_eq!(field(dtype).check_definition(), Ok(()), "{dtype}");
            assert_eq!(
                field(dtype).string_encoded().check_definition(),
                Err(FieldDefinitionViolation::EncodingNotNeeded {
                    dtype: dtype.into()
                }),
                "{dtype}"
            );
        }
        // A dtype the model doesn't know can't be judged, so it is refused, not passed.
        assert_eq!(
            field("i8").check_definition(),
            Err(FieldDefinitionViolation::UnknownDtype { dtype: "i8".into() })
        );
    }

    #[test]
    fn a_range_is_numeric_within_the_format_bound_and_not_empty() {
        assert_eq!(
            field("float64").range(0.0, 1000.0).check_definition(),
            Ok(())
        );
        assert_eq!(
            field("uint64")
                .string_encoded()
                .minimum(1.0)
                .check_definition(),
            Ok(())
        );
        let bound = MAX_EXACT_INTEGER as f64;
        assert_eq!(
            field("float64").range(-bound, bound).check_definition(),
            Ok(())
        );
        assert_eq!(
            field("string").maximum(1.0).check_definition(),
            Err(FieldDefinitionViolation::RangeNotNumeric {
                dtype: "string".into()
            })
        );
        assert_eq!(
            field("float64").maximum(bound + 2.0).check_definition(),
            Err(FieldDefinitionViolation::BoundBeyondFormat {
                bound: "maximum",
                value: bound + 2.0
            })
        );
        assert_eq!(
            field("float64").range(2.0, 1.0).check_definition(),
            Err(FieldDefinitionViolation::EmptyRange {
                minimum: 2.0,
                maximum: 1.0
            })
        );
    }

    #[test]
    fn a_string_encoded_value_is_the_canonical_decimal_of_its_dtype() {
        let u = field("uint64").string_encoded();
        let i = field("int64").string_encoded();
        for ok in ["0", "7", "1730000000000000000", "18446744073709551615"] {
            assert_eq!(u.check_value(&json!(ok)), Ok(()), "{ok}");
        }
        for ok in ["-9223372036854775808", "-1", "9223372036854775807"] {
            assert_eq!(i.check_value(&json!(ok)), Ok(()), "{ok}");
        }
        assert_eq!(u.check_value(&json!(["1", "2"])), Ok(()));
        for bad in [
            json!(7),
            json!("-0"),
            json!("+7"),
            json!("07"),
            json!(" 7"),
            json!("7.0"),
            json!("1e3"),
            json!(""),
            json!("-"),
            json!("18446744073709551616"),
            json!("-1"),
            json!([]),
            json!(["1", 2]),
            json!(null),
        ] {
            assert!(
                matches!(
                    u.check_value(&bad),
                    Err(FieldValueViolation::NotDecimalString { .. })
                ),
                "{bad}"
            );
        }
        assert!(i.check_value(&json!("9223372036854775808")).is_err());
    }

    #[test]
    fn a_range_refuses_out_of_range_values_and_keeps_in_range_ones() {
        let f = field("float64").range(0.0, 1000.0);
        for ok in [
            json!(0),
            json!(120),
            json!(120.5),
            json!(1000),
            json!([0.5, 0.5]),
        ] {
            assert_eq!(f.check_value(&ok), Ok(()), "{ok}");
        }
        assert_eq!(
            f.check_value(&json!(1000.5)),
            Err(FieldValueViolation::AboveMaximum {
                value: "1000.5".into(),
                maximum: 1000.0
            })
        );
        assert_eq!(
            f.check_value(&json!([1, -1])),
            Err(FieldValueViolation::BelowMinimum {
                value: "-1".into(),
                minimum: 0.0
            })
        );
        for bad in [json!("120"), json!([]), json!([[1]]), json!(null)] {
            assert!(
                matches!(
                    f.check_value(&bad),
                    Err(FieldValueViolation::NotANumber { .. })
                ),
                "{bad}"
            );
        }
        // An unranged, unencoded field checks nothing (the values it held before #895 still pass).
        assert_eq!(field("float64").check_value(&json!("anything")), Ok(()));
    }

    /// Integers are compared exactly, also against a fractional bound and next to 2^53.
    #[test]
    fn integers_are_compared_with_a_bound_exactly() {
        let bound = MAX_EXACT_INTEGER as f64;
        let wide = field("uint64").string_encoded().maximum(bound);
        assert_eq!(wide.check_value(&json!("9007199254740991")), Ok(()));
        // 2^53 is a double, and 2^53 + 1 rounds to it: a double comparison would accept it.
        assert!(wide.check_value(&json!("9007199254740993")).is_err());
        let half = field("int32").range(0.5, 2.5);
        assert!(half.check_value(&json!(0)).is_err());
        assert_eq!(half.check_value(&json!(1)), Ok(()));
        assert_eq!(half.check_value(&json!(2)), Ok(()));
        assert!(half.check_value(&json!(3)).is_err());
    }
}
