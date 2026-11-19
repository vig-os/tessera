//! The ingest door's refusal of a vendor number a seal cannot hold (SPEC §1, I-JSON RFC 7493 §2.2).
//!
//! A vendor's curated numeric fields stay JSON numbers when they fit (owner decision 2026-11-17). One
//! that cannot be held (NaN, ±infinity, or beyond ±(2^53−1) in magnitude) is refused at the format's
//! own door, before anything is written, in that format's own words: the tag and the element's text
//! for DICOM, the header field and its value for NIfTI. The person cannot rewrite the source file as a
//! string, so the error never advises it.
//!
//! One formatter serves every vendor door, so the wording cannot drift between them.

use tessera_core::canonical::{underflows, unsealable_f64, Unsealable};
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
        // In range as a double, but a value that is not zero yet reads as 0.0 (`5e-400`) would seal
        // a different number. `text` may hold several `\`-separated values (DICOM `DS`).
        return match text.split('\\').map(str::trim).find(|t| underflows(t)) {
            Some(tiny) => Err(refuse_underflow(door, field, tiny)),
            None => Ok(()),
        };
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

/// The refusal of a number that is not zero but reads as 0.0, in the same shape as the others.
fn refuse_underflow(door: &str, field: &str, token: &str) -> Error {
    Error::Invalid(format!(
        "{door}: {field} = \"{token}\" is too small for a double (it reads as 0, a different number), \
         which a sealed JSON number cannot be"
    ))
}

/// Refuse a float in TOML `text` that is not zero but reads as 0.0 (`x = 5e-400`), naming its path.
///
/// The `toml` parser keeps only the double, so the token is read from the source through each
/// value's span. Zero (`0.0`, `0e5`) and an exact denormal (`1e-320`) are not refused.
///
/// # Errors
/// [`Error::Invalid`] naming `door`, the path (`product[0].metadata.x`) and the token.
pub(crate) fn refuse_underflowing_toml_floats(door: &str, text: &str) -> Result<()> {
    use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
    use toml::Spanned;

    /// The shape of a TOML value as far as its floats go; every other value is `Other`.
    enum Node {
        Float(f64),
        Seq(Vec<Spanned<Node>>),
        Map(Vec<(String, Spanned<Node>)>),
        Other,
    }
    struct NodeVisitor;
    impl<'de> Visitor<'de> for NodeVisitor {
        type Value = Node;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("a TOML value")
        }
        fn visit_f64<E>(self, v: f64) -> std::result::Result<Node, E> {
            Ok(Node::Float(v))
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> std::result::Result<Node, A::Error> {
            let mut out = Vec::new();
            while let Some(v) = a.next_element()? {
                out.push(v);
            }
            Ok(Node::Seq(out))
        }
        fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> std::result::Result<Node, A::Error> {
            let mut out = Vec::new();
            while let Some(k) = a.next_key::<String>()? {
                out.push((k, a.next_value()?));
            }
            Ok(Node::Map(out))
        }
        // Booleans, integers, strings and datetimes carry no float.
        fn visit_bool<E>(self, _: bool) -> std::result::Result<Node, E> {
            Ok(Node::Other)
        }
        fn visit_i64<E>(self, _: i64) -> std::result::Result<Node, E> {
            Ok(Node::Other)
        }
        fn visit_u64<E>(self, _: u64) -> std::result::Result<Node, E> {
            Ok(Node::Other)
        }
        fn visit_str<E>(self, _: &str) -> std::result::Result<Node, E> {
            Ok(Node::Other)
        }
    }
    impl<'de> Deserialize<'de> for Node {
        fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Node, D::Error> {
            d.deserialize_any(NodeVisitor)
        }
    }

    fn walk(door: &str, text: &str, path: &str, n: &Spanned<Node>) -> Result<()> {
        match n.get_ref() {
            Node::Float(f) if *f == 0.0 => {
                let token = text.get(n.span()).unwrap_or("").trim();
                if underflows(token) {
                    return Err(refuse_underflow(door, path, token));
                }
                Ok(())
            }
            Node::Seq(items) => items
                .iter()
                .enumerate()
                .try_for_each(|(i, v)| walk(door, text, &format!("{path}[{i}]"), v)),
            Node::Map(entries) => entries.iter().try_for_each(|(k, v)| {
                let at = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                walk(door, text, &at, v)
            }),
            _ => Ok(()),
        }
    }

    // A text that does not parse is the caller's own parse error to report.
    let Ok(root) = toml::from_str::<Spanned<Node>>(text) else {
        return Ok(());
    };
    walk(door, text, "", &root)
}
