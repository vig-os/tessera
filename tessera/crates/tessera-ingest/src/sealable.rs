//! The ingest door's refusal of a vendor number a seal cannot hold (SPEC §1, I-JSON RFC 7493 §2.2).
//!
//! A vendor's curated numeric fields stay JSON numbers when they fit (owner decision 2026-11-17). One
//! that cannot be held (NaN, ±infinity, or beyond ±(2^53−1) in magnitude) is refused at the format's
//! own door, before anything is written, in that format's own words: the tag and the element's text
//! for DICOM, the header field and its value for NIfTI. The person cannot rewrite the source file as a
//! string, so the error never advises it.
//!
//! One formatter serves every vendor door, so the wording cannot drift between them.

use tessera_core::canonical::{unsealable_f64, Unsealable};
use tessera_core::{Error, Result};

/// Refuse the first of `values` that a seal cannot hold, as `{door}: {field} = "{text}" …`.
///
/// `door` is the format's prefix (`dicom`, `nifti`), `field` the source field in the format's own
/// words (`(0028,1053) RescaleSlope`, `scl_slope`) and `text` the value as the source spells it.
///
/// # Errors
/// [`Error::Invalid`] naming `field` and `text` when a value is NaN, ±infinity or beyond
/// ±(2^53−1).
pub(crate) fn refuse_unsealable(door: &str, field: &str, text: &str, values: &[f64]) -> Result<()> {
    let Some(why) = values.iter().find_map(|&v| unsealable_f64(v)) else {
        return Ok(());
    };
    let reason = match why {
        Unsealable::NonFinite => {
            "is not a finite number (NaN, infinity, or past a double's range), which a sealed JSON \
             number cannot be (RFC 8259 §6)"
        }
        Unsealable::Beyond => {
            "is outside the range a sealed JSON number can hold (±(2^53−1), I-JSON RFC 7493 §2.2)"
        }
    };
    Err(Error::Invalid(format!(
        "{door}: {field} = \"{text}\" {reason}"
    )))
}
