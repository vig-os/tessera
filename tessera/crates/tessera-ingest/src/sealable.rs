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
/// The `toml` parser keeps only the double, so the token is read from the source through the float's
/// span. Zero (`0.0`, `0e5`) and an exact denormal (`1e-320`) are not refused.
///
/// **Why a guided walk.** A span exists only for a value written as one token or one bracketed
/// construct. A table made by a dotted key (`a.b = 1`) or implicitly (`[a.b.c]` creates `a` and `a.b`)
/// has none, so deserializing every value as a `Spanned` fails on exactly those documents. The walk is
/// therefore guided by the parsed `toml::Value` tree: it asks for a span only where the tree has a zero
/// float, and walks every table, array and inline table as the plain map or sequence it is.
///
/// **Fails closed.** Callers have already parsed `text`, so a document this cannot inspect is an
/// internal fault, and it is refused as one; it is never passed.
///
/// # Errors
/// [`Error::Invalid`] naming `door`, the path (`product[0].metadata.x`) and the token, or naming an
/// internal fault when the document could not be inspected.
pub(crate) fn refuse_underflowing_toml_floats(door: &str, text: &str) -> Result<()> {
    use serde::de::{DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};
    use std::cell::RefCell;

    /// One position in the document: the parsed tree there, and the path to it.
    struct Guide<'a> {
        tree: &'a toml::Value,
        path: String,
        text: &'a str,
        /// The first underflowing float found: its path and token.
        found: &'a RefCell<Option<(String, String)>>,
    }
    impl Guide<'_> {
        fn child<'b>(&'b self, tree: &'b toml::Value, path: String) -> Guide<'b> {
            Guide {
                tree,
                path,
                text: self.text,
                found: self.found,
            }
        }
    }
    impl<'de> DeserializeSeed<'de> for Guide<'_> {
        type Value = ();
        fn deserialize<D: serde::Deserializer<'de>>(
            self,
            d: D,
        ) -> std::result::Result<(), D::Error> {
            match self.tree {
                toml::Value::Float(f) if *f == 0.0 => {
                    let at = toml::Spanned::<f64>::deserialize(d)?;
                    let token = self.text.get(at.span()).unwrap_or("").trim();
                    if underflows(token) && self.found.borrow().is_none() {
                        *self.found.borrow_mut() = Some((self.path, token.to_owned()));
                    }
                    Ok(())
                }
                toml::Value::Table(_) => d.deserialize_map(self),
                toml::Value::Array(_) => d.deserialize_seq(self),
                _ => IgnoredAny::deserialize(d).map(|_| ()),
            }
        }
    }
    impl<'de> Visitor<'de> for Guide<'_> {
        type Value = ();
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("the TOML table or array the parsed tree has here")
        }
        fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> std::result::Result<(), A::Error> {
            let toml::Value::Table(table) = self.tree else {
                return Err(serde::de::Error::custom("expected a table"));
            };
            while let Some(key) = a.next_key::<String>()? {
                let Some(child) = table.get(&key) else {
                    return Err(serde::de::Error::custom(format!(
                        "no key {key:?} in the tree"
                    )));
                };
                let path = if self.path.is_empty() {
                    key
                } else {
                    format!("{}.{key}", self.path)
                };
                a.next_value_seed(self.child(child, path))?;
            }
            Ok(())
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> std::result::Result<(), A::Error> {
            let toml::Value::Array(items) = self.tree else {
                return Err(serde::de::Error::custom("expected an array"));
            };
            for (i, child) in items.iter().enumerate() {
                let path = format!("{}[{i}]", self.path);
                if a.next_element_seed(self.child(child, path))?.is_none() {
                    return Err(serde::de::Error::custom("array shorter than the tree"));
                }
            }
            match a.next_element::<IgnoredAny>()? {
                None => Ok(()),
                Some(_) => Err(serde::de::Error::custom("array longer than the tree")),
            }
        }
    }
    use serde::Deserialize as _;

    let cannot_inspect = |why: String| {
        Error::Invalid(format!(
            "{door}: internal error: the underflow check could not inspect a TOML document the \
             parser accepted ({why}); refusing it rather than passing it unchecked"
        ))
    };
    let tree: toml::Value = toml::from_str(text).map_err(|e| cannot_inspect(e.to_string()))?;
    let found = RefCell::new(None);
    Guide {
        tree: &tree,
        path: String::new(),
        text,
        found: &found,
    }
    .deserialize(toml::de::Deserializer::new(text))
    .map_err(|e| cannot_inspect(e.to_string()))?;
    match found.into_inner() {
        Some((path, token)) => Err(refuse_underflow(door, &path, &token)),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::refuse_underflowing_toml_floats as check;

    /// The path and token the check refuses, or `None` when it accepts the document.
    fn refused(text: &str) -> Option<String> {
        check("t", text).err().map(|e| e.to_string())
    }

    /// Every valid TOML document is inspected, whatever shape its tables take: a plain table, a dotted
    /// key (any depth), an implicitly created table, an array of tables, an inline table inside a dotted
    /// key (#886 round 4). The span-aware parse used to fail on a table with no span of its own (a
    /// dotted key or an implicit table) and the check then returned `Ok`: `5e-400` sealed as `0.0`.
    #[test]
    fn an_underflow_is_refused_in_every_shape_of_toml_table() {
        let tiny = |path: &str| {
            format!(
                "invalid product: t: {path} = \"5e-400\" is too small for a double (it reads as 0, a \
                 different number), which a sealed JSON number cannot be"
            )
        };
        for (doc, path) in [
            // The four repros of the review.
            ("x = 5e-400\ndeep.z = 1\n", "x"),
            ("x = 5e-400\n[product.metadata.deep.deeper]\nz = 1\n", "x"),
            (
                "[product.metadata.deep.deeper]\ny = 5e-400\n",
                "product.metadata.deep.deeper.y",
            ),
            ("a.scale = 5e-400\n", "a.scale"),
            // A dotted key nested two levels deep.
            ("a.b.c = 5e-400\n", "a.b.c"),
            ("a.b.c = 1\na.b.d = 5e-400\n", "a.b.d"),
            // An array of tables.
            ("[[x]]\ny = 1\n[[x]]\ny = 5e-400\n", "x[1].y"),
            ("[[a.b]]\nc.d = 5e-400\n", "a.b[0].c.d"),
            // An inline table inside a dotted key, and arrays inside both.
            ("a.b = { c = 1, d = 5e-400 }\n", "a.b.d"),
            ("a.b = { c = [1, 5e-400] }\n", "a.b.c[1]"),
            ("a.b = [ { c = 5e-400 } ]\nz = 1\n", "a.b[0].c"),
        ] {
            assert_eq!(refused(doc), Some(tiny(path)), "{doc:?}");
        }
        // The token is quoted as the file spells it: signed, with TOML's digit separators.
        let msg = refused("x = -5e-4_00\n").unwrap();
        assert!(msg.contains("x = \"-5e-4_00\" is too small"), "{msg}");
    }

    /// In-range dotted and implicit tables, zero in any spelling and an exact denormal are accepted.
    #[test]
    fn in_range_toml_of_every_shape_is_accepted() {
        for doc in [
            "",
            "x = 1\n",
            "x = 0.0\ny = 0e5\nz = -0.0\nw = 1e-320\n",
            "a.b.c = 1\na.b.d = 0.0\n",
            "[product.metadata.deep.deeper]\nz = 1e-320\n",
            "[[x]]\ny = 1.5\n[[x]]\ny = 0e0\n",
            "a.b = { c = 1, d = [0.0, 1e-320] }\n",
            "s = \"5e-400 in a string\"\n# x = 5e-400 in a comment\n",
            "when = 1979-05-27T07:32:00Z\nn = 0\nok = true\n",
        ] {
            assert_eq!(refused(doc), None, "{doc:?}");
        }
    }

    /// The check fails closed: text the TOML parser does not accept is an error from the check itself,
    /// never `Ok` (its callers have parsed the document before, so this is an internal fault).
    #[test]
    fn a_document_the_check_cannot_inspect_is_refused_not_passed() {
        let msg = refused("x = = 5e-400").expect("an uninspectable document is refused");
        assert!(
            msg.starts_with(
                "invalid product: t: internal error: the underflow check could not inspect"
            ),
            "{msg}"
        );
    }
}
