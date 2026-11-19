//! Canonical JSON encoding (RFC 8785 JCS) — the one serialization Tessera hashes over.
//!
//! Any byte difference changes a content hash, so hashing must be over a *canonical* form:
//! lexicographically sorted object keys, no insignificant whitespace, ECMAScript-canonical
//! numbers. We use [`serde_jcs`] (RFC 8785). Reproducible by any language's JCS implementation
//! — the basis for cross-implementation + cross-version hash agreement (see ADR-0020).

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::Serialize;
use serde_json::{Map, Value};

use crate::field_path::FieldPath;

/// The largest integer magnitude a sealed JSON number may hold: 2^53 − 1, the bound below which an
/// IEEE-754 double represents every integer exactly (I-JSON, RFC 7493 §2.2).
pub const MAX_EXACT_INTEGER: u64 = (1 << 53) - 1;

/// Serialize `value` to canonical RFC 8785 JCS bytes.
///
/// Every sealed document is canonicalised here, so this is where a writer refuses input that is
/// not I-JSON: the value is checked by [`check_i_json`] before any byte is produced or hashed, so a
/// `u64`/`i64` from Rust code beyond [`MAX_EXACT_INTEGER`] fails instead of sealing approximately.
///
/// # Errors
/// [`crate::Error::InexactInteger`] for an integer beyond ±[`MAX_EXACT_INTEGER`];
/// [`crate::Error::Canonicalization`] if `value` has no JSON form.
pub fn to_bytes<T: Serialize>(value: &T) -> crate::Result<Vec<u8>> {
    let canon = |e: serde_json::Error| crate::Error::Canonicalization(e.to_string());
    check_i_json(&serde_json::to_value(value).map_err(canon)?)?;
    // Serialised from `value` itself, not from the checked `Value`: the bytes are exactly what they
    // were before the check existed (an `f32` field keeps its own shortest form).
    serde_jcs::to_vec(value).map_err(canon)
}

/// Refuse a JSON number that RFC 8785 cannot canonicalise exactly: an integer whose magnitude
/// exceeds [`MAX_EXACT_INTEGER`] (I-JSON, RFC 7493 §2.2, the input rule of RFC 8785 §3.1).
///
/// JCS writes every number as an IEEE-754 double (RFC 8785 §3.2.2.3), so `9007199254740993` and
/// `9007199254740992` canonicalise to the same bytes and one seal would verify both. A number is
/// refused when its value, as parsed, is such an integer. For a token that spells an integer
/// (`9007199254740993`, `9007199254740993.0`, `9.007199254740993e15`, `1e300`, `-9007199254740993`)
/// that is exactly "the token's integer is beyond ±(2^53−1)": rounding to the nearest double is
/// monotone and 2^53−1 and 2^53 are both doubles, so an integer token lands beyond the bound if and
/// only if it is beyond it. A token with a fractional part (`1.5`, `-0.0`) is accepted, unless it
/// rounds to a double beyond the bound (`9007199254740992.5` reads as `9007199254740992`): every
/// double there is an integer, so the fraction is already lost and the token collides the same
/// way; it is refused too. `-0` is the integer 0.
///
/// The error names the first offending number by its path from `value`'s root (`metadata.t_ns`,
/// `blocks[0].spec.fill_value`, the rendering [`crate::Error::NewerFormatFields`] uses).
///
/// # Errors
/// [`crate::Error::InexactInteger`] naming the path and the number.
pub fn check_i_json(value: &Value) -> crate::Result<()> {
    match inexact_integer(value, &FieldPath::root()) {
        None => Ok(()),
        Some((path, n)) => Err(crate::Error::InexactInteger {
            path: if path.is_root() {
                "the top-level value".to_owned()
            } else {
                path.to_string()
            },
            value: n.to_string(),
            // Only a free-form `metadata` field accepts a string; a typed field has none.
            as_string: path.has_key("metadata"),
        }),
    }
}

/// The first number in `value`, in key order, that [`check_i_json`] refuses, with its path.
fn inexact_integer<'a>(
    value: &'a Value,
    path: &FieldPath,
) -> Option<(FieldPath, &'a serde_json::Number)> {
    match value {
        Value::Number(n) if beyond_exact(n) => Some((path.clone(), n)),
        Value::Array(a) => a
            .iter()
            .enumerate()
            .find_map(|(i, v)| inexact_integer(v, &path.index(i))),
        Value::Object(o) => o.iter().find_map(|(k, v)| inexact_integer(v, &path.key(k))),
        _ => None,
    }
}

/// Whether the double `f` lies outside the range a sealed JSON number may hold: its magnitude
/// exceeds [`MAX_EXACT_INTEGER`] (or it is infinite). This is the rule [`check_i_json`] applies to
/// a parsed number; a reader of a typed source (a DICOM `DS`/`FD` value headed for a sealed field)
/// uses it to refuse such a value at its own door, where it can name the source.
///
/// A double above 2^53 − 1 in magnitude has no fractional bits, so "beyond" alone means "an integer
/// beyond", which two documents could not tell apart once JCS writes it (RFC 8785 §3.2.2.3).
#[must_use]
pub fn beyond_exact_f64(f: f64) -> bool {
    // Exact: 2^53 − 1 is a double. NaN compares false and is no JSON number at all.
    f.abs() > MAX_EXACT_INTEGER as f64
}

/// Why a double cannot be a sealed JSON number, from [`unsealable_f64`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unsealable {
    /// NaN or ±infinity: JSON (RFC 8259 §6) has no spelling for it, and `serde_json` writes it as `null`.
    NonFinite,
    /// Finite, but its magnitude exceeds [`MAX_EXACT_INTEGER`] (see [`beyond_exact_f64`]).
    Beyond,
}

/// Whether, and why, the double `f` cannot be a sealed JSON number: `None` when it can.
///
/// This is the one rule every f64→JSON conversion on a write path applies ([`json_number`],
/// [`to_json_value`]); a typed source's reader (DICOM, NIfTI) calls it directly to refuse a value at
/// its own door, where the error can name the tag or header field.
#[must_use]
pub fn unsealable_f64(f: f64) -> Option<Unsealable> {
    if !f.is_finite() {
        Some(Unsealable::NonFinite)
    } else if beyond_exact_f64(f) {
        Some(Unsealable::Beyond)
    } else {
        None
    }
}

/// The JSON number for the double `f`, or an error: the **only** way a write path turns an `f64`
/// into JSON.
///
/// `serde_json` maps a non-finite double to `null` (`Value::from(f64::NAN)`, `json!(f64::INFINITY)`,
/// `to_value`), so a seal would pin `null` where the person gave `inf`, with rc=0. This refuses it,
/// and a finite value beyond ±[`MAX_EXACT_INTEGER`], instead.
///
/// `what` names the value for the error.
///
/// # Errors
/// [`crate::Error::NonFiniteNumber`] for NaN or ±infinity; [`crate::Error::InexactInteger`] beyond
/// the bound.
pub fn json_number(what: &str, f: f64) -> crate::Result<Value> {
    match unsealable_f64(f) {
        None => Ok(Value::from(f)),
        Some(Unsealable::NonFinite) => Err(crate::Error::NonFiniteNumber {
            what: what.to_owned(),
            value: f.to_string(),
        }),
        Some(Unsealable::Beyond) => Err(crate::Error::InexactInteger {
            path: what.to_owned(),
            value: f.to_string(),
            as_string: false,
        }),
    }
}

/// `serde_json::to_value(value)` that refuses a non-finite number anywhere in `value`, the form every
/// write path uses to build the JSON it seals from a typed document (a block spec, a manifest, a
/// collection).
///
/// `to_value` writes NaN and ±infinity as `null`, which is indistinguishable from a field that was
/// `null` in the source, so the check has to look at the typed value, where the float is still a
/// float: a probe serializer walks it and fails on the first non-finite `f32`/`f64`, however deeply
/// nested. The returned value is `to_value`'s, unchanged.
///
/// # Errors
/// [`crate::Error::NonFiniteNumber`] when `value` holds NaN or ±infinity; [`crate::Error::Serde`] if
/// `value` has no JSON form.
pub fn to_json_value<T: Serialize>(value: &T) -> crate::Result<Value> {
    let v = serde_json::to_value(value)?;
    // `to_value` succeeded, so the document has a JSON form: the probe can fail only on a float.
    if value.serialize(finite::Probe::find()).is_err() {
        // Rare: walk it again, this time naming where.
        let found = value
            .serialize(finite::Probe::locate())
            .expect_err("the same value fails the same way");
        return Err(crate::Error::NonFiniteNumber {
            what: if found.path.is_empty() {
                "the top-level value".to_owned()
            } else {
                found.path
            },
            value: found.value,
        });
    }
    Ok(v)
}

/// A `serde::Serializer` that writes nothing and fails on the first non-finite `f32`/`f64`: the
/// check [`to_json_value`] runs on the typed value, where the float is still a float.
///
/// `serde_json` turns the float into `null` inside `to_value`, and `serde_jcs` refuses a non-finite
/// float only at the top level of a document, not nested in a field, so neither can be asked.
mod finite {
    use serde::ser::{
        Serialize, SerializeMap, SerializeSeq, SerializeStruct, SerializeStructVariant,
        SerializeTuple, SerializeTupleStruct, SerializeTupleVariant, Serializer,
    };

    /// The probe found a NaN or an infinity: where (a JSON path from the document root, empty at the
    /// root) and its text. A serializer `custom` error also lands here, which cannot arise for a value
    /// `to_value` already serialised.
    #[derive(Debug, PartialEq, Eq)]
    pub(super) struct NonFinite {
        pub(super) path: String,
        pub(super) value: String,
    }

    impl std::fmt::Display for NonFinite {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{} = {} is not finite", self.path, self.value)
        }
    }
    impl std::error::Error for NonFinite {}
    impl serde::ser::Error for NonFinite {
        fn custom<T: std::fmt::Display>(m: T) -> Self {
            NonFinite {
                path: String::new(),
                value: m.to_string(),
            }
        }
    }

    /// Walks a value, counting the elements of a sequence and holding the last map key. A tracking
    /// probe names every child the way the document spells it (`blocks[0].spec.scale`); an untracking
    /// one allocates nothing, so the common case, a finite document, costs one pass and no strings.
    pub(super) struct Probe {
        /// `None` when not tracking.
        path: Option<String>,
        next_index: usize,
        key: String,
    }

    impl Probe {
        /// The cheap probe: finds a non-finite number, names nothing.
        pub(super) fn find() -> Self {
            Probe {
                path: None,
                next_index: 0,
                key: String::new(),
            }
        }
        /// The naming probe, run again once [`Probe::find`] has failed.
        pub(super) fn locate() -> Self {
            Probe {
                path: Some(String::new()),
                ..Probe::find()
            }
        }
        fn child(&self, path: impl FnOnce(&str) -> String) -> Self {
            Probe {
                path: self.path.as_deref().map(path),
                next_index: 0,
                key: String::new(),
            }
        }
        fn field(&self, name: &str) -> Self {
            self.child(|p| {
                if p.is_empty() {
                    name.to_owned()
                } else {
                    format!("{p}.{name}")
                }
            })
        }
        fn element(&mut self) -> Self {
            let i = self.next_index;
            self.next_index += 1;
            self.child(|p| format!("{p}[{i}]"))
        }
        fn float(self, v: f64, text: impl FnOnce() -> String) -> Result<(), NonFinite> {
            if v.is_finite() {
                Ok(())
            } else {
                Err(NonFinite {
                    path: self.path.unwrap_or_default(),
                    value: text(),
                })
            }
        }
    }

    macro_rules! ignore {
        ($($f:ident($t:ty)),* $(,)?) => {$(
            fn $f(self, _: $t) -> Result<(), NonFinite> { Ok(()) }
        )*};
    }

    impl Serializer for Probe {
        type Ok = ();
        type Error = NonFinite;
        type SerializeSeq = Probe;
        type SerializeTuple = Probe;
        type SerializeTupleStruct = Probe;
        type SerializeTupleVariant = Probe;
        type SerializeMap = Probe;
        type SerializeStruct = Probe;
        type SerializeStructVariant = Probe;

        ignore!(
            serialize_bool(bool),
            serialize_i8(i8),
            serialize_i16(i16),
            serialize_i32(i32),
            serialize_i64(i64),
            serialize_u8(u8),
            serialize_u16(u16),
            serialize_u32(u32),
            serialize_u64(u64),
            serialize_char(char),
            serialize_str(&str),
            serialize_bytes(&[u8]),
        );
        fn serialize_f32(self, v: f32) -> Result<(), NonFinite> {
            self.float(f64::from(v), || v.to_string())
        }
        fn serialize_f64(self, v: f64) -> Result<(), NonFinite> {
            self.float(v, || v.to_string())
        }
        fn serialize_none(self) -> Result<(), NonFinite> {
            Ok(())
        }
        fn serialize_some<T: ?Sized + Serialize>(self, v: &T) -> Result<(), NonFinite> {
            v.serialize(self)
        }
        fn serialize_unit(self) -> Result<(), NonFinite> {
            Ok(())
        }
        fn serialize_unit_struct(self, _: &'static str) -> Result<(), NonFinite> {
            Ok(())
        }
        fn serialize_unit_variant(
            self,
            _: &'static str,
            _: u32,
            _: &'static str,
        ) -> Result<(), NonFinite> {
            Ok(())
        }
        fn serialize_newtype_struct<T: ?Sized + Serialize>(
            self,
            _: &'static str,
            v: &T,
        ) -> Result<(), NonFinite> {
            v.serialize(self)
        }
        fn serialize_newtype_variant<T: ?Sized + Serialize>(
            self,
            _: &'static str,
            _: u32,
            variant: &'static str,
            v: &T,
        ) -> Result<(), NonFinite> {
            // Externally tagged, as `serde_json` writes it: `{"Variant": v}`.
            v.serialize(self.field(variant))
        }
        fn serialize_seq(self, _: Option<usize>) -> Result<Probe, NonFinite> {
            Ok(self)
        }
        fn serialize_tuple(self, _: usize) -> Result<Probe, NonFinite> {
            Ok(self)
        }
        fn serialize_tuple_struct(self, _: &'static str, _: usize) -> Result<Probe, NonFinite> {
            Ok(self)
        }
        fn serialize_tuple_variant(
            self,
            _: &'static str,
            _: u32,
            variant: &'static str,
            _: usize,
        ) -> Result<Probe, NonFinite> {
            Ok(self.field(variant))
        }
        fn serialize_map(self, _: Option<usize>) -> Result<Probe, NonFinite> {
            Ok(self)
        }
        fn serialize_struct(self, _: &'static str, _: usize) -> Result<Probe, NonFinite> {
            Ok(self)
        }
        fn serialize_struct_variant(
            self,
            _: &'static str,
            _: u32,
            variant: &'static str,
            _: usize,
        ) -> Result<Probe, NonFinite> {
            Ok(self.field(variant))
        }
    }

    macro_rules! element_of_a_sequence {
        ($($tr:ident::$f:ident),* $(,)?) => {$(
            impl $tr for Probe {
                type Ok = ();
                type Error = NonFinite;
                fn $f<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<(), NonFinite> {
                    v.serialize(self.element())
                }
                fn end(self) -> Result<(), NonFinite> {
                    Ok(())
                }
            }
        )*};
    }
    element_of_a_sequence!(
        SerializeSeq::serialize_element,
        SerializeTuple::serialize_element,
        SerializeTupleStruct::serialize_field,
        SerializeTupleVariant::serialize_field,
    );

    impl SerializeMap for Probe {
        type Ok = ();
        type Error = NonFinite;
        fn serialize_key<T: ?Sized + Serialize>(&mut self, k: &T) -> Result<(), NonFinite> {
            // `to_value` already accepted this key, so it is a string or a number.
            self.key = match serde_json::to_value(k) {
                Ok(serde_json::Value::String(s)) => s,
                Ok(other) => other.to_string(),
                Err(_) => String::new(),
            };
            Ok(())
        }
        fn serialize_value<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<(), NonFinite> {
            v.serialize(self.field(&self.key))
        }
        fn end(self) -> Result<(), NonFinite> {
            Ok(())
        }
    }
    impl SerializeStruct for Probe {
        type Ok = ();
        type Error = NonFinite;
        fn serialize_field<T: ?Sized + Serialize>(
            &mut self,
            name: &'static str,
            v: &T,
        ) -> Result<(), NonFinite> {
            v.serialize(self.field(name))
        }
        fn end(self) -> Result<(), NonFinite> {
            Ok(())
        }
    }
    impl SerializeStructVariant for Probe {
        type Ok = ();
        type Error = NonFinite;
        fn serialize_field<T: ?Sized + Serialize>(
            &mut self,
            name: &'static str,
            v: &T,
        ) -> Result<(), NonFinite> {
            v.serialize(self.field(name))
        }
        fn end(self) -> Result<(), NonFinite> {
            Ok(())
        }
    }
}

/// Read one value a person typed for a sealed field (`tessera commit --set KEY=VALUE`,
/// `tessera ingest … --meta KEY=VALUE`) into JSON, refusing a number a seal cannot hold.
///
/// Whether the text is JSON is decided by the RFC 8259 grammar, never by whether `serde_json` can
/// hold every number in it (numbers in the grammar are unbounded):
/// - JSON text (`42`, `-1.5`, `1e300`, `true`, `null`, `"quoted"`, `[1,2]`, `{"a":1}`, surrounding
///   JSON whitespace allowed) is that value. A number in it beyond ±[`MAX_EXACT_INTEGER`] is
///   refused, and so is a number past a double's range (`1e400`, a 401-digit integer,
///   `[1e400]`), never kept as a string: the person wrote JSON, and a string would silently seal
///   something else. JSON text that is not I-JSON in another way (an escaped lone surrogate,
///   `"\ud800"`) is refused too.
/// - Text that is not JSON but that Rust's `str::parse::<f64>()` reads as a number (`+5`, `.5`, `5.`,
///   `01`, `inf`, `Infinity`, `NaN`, `+1e400`) is a number the person meant, spelled in a way JSON
///   does not allow. It is refused, naming the flag and key and saying why (non-finite, past a
///   double's range, or not JSON number spelling), never stored as a string they didn't ask for.
/// - Any other text (`hello`, `DUPLET-07`, `0x10`, `1_000`, `1.2.3`, `[draft]`) is a bare string.
///
/// To keep a number-shaped value as text, write it as a JSON string:
/// `--set k='"1730000000000000000"'` (or `--set k='"+5"'`).
///
/// `label` names the value in the error the way the person wrote it (`--set daq_start_ns`); a
/// number inside a composite is named by its path under it (`--set x.a[1]`). A bare number is
/// quoted as typed.
///
/// # Errors
/// [`crate::Error::InexactInteger`] for a number beyond ±[`MAX_EXACT_INTEGER`] or a bare number
/// past a double's range; [`crate::Error::Invalid`] for other JSON text that isn't I-JSON (a composite
/// holding a number past a double's range, a lone surrogate), with `serde_json`'s reason, and for
/// text read as a number that is not JSON number spelling (see above).
pub fn parse_user_value(label: &str, text: &str) -> crate::Result<Value> {
    // `IgnoredAny` makes serde_json check the grammar alone: it skips a number token without
    // computing its value, so `1e400` passes here and fails only the `Value` parse below.
    if serde_json::from_str::<de::IgnoredAny>(text).is_err() {
        // Not JSON. A text Rust reads as a number (`+5`, `.5`, `01`, `inf`, `NaN`, `+1e400`) is one
        // the person meant as a number, spelled in a way JSON does not allow: refuse it, never
        // store it as a string they didn't ask for. Anything else is a bare string.
        return match text.parse::<f64>() {
            Ok(f) => Err(refuse_number_text(label, text, TextNumber::of(text, f))),
            Err(_) => Ok(Value::String(text.to_owned())),
        };
    }
    let token = text.trim_matches([' ', '\t', '\n', '\r']);
    // A number token that is not zero but reads as ±0.0 would seal a different number (`5e-400`).
    // `serde_json` has already lost the token by the time it is a `Value`, so look at the text.
    if let Some(tiny) = underflowing_number_token(text) {
        return Err(refuse_number_text(label, tiny, TextNumber::Underflow));
    }
    let refuse = |path: String, value: String| crate::Error::InexactInteger {
        path,
        value,
        as_string: true,
    };
    let bare_number = token.starts_with(|c: char| c == '-' || c.is_ascii_digit());
    let value = match serde_json::from_str::<Value>(text) {
        Ok(value) => value,
        // A bare number token can fail only by its magnitude: it reads as an infinity.
        Err(_) if bare_number => {
            return Err(refuse_number_text(label, token, TextNumber::Overflow));
        }
        Err(e) => {
            return Err(crate::Error::Invalid(format!(
                "{label} = {token} is JSON a seal cannot hold (I-JSON, RFC 7493): {e}"
            )))
        }
    };
    match inexact_integer(&value, &FieldPath::root()) {
        None => Ok(value),
        Some((path, _)) if path.is_root() => Err(refuse(label.to_owned(), token.to_owned())),
        Some((path, n)) => {
            let rest = path.to_string();
            let sep = if rest.starts_with('[') { "" } else { "." };
            Err(refuse(format!("{label}{sep}{rest}"), n.to_string()))
        }
    }
}

/// Why a typed number text cannot be sealed, for [`refuse_number_text`]: the one message family every
/// refusal of a person's number text (`--set`, `--meta`) is written from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TextNumber {
    /// `NaN`, `inf`, `Infinity`: JSON has no spelling for them.
    NonFinite,
    /// A number past a double's range: it reads as infinity.
    Overflow,
    /// A non-zero number below a double's range: it reads as zero.
    Underflow,
    /// A number Rust reads but JSON does not spell (`+5`, `.5`, `5.`, `01`).
    Spelling,
}

impl TextNumber {
    /// Classify `text`, which is not JSON but which Rust read as the number `f`.
    fn of(text: &str, f: f64) -> Self {
        let lower = text.trim_start_matches(['+', '-']).to_ascii_lowercase();
        if matches!(lower.as_str(), "inf" | "infinity" | "nan") {
            TextNumber::NonFinite
        } else if f.is_infinite() {
            TextNumber::Overflow
        } else if f == 0.0 && underflows(text) {
            TextNumber::Underflow
        } else {
            TextNumber::Spelling
        }
    }
}

/// The refusal of a typed number text a seal cannot hold, naming the flag and key (`label`), the
/// text and why, and the way to store the text itself.
fn refuse_number_text(label: &str, text: &str, why: TextNumber) -> crate::Error {
    // `--set k` → `--set k='"text"'`: the way to store the text itself.
    let quoted = format!("{label}='\"{text}\"'");
    let why = match why {
        TextNumber::NonFinite => {
            "is not a finite number: JSON has no spelling for NaN or infinity, so a seal cannot hold it"
        }
        TextNumber::Overflow => {
            "is past a double's range (it reads as infinity), which a seal cannot hold"
        }
        TextNumber::Underflow => {
            "is too small for a double (it reads as 0, a different number), which a seal cannot hold"
        }
        TextNumber::Spelling => {
            "is a number, but not in JSON's number spelling (write `5`, not `+5`; no leading `+` or \
             zero, and no `.5` or `5.`)"
        }
    };
    crate::Error::Invalid(format!(
        "{label} = {text} {why}. Write it as a JSON number, or to store the text as it is, quote \
         it as a JSON string: {quoted}"
    ))
}

/// Whether a number token is not zero yet reads as ±0.0: it underflows a double (`5e-400`), so a
/// seal would pin a different number than the one given. A token that is zero (`0`, `0.0`, `-0`,
/// `0e5`) and one that is a denormal (`1e-320`, exact) are not underflow.
///
/// Digits may be separated by `_`, as TOML spells them.
#[must_use]
pub fn underflows(token: &str) -> bool {
    let clean: String = token.chars().filter(|&c| c != '_').collect();
    let mantissa = clean.split(['e', 'E']).next().unwrap_or("");
    mantissa.bytes().any(|b| (b'1'..=b'9').contains(&b))
        && clean.parse::<f64>().is_ok_and(|f| f == 0.0)
}

/// The first number token in JSON `text` that [`underflows`], skipping strings. `text` is valid
/// JSON, so a run of `0-9 + - . e E` outside a string is one number token.
fn underflowing_number_token(text: &str) -> Option<&str> {
    let b = text.as_bytes();
    let (mut i, mut in_string) = (0, false);
    while i < b.len() {
        match b[i] {
            b'\\' if in_string => i += 1,
            b'"' => in_string = !in_string,
            c if !in_string && (c == b'-' || c.is_ascii_digit()) => {
                let end = b[i..]
                    .iter()
                    .position(|c| !matches!(c, b'0'..=b'9' | b'+' | b'-' | b'.' | b'e' | b'E'))
                    .map_or(b.len(), |n| i + n);
                if underflows(&text[i..end]) {
                    return Some(&text[i..end]);
                }
                i = end;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Whether `n` is an integer beyond ±[`MAX_EXACT_INTEGER`]. A double above 2^53 − 1 in magnitude
/// has no fractional bits, so for the `f64` form "beyond" alone means "an integer beyond".
fn beyond_exact(n: &serde_json::Number) -> bool {
    if let Some(u) = n.as_u64() {
        u > MAX_EXACT_INTEGER
    } else if let Some(i) = n.as_i64() {
        i.unsigned_abs() > MAX_EXACT_INTEGER
    } else {
        // Exact: 2^53 − 1 is a double. `as_f64` is `Some` for every number without
        // `arbitrary_precision`, which this workspace doesn't enable. The token-to-double step must
        // round correctly, as Python's `float` does, or the readers disagree near the bound: that is
        // serde_json's `float_roundtrip`, which `serde_jcs` enables for the whole build
        // (`numbers_a_double_holds_exactly_are_accepted` pins it with `…991.4999…`).
        n.as_f64().is_some_and(beyond_exact_f64)
    }
}

/// Canonical bytes of `value`, hashed with blake3 → `"blake3:<hex>"`.
pub fn hash<T: Serialize>(value: &T) -> crate::Result<String> {
    Ok(crate::hash::digest(&to_bytes(value)?))
}

/// The digest of a JSON **object** with exactly one key removed: `digest(JCS(object − key))`.
///
/// This is how a seal is computed over the object a reader *received* (SPEC §4): no other key is
/// added, dropped or normalised, so unknown keys, an explicit `null` and an omitted default all stay
/// exactly as they arrived. Writer and reader call this one function, so the seal a writer stamps is
/// by construction the one a reader recomputes over the writer's output.
///
/// # Errors
/// [`crate::Error::Canonicalization`] if `object` is not a JSON object.
pub fn hash_without(object: &Value, key: &str) -> crate::Result<String> {
    Ok(crate::hash::digest(&bytes_without(object, key)?))
}

/// The canonical bytes [`hash_without`] hashes: `JCS(object − key)`.
///
/// # Errors
/// [`crate::Error::Canonicalization`] if `object` is not a JSON object.
pub fn bytes_without(object: &Value, key: &str) -> crate::Result<Vec<u8>> {
    let Value::Object(map) = object else {
        return Err(crate::Error::Canonicalization(format!(
            "a seal is computed over a JSON object, got {}",
            json_type(object)
        )));
    };
    let mut bare = map.clone();
    bare.remove(key);
    to_bytes(&bare)
}

/// Parse JSON text into a [`Value`], **refusing a duplicate key** in any object at any depth.
///
/// RFC 8259 §4 only says names *SHOULD* be unique and leaves the meaning of a repeat to the parser;
/// RFC 8785 (JCS) §3.1 requires its input to be I-JSON (RFC 7493 §2.3: names MUST be unique). Plain
/// `serde_json` keeps the last repeat and Python's `json` does too, but other parsers keep the first,
/// so two readers would see different values under one seal. Hashed input is therefore refused
/// rather than guessed at. Keys compare after unescaping (`"a"` and `"\u0061"` are the same key).
///
/// The rest of I-JSON's MUSTs (RFC 7493 §2) are enforced here too, so every sealed document a
/// reader accepts is valid JCS input:
/// - §2.1 UTF-8 only, no unpaired surrogate: the text is a `&str` (the bytes
///   [`check_canonical`] reads are validated as UTF-8 by the same parser), and `serde_json` refuses
///   an escaped lone surrogate (`"\ud800"`) in a string or key;
/// - §2.2 numbers: [`check_i_json`] refuses an integer beyond ±(2^53−1); `NaN`/`Infinity` aren't
///   JSON and a number beyond a double's range (`1e400`) is a parse error;
/// - §2.3 unique names: the duplicate-key refusal above.
///
/// # Errors
/// [`crate::Error::Serde`] for malformed JSON, a lone surrogate or a duplicate key (the message
/// names the key); [`crate::Error::InexactInteger`] for an integer beyond ±(2^53−1).
pub fn parse_strict(s: &str) -> crate::Result<Value> {
    parse_strict_bytes(s.as_bytes())
}

/// [`parse_strict`] over bytes: the one strict parser. `serde_json`'s slice reader also refuses
/// bytes that are not UTF-8 (in a string, or anywhere else), which a `&str` can't hold.
fn parse_strict_bytes(bytes: &[u8]) -> crate::Result<Value> {
    let mut de = serde_json::Deserializer::from_slice(bytes);
    let StrictValue(v) = StrictValue::deserialize(&mut de)?;
    de.end()?;
    check_i_json(&v)?;
    Ok(v)
}

/// Parse the **stored bytes** of a sealed JSON document strictly and refuse them unless they are
/// exactly their own RFC 8785 (JCS) form; on success, the parsed value (ADR-0066 rules 1 and 4).
///
/// Under ADR-0066 a seal is `blake3` over the stored bytes, so the bytes must be the one spelling
/// of their value: with this check, one seal has one byte string and one meaning in every reader.
/// The seal alone catches every edit made after sealing, but not a respelling the attacker seals
/// (`64.0` for `64`, a pretty layout), on which readers disagree (spike #897, case S7).
///
/// Two steps, in this order:
///
/// 1. **Strict parse**, [`parse_strict`]'s rules: bytes that are not UTF-8, malformed JSON, a
///    duplicate key, a lone surrogate (`"\ud800"`), nesting deeper than `serde_json`'s recursion
///    limit (127 nested arrays or objects are accepted, 128 refused), and an integer beyond
///    ±[`MAX_EXACT_INTEGER`] in any spelling ([`check_i_json`]) are all refused.
/// 2. **Byte comparison**: the parsed value is serialised with [`to_bytes`] and compared with the
///    input; the first differing byte is reported. This refuses every other spelling of a value:
///    key order, whitespace (a trailing newline included), `\u` escapes JCS doesn't write
///    (`\u0061`, `\/`), and number spellings other than ECMAScript's shortest form (`64.0` for
///    `64`, `1E2` for `100`, `-0` for `0`, `0.0000001` for `1e-7`).
///
/// Step 1 is not redundant with step 2. An integer beyond 2^53 that a double holds exactly
/// (`9007199254740992`, `1730000000123456800`) is its own JCS form, so the byte comparison alone
/// would accept it; it is what a writer that rounded `1730000000123456789` emits, so it is refused
/// by I-JSON in step 1 (ADR-0066 rule 7, spike case S5b).
///
/// The check is over any JSON value; the callers ([`crate::Manifest::from_stored`],
/// [`crate::Collection::from_stored`]) then require an object.
///
/// # Errors
/// - [`crate::Error::Serde`] for bytes that are not UTF-8, malformed JSON, a duplicate key, a lone
///   surrogate or excess nesting;
/// - [`crate::Error::InexactInteger`] for an integer beyond ±(2^53−1), naming its path;
/// - [`crate::Error::NonCanonical`] for valid I-JSON that is not its own JCS form, naming the
///   offset of the first differing byte.
pub fn check_canonical(bytes: &[u8]) -> crate::Result<Value> {
    let value = parse_strict_bytes(bytes)?;
    let jcs = to_bytes(&value)?;
    if let Some(offset) = first_difference(bytes, &jcs) {
        return Err(crate::Error::NonCanonical {
            offset,
            found: excerpt(bytes, offset),
            canonical: excerpt(&jcs, offset),
        });
    }
    Ok(value)
}

/// The index of the first byte where `a` and `b` differ, or the shorter length when one is a strict
/// prefix of the other; `None` when they are equal.
fn first_difference(a: &[u8], b: &[u8]) -> Option<usize> {
    if a == b {
        return None;
    }
    Some(
        a.iter()
            .zip(b)
            .position(|(x, y)| x != y)
            .unwrap_or(a.len().min(b.len())),
    )
}

/// Up to 16 bytes of `bytes` from `at`, ASCII-escaped for an error message (empty past the end).
fn excerpt(bytes: &[u8], at: usize) -> String {
    let end = bytes.len().min(at.saturating_add(16));
    bytes.get(at..end).unwrap_or(&[]).escape_ascii().to_string()
}

/// Deserialize a parsed value into the typed view `T`, **refusing any spelling but the declared
/// one**: a struct only as a JSON object, a unit-variant name only as a string (SPEC §4 "objects
/// only, one spelling per value"). `newer` is the declared-field walk's report for `v`: the paths of
/// the keys whose parent type doesn't declare them (ADR-0060 §2.2), as structured paths, so a key is
/// exempt only at the exact place the walk found it and never by how its path happens to be spelled.
///
/// serde accepts two spellings the sealed formats don't define. It reads a struct from a JSON array
/// positionally (`["t","u8"]` for `{"name":"t","dtype":"u8"}`), and a unit enum variant from a
/// one-key object (`{"table": null}` for `"table"`). Either is a second spelling of one sealed
/// value, and an array's "keys" are invisible to the declared-field walk, so an unknown rule inside
/// an array-form parent would be dropped instead of refused. So after `T` is built, its own
/// serialization is walked alongside `v`, **key by key of `v`**:
///
/// - a key `T` wrote back: wherever either side holds an object or an array, the other must hold
///   the same kind (this covers every type `T` is made of, internally tagged and untagged enums
///   included, without a per-type list);
/// - a key `T` did not write back and the walk named in `newer`: a newer writer's field, left to
///   the caller (verified, warned about, never re-sealed). The walk names a key only where its
///   parent's declared field set lacks it, and paths compare segment by segment, so a declared key
///   is never exempt: a crafted key literally named `members[0].kind` is a different path from the
///   first member's `kind`;
/// - any other key `T` did not write back is a declared key whose value `T` reduced to a default
///   its serializer skips (`skip_serializing_if`: `None`, an empty list or map, `false`, a default
///   unit variant such as a member `kind` of `product`). A non-empty object or array there is
///   refused (`{"product": null}` for `"product"`). What is left (`null`, a boolean, a number, a
///   string, `[]`, `{}`) is the default's own spelling, because serde coerces nothing between
///   those: only `null` reads as `None`, `false` as `false`, `[]` as an empty list, `{}` as an
///   empty map and the variant's name as the unit variant. The explicit default itself
///   (`"study": null`, `"kind": "product"`, `"derived_from": []`) is accepted: SPEC §4 seals the
///   object as received, so it is a present key, hashed as received, and not a newer field.
///
/// Opaque values (`serde_json::Value` fields) serialize as received, so they never differ.
///
/// Manifest, collection and embedded product schema all deserialize through this.
///
/// # Errors
/// A [`serde_json::Error`]: `T`'s own deserialization error (a known field of the wrong type), or
/// one naming the first value in an undeclared spelling by its path, rendered as
/// [`crate::field_path`] defines (`blocks[0]`, `tables.events`, `members[0].role`,
/// `tables["events.v2"].order`).
pub(crate) fn from_declared_form<T>(v: &Value, newer: &[FieldPath]) -> Result<T, serde_json::Error>
where
    T: serde::de::DeserializeOwned + Serialize,
{
    let typed = T::deserialize(v)?;
    let written = serde_json::to_value(&typed)?;
    if let Some((path, got, declared)) = form_mismatch(v, Some(&written), newer, FieldPath::root())
    {
        let at = if path.is_root() {
            "the top-level value".to_owned()
        } else {
            format!("`{path}`")
        };
        return Err(de::Error::custom(format!(
            "{at} is {} where the format defines {declared} (one spelling per value: a record is \
             an object, never an array; a name is a string, never an object)",
            json_type(got),
        )));
    }
    Ok(typed)
}

/// The first place, by path, where `received` is not in the spelling `typed` (the
/// serialization of the view built from `received`) declares, with the received value and what the
/// format defines there. `typed` is `None` for a key the serializer skipped: see
/// [`from_declared_form`] for the rule, and `newer` for the keys exempt from it.
fn form_mismatch<'a>(
    received: &'a Value,
    typed: Option<&Value>,
    newer: &[FieldPath],
    path: FieldPath,
) -> Option<(FieldPath, &'a Value, &'static str)> {
    let Some(typed) = typed else {
        return match received {
            Value::Object(o) if !o.is_empty() => Some((path, received, SKIPPED_DEFAULT)),
            Value::Array(a) if !a.is_empty() => Some((path, received, SKIPPED_DEFAULT)),
            _ => None,
        };
    };
    match (received, typed) {
        (Value::Object(r), Value::Object(t)) => r.iter().find_map(|(k, rv)| {
            let at = path.key(k);
            // Exempt only the key the walk found undeclared at exactly this place: paths compare
            // segment by segment, so no key name can stand in for another key's path.
            match t.get(k) {
                None if newer.contains(&at) => None,
                tv => form_mismatch(rv, tv, newer, at),
            }
        }),
        (Value::Array(r), Value::Array(t)) => r
            .iter()
            .zip(t)
            .enumerate()
            .find_map(|(i, (rv, tv))| form_mismatch(rv, Some(tv), newer, path.index(i))),
        (Value::Object(_) | Value::Array(_), _) | (_, Value::Object(_) | Value::Array(_)) => {
            Some((path, received, json_type(typed)))
        }
        _ => None,
    }
}

/// What the format defines for a key whose value the serializer skips as a default.
const SKIPPED_DEFAULT: &str = "a default, spelled as null, a boolean, a number, a string, [] or {}";

fn json_type(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// A [`Value`] whose deserialisation refuses duplicate object keys (see [`parse_strict`]).
struct StrictValue(Value);

impl<'de> Deserialize<'de> for StrictValue {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(StrictVisitor).map(StrictValue)
    }
}

struct StrictVisitor;

impl<'de> Visitor<'de> for StrictVisitor {
    type Value = Value;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("any JSON value")
    }
    fn visit_unit<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_bool<E>(self, b: bool) -> Result<Value, E> {
        Ok(Value::Bool(b))
    }
    fn visit_i64<E>(self, n: i64) -> Result<Value, E> {
        Ok(n.into())
    }
    fn visit_u64<E>(self, n: u64) -> Result<Value, E> {
        Ok(n.into())
    }
    // Same number model as `serde_json::Value` (JSON has no non-finite numbers, so `from_f64` never
    // yields `Null` here): this parser changes nothing about how numbers canonicalise (#587).
    fn visit_f64<E>(self, n: f64) -> Result<Value, E> {
        Ok(serde_json::Number::from_f64(n).map_or(Value::Null, Value::Number))
    }
    fn visit_str<E>(self, s: &str) -> Result<Value, E> {
        Ok(Value::String(s.to_owned()))
    }
    fn visit_string<E>(self, s: String) -> Result<Value, E> {
        Ok(Value::String(s))
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut out = Vec::new();
        while let Some(StrictValue(v)) = seq.next_element()? {
            out.push(v);
        }
        Ok(Value::Array(out))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut out = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if out.contains_key(&key) {
                return Err(de::Error::custom(format!(
                    "duplicate key {key:?} in a JSON object (refused: hashed JSON must be I-JSON)"
                )));
            }
            let StrictValue(v) = map.next_value()?;
            out.insert(key, v);
        }
        Ok(Value::Object(out))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn key_order_is_irrelevant_to_canonical_bytes() {
        // Two JSON objects with the same content but different key insertion order must
        // canonicalize to identical bytes (and thus identical hashes).
        let a = json!({ "b": 1, "a": 2, "nested": { "y": 1, "x": 2 } });
        let b = json!({ "nested": { "x": 2, "y": 1 }, "a": 2, "b": 1 });
        assert_eq!(super::to_bytes(&a).unwrap(), super::to_bytes(&b).unwrap());
        assert_eq!(super::hash(&a).unwrap(), super::hash(&b).unwrap());
        // and it really is sorted, whitespace-free
        assert_eq!(
            String::from_utf8(super::to_bytes(&a).unwrap()).unwrap(),
            r#"{"a":2,"b":1,"nested":{"x":2,"y":1}}"#
        );
    }

    fn refused_at(text: &str) -> (String, String) {
        match super::parse_strict(text) {
            Err(crate::Error::InexactInteger { path, value, .. }) => (path, value),
            other => panic!("{text}: expected InexactInteger, got {other:?}"),
        }
    }

    /// The defect (#886): both integers canonicalise to the same bytes, so a seal over one
    /// verified the other. Each is now refused instead, in every spelling of an integer.
    #[test]
    fn integers_beyond_2_pow_53_collide_under_jcs_and_are_refused() {
        let a = super::to_bytes(&json!(9_007_199_254_740_992_u64 - 1)).unwrap();
        assert_eq!(
            a, b"9007199254740991",
            "the largest exact integer is sealed"
        );
        for token in [
            "9007199254740992",
            "9007199254740993",
            "-9007199254740993",
            "9007199254740993.0",
            "9.007199254740993e15",
            "90071992547409930e-1",
            "1e300",
            "-1E+300",
            "18446744073709551616",
            "9007199254740992.5",
            "9007199254740991.5",
        ] {
            let (path, _) = refused_at(&format!(r#"{{"metadata":{{"t":[0,{token}]}}}}"#));
            assert_eq!(path, "metadata.t[1]", "{token}");
        }
        assert_eq!(refused_at("9007199254740993").0, "the top-level value");
    }

    #[test]
    fn numbers_a_double_holds_exactly_are_accepted() {
        for token in [
            "9007199254740991",
            "-9007199254740991",
            "9007199254740991.0",
            "9.007199254740991e15",
            "-0",
            "-0.0",
            "1.5",
            "1e-300",
            "0.1",
            "4503599627370495.5",
            // Correctly rounded down to 2^53 − 1; an approximate parser rounds it up to 2^53.
            "9007199254740991.4999999999999999999999",
        ] {
            let v = super::parse_strict(token).unwrap_or_else(|e| panic!("{token}: {e}"));
            super::to_bytes(&v).unwrap_or_else(|e| panic!("{token}: {e}"));
        }
    }

    /// A writer refuses before any byte is hashed, whatever Rust type holds the integer.
    #[test]
    fn writers_refuse_an_inexact_integer_from_rust_values() {
        #[derive(serde::Serialize)]
        struct Doc {
            id_inputs: std::collections::BTreeMap<String, i64>,
        }
        let doc = Doc {
            id_inputs: [("daq_start_ns".to_owned(), 1_730_000_000_000_000_000)].into(),
        };
        let e = super::hash(&doc).unwrap_err();
        assert_eq!(
            e.to_string(),
            "id_inputs.daq_start_ns = 1730000000000000000 is outside the range a sealed JSON \
             number can hold (±(2^53−1), I-JSON RFC 7493 §2.2): a sealed JSON number is an \
             IEEE-754 double (RFC 8785 §3.2.2.3), which can't hold this integer exactly"
        );
        // A typed field has no string form, so no "as a string" advice. A free-form `metadata`
        // field does.
        assert!(!e.to_string().contains("as a string"), "{e}");
        let free = super::hash(&json!({ "metadata": { "t": 9_007_199_254_740_993_u64 } }));
        assert!(
            free.unwrap_err()
                .to_string()
                .ends_with("; write it as a string"),
            "a free-form metadata field keeps the advice"
        );
        for v in [json!(u64::MAX), json!(i64::MIN), json!(1e300_f64)] {
            let object = json!({ "manifest_hash": "x", "extra": { "v": v } });
            assert!(
                matches!(
                    super::hash_without(&object, "manifest_hash"),
                    Err(crate::Error::InexactInteger { ref path, .. }) if path == "extra.v"
                ),
                "{v}"
            );
        }
    }

    /// Typed JSON is JSON: a number beyond the bound or past a double's range is refused, naming
    /// the person's label and their own text, never kept as a string (#886 review).
    #[test]
    fn a_typed_number_a_seal_cannot_hold_is_refused_not_stringified() {
        let refused = |text: &str| match super::parse_user_value("--set x", text) {
            Err(crate::Error::InexactInteger { path, value, .. }) => (path, value),
            other => panic!("{text}: expected InexactInteger, got {other:?}"),
        };
        for text in [
            "9007199254740992",
            "-9007199254740993",
            "9007199254740993.0",
            "9.007199254740993e15",
            "1E300",
        ] {
            assert_eq!(refused(text), ("--set x".into(), text.into()), "{text}");
        }
        // Surrounding JSON whitespace doesn't make it text; the error quotes the token.
        assert_eq!(refused(" 9007199254740993\n").1, "9007199254740993");
        // Inside a composite the error names the path under the label.
        assert_eq!(
            refused(r#"{"a":[1,9007199254740993]}"#),
            ("--set x.a[1]".into(), "9007199254740993".into())
        );
        assert_eq!(refused("[1e300]").0, "--set x[0]");
        // A bare number past a double's range is refused in the same words as `+1e400`, from one
        // helper, and says what it is: a number that reads as infinity, not an inexact integer.
        let big = "9".repeat(401);
        for text in [big.as_str(), "1e400", "-1e400", "1E+999"] {
            match super::parse_user_value("--set x", text) {
                Err(crate::Error::Invalid(m)) => assert_eq!(
                    m,
                    format!(
                        "--set x = {text} is past a double's range (it reads as infinity), which \
                         a seal cannot hold. Write it as a JSON number, or to store the text as it \
                         is, quote it as a JSON string: --set x='\"{text}\"'"
                    )
                ),
                other => panic!("{text}: expected Invalid, got {other:?}"),
            }
        }
        // JSON past a double's range inside a composite is refused too, never kept as a string.
        for text in ["[1e400]", r#"{"a":[-1e400]}"#, r#""\ud800""#] {
            match super::parse_user_value("--set x", text) {
                Err(crate::Error::Invalid(m)) => {
                    assert!(m.starts_with(&format!("--set x = {text} is JSON")), "{m}")
                }
                other => panic!("{text}: expected Invalid, got {other:?}"),
            }
        }
    }

    /// A non-zero number that reads as ±0.0 would seal a different number: refused in every JSON
    /// position. Zero in any spelling, and a denormal (exact), stay.
    #[test]
    fn a_number_that_underflows_to_zero_is_refused_and_zero_and_denormals_are_kept() {
        for text in [
            "5e-400",
            "-5e-400",
            "1e-1000",
            r#"{"a":5e-400}"#,
            "[1,[2e-999]]",
        ] {
            match super::parse_user_value("--meta k", text) {
                Err(crate::Error::Invalid(m)) => {
                    assert!(
                        m.contains("is too small for a double (it reads as 0"),
                        "{text}: {m}"
                    );
                    assert!(m.contains("--meta k"), "{m}");
                }
                other => panic!("{text}: expected a refusal, got {other:?}"),
            }
        }
        for text in [
            "0",
            "0.0",
            "-0",
            "0e5",
            "0.000e-9999",
            "1e-320",
            "5e-324",
            "[0e5,1e-320]",
        ] {
            super::parse_user_value("--meta k", text)
                .unwrap_or_else(|e| panic!("{text} must be accepted: {e}"));
        }
        assert!(super::underflows("5e-400") && super::underflows("4_9e-4_00"));
        assert!(!super::underflows("0e5") && !super::underflows("1e-320"));
    }

    /// Everything else keeps its meaning: numbers in range, other JSON, and plain text as a string.
    #[test]
    fn a_typed_value_in_range_or_not_a_number_is_kept() {
        let v = |text: &str| super::parse_user_value("--set x", text).unwrap();
        assert_eq!(v("9007199254740991"), json!(9_007_199_254_740_991_u64));
        assert_eq!(v("-9007199254740991"), json!(-9_007_199_254_740_991_i64));
        assert_eq!(v("1.5"), json!(1.5));
        assert_eq!(
            v("-0"),
            json!(-0.0),
            "serde_json reads `-0` as the double -0.0"
        );
        assert_eq!(v("true"), json!(true));
        assert_eq!(v(r#""1730000000000000000""#), json!("1730000000000000000"));
        assert_eq!(v("[1,2]"), json!([1, 2]));
        // Not JSON by the RFC 8259 grammar, and not a number to Rust either: a bare string.
        for text in [
            "[draft]",
            "hello",
            "DUPLET-07",
            "1e",
            "0x10",
            "1_000",
            "1.2.3",
            "",
            "-",
        ] {
            assert_eq!(v(text), json!(text), "{text:?}");
        }
    }

    /// A text Rust's `parse::<f64>()` reads as a number is a number the person meant: it is refused
    /// (naming the flag and key and why), never stored as a string; the JSON-quoted form stores the
    /// text (#886 round 2).
    #[test]
    fn a_number_spelled_loosely_is_refused_and_the_quoted_form_stores_the_text() {
        let msg = |text: &str| match super::parse_user_value("--set k", text) {
            Err(crate::Error::Invalid(m)) => m,
            other => panic!("{text}: expected a refusal, got {other:?}"),
        };
        assert_eq!(
            msg("+5"),
            "--set k = +5 is a number, but not in JSON's number spelling (write `5`, not `+5`; no \
             leading `+` or zero, and no `.5` or `5.`). Write it as a JSON number, or to store the \
             text as it is, quote it as a JSON string: --set k='\"+5\"'"
        );
        assert_eq!(
            msg("Infinity"),
            "--set k = Infinity is not a finite number: JSON has no spelling for NaN or infinity, \
             so a seal cannot hold it. Write it as a JSON number, or to store the text as it is, \
             quote it as a JSON string: --set k='\"Infinity\"'"
        );
        assert_eq!(
            msg("+1e400"),
            "--set k = +1e400 is past a double's range (it reads as infinity), which a seal cannot \
             hold. Write it as a JSON number, or to store the text as it is, quote it as a JSON \
             string: --set k='\"+1e400\"'"
        );
        for text in [
            "+9007199254740993",
            "-Infinity",
            "inf",
            "-inf",
            "NaN",
            "nan",
            "+inf",
            "01",
            "1.",
            ".5",
            "+1",
        ] {
            msg(text);
        }
        // The quoted form is JSON, so it stores the text as a string.
        let v = |t: &str| super::parse_user_value("--set k", t).unwrap();
        assert_eq!(v(r#""+5""#), json!("+5"));
        assert_eq!(v(r#""Infinity""#), json!("Infinity"));
        assert_eq!(v(r#""01""#), json!("01"));
        // Plain JSON numbers, 2^53−1 included, stay numbers.
        assert_eq!(v("5"), json!(5));
        assert_eq!(v("9007199254740991"), json!(9_007_199_254_740_991_u64));
    }

    /// The other I-JSON MUSTs (RFC 7493 §2.1): an escaped lone surrogate is refused in a value and
    /// in a key. (Duplicate names, §2.3: `tests/seal_cases.rs::duplicate_keys_are_refused_by_name`.)
    #[test]
    fn lone_surrogates_are_refused() {
        for text in [r#"{"a":"\ud800"}"#, r#"{"a":"\udc00x"}"#, r#"{"\ud83d":1}"#] {
            assert!(
                matches!(super::parse_strict(text), Err(crate::Error::Serde(_))),
                "{text}"
            );
        }
        assert!(super::parse_strict(r#"{"a":"😀"}"#).is_ok());
    }

    /// The one f64→JSON helper (#886): ±inf and NaN are refused (not written as `null`), the bound
    /// is accepted and the bound + 1 refused.
    #[test]
    fn json_number_refuses_non_finite_and_beyond_the_bound() {
        use crate::Error;
        for f in [f64::INFINITY, f64::NEG_INFINITY, f64::NAN] {
            match super::json_number("x", f) {
                Err(Error::NonFiniteNumber { what, value }) => {
                    assert_eq!(what, "x");
                    assert_eq!(value, f.to_string());
                }
                other => panic!("{f}: expected NonFiniteNumber, got {other:?}"),
            }
            assert_eq!(super::unsealable_f64(f), Some(super::Unsealable::NonFinite));
        }
        let bound = super::MAX_EXACT_INTEGER as f64;
        assert_eq!(super::json_number("x", bound).unwrap(), json!(bound));
        assert_eq!(super::json_number("x", -bound).unwrap(), json!(-bound));
        // 2^53 is the first double past the bound (the bound + 1).
        for f in [bound + 1.0, -(bound + 1.0), 1e300] {
            assert!(
                matches!(
                    super::json_number("x", f),
                    Err(Error::InexactInteger { .. })
                ),
                "{f}"
            );
            assert_eq!(super::unsealable_f64(f), Some(super::Unsealable::Beyond));
        }
        assert_eq!(super::json_number("x", -0.0).unwrap(), json!(-0.0));
        assert_eq!(super::json_number("x", 1.5).unwrap(), json!(1.5));
        let e = super::json_number("rescale_slope", f64::NAN).unwrap_err();
        assert_eq!(
            e.to_string(),
            "rescale_slope = NaN is not a finite number: JSON has no spelling for NaN or infinity \
             (RFC 8259 §6), so a seal cannot hold it"
        );
    }

    /// `to_value` writes a typed NaN as `null`; `to_json_value` refuses it, and still returns
    /// `to_value`'s result for a finite document (#886).
    #[test]
    fn to_json_value_refuses_a_non_finite_field_and_keeps_null() {
        #[derive(serde::Serialize)]
        struct Spec {
            slope: Option<f64>,
            affine: [f64; 2],
            absent: Option<f64>,
        }
        let ok = Spec {
            slope: Some(2.0),
            affine: [1.0, 0.5],
            absent: None,
        };
        assert_eq!(
            super::to_json_value(&ok).unwrap(),
            json!({"slope": 2.0, "affine": [1.0, 0.5], "absent": null})
        );
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            // The defect: plain `to_value` turns the field into `null`.
            let silent = serde_json::to_value(Spec {
                slope: Some(bad),
                affine: [1.0, 0.5],
                absent: None,
            });
            assert_eq!(silent.unwrap()["slope"], json!(null));
            for doc in [
                Spec {
                    slope: Some(bad),
                    affine: [1.0, 0.5],
                    absent: None,
                },
                Spec {
                    slope: None,
                    affine: [1.0, bad],
                    absent: None,
                },
            ] {
                assert!(
                    matches!(
                        super::to_json_value(&doc),
                        Err(crate::Error::NonFiniteNumber { .. })
                    ),
                    "{bad}"
                );
            }
        }
    }

    /// The probe names where the non-finite number is, in the document's own spelling, through
    /// `f32` and `f64`, sequences, maps, options and every enum shape (#886). Each case fails if
    /// `serialize_f32` (or the f64 twin, or a compound's element walk) were to accept the float.
    #[test]
    fn the_probe_names_the_path_of_a_non_finite_f32_f64_map_and_enum_field() {
        use std::collections::BTreeMap;

        #[derive(serde::Serialize)]
        enum Shape {
            Unit,
            Newtype(f32),
            Tuple(u8, f64),
            Struct { scale: f32 },
        }
        #[derive(serde::Serialize)]
        struct Doc {
            shapes: Vec<Shape>,
            by_name: BTreeMap<String, Option<f32>>,
            column_meta: BTreeMap<String, f64>,
        }
        let doc = |f: &dyn Fn(&mut Doc)| {
            let mut d = Doc {
                shapes: vec![
                    Shape::Unit,
                    Shape::Newtype(1.0),
                    Shape::Tuple(1, 2.0),
                    Shape::Struct { scale: 3.0 },
                ],
                by_name: [("a".to_owned(), Some(1.0))].into(),
                column_meta: [("energy".to_owned(), 0.5)].into(),
            };
            f(&mut d);
            d
        };
        let named = |d: &Doc| match super::to_json_value(d) {
            Err(crate::Error::NonFiniteNumber { what, value }) => (what, value),
            other => panic!("expected NonFiniteNumber, got {other:?}"),
        };
        assert!(super::to_json_value(&doc(&|_| {})).is_ok());
        for (bad, text) in [
            (f32::NAN, "NaN"),
            (f32::INFINITY, "inf"),
            (f32::NEG_INFINITY, "-inf"),
        ] {
            let b64 = f64::from(bad);
            assert_eq!(
                named(&doc(&|d| d.shapes[1] = Shape::Newtype(bad))),
                ("shapes[1].Newtype".into(), text.into())
            );
            assert_eq!(
                named(&doc(&|d| d.shapes[2] = Shape::Tuple(1, b64))),
                ("shapes[2].Tuple[1]".into(), text.into())
            );
            assert_eq!(
                named(&doc(&|d| d.shapes[3] = Shape::Struct { scale: bad })),
                ("shapes[3].Struct.scale".into(), text.into())
            );
            assert_eq!(
                named(&doc(&|d| {
                    d.by_name.insert("a".into(), Some(bad));
                })),
                ("by_name.a".into(), text.into())
            );
            assert_eq!(
                named(&doc(&|d| {
                    d.column_meta.insert("energy".into(), b64);
                })),
                ("column_meta.energy".into(), text.into())
            );
        }
        // A bare top-level float is named as the value itself.
        assert_eq!(
            super::to_json_value(&f32::NAN).unwrap_err().to_string(),
            "the top-level value = NaN is not a finite number: JSON has no spelling for NaN or \
             infinity (RFC 8259 §6), so a seal cannot hold it"
        );
    }

    /// A finite `f32` beyond the bound passes the probe (it is finite) and is refused where every
    /// typed number is, at the seal, with the neutral message: a typed field has no string form.
    #[test]
    fn a_finite_f32_beyond_the_bound_is_refused_at_the_seal_without_string_advice() {
        #[derive(serde::Serialize)]
        struct Spec {
            scale: f32,
        }
        let spec = Spec { scale: 1.0e16 };
        assert!(super::to_json_value(&spec).is_ok());
        let e = super::hash(&spec).unwrap_err();
        assert!(
            matches!(&e, crate::Error::InexactInteger { path, as_string: false, .. } if path == "scale"),
            "{e}"
        );
        assert!(!e.to_string().contains("as a string"), "{e}");
    }

    // ---------------------------------------------------------------- check_canonical (ADR-0066)
    //
    // The respelling cases are the spike's (docs/spikes/897-seal-encoding/src/attacks.rs): T4 key
    // order, T5 whitespace, T6 a `\u` escape, T7/S7 `64.0` for `64`, S8 a pretty layout; S5b an
    // integer a double holds exactly beyond 2^53. Each corpus product gets the same replay in
    // tests/stored_bytes.rs; here they are pinned on a small document with exact offsets.

    use super::check_canonical;
    use crate::Error;

    /// A small canonical document holding every JSON type, an array shape and a non-ASCII string.
    const DOC: &str = r#"{"a":[64,32],"b":{"c":null,"d":true},"e":"x\u00e9y","f":1.5,"g":-3}"#;

    fn non_canonical_at(bytes: &[u8]) -> usize {
        match check_canonical(bytes) {
            Err(Error::NonCanonical { offset, .. }) => offset,
            other => panic!(
                "{:?}: expected NonCanonical, got {other:?}",
                String::from_utf8_lossy(bytes)
            ),
        }
    }

    #[test]
    fn a_canonical_document_is_accepted_and_parsed() {
        // The constant spells `é` as an escape only in the Rust source: the bytes are UTF-8.
        let doc = DOC.replace(r"\u00e9", "é");
        let v = check_canonical(doc.as_bytes()).unwrap();
        assert_eq!(v["a"], json!([64, 32]));
        assert_eq!(v["e"], json!("xéy"));
        // Any JSON value, not only an object; and the empty object.
        assert_eq!(check_canonical(b"[]").unwrap(), json!([]));
        assert_eq!(check_canonical(b"{}").unwrap(), json!({}));
        assert_eq!(check_canonical(b"1e-7").unwrap(), json!(1e-7));
    }

    /// T4: the same members in another order.
    #[test]
    fn reordered_keys_are_refused_at_the_first_differing_byte() {
        assert_eq!(non_canonical_at(br#"{"b":1,"a":2}"#), 2);
        // Nested: the outer order is canonical, the inner one is not.
        assert_eq!(non_canonical_at(br#"{"a":{"y":1,"x":2}}"#), 7);
    }

    /// T5 and S8: whitespace anywhere, a trailing newline, a pretty layout.
    #[test]
    fn whitespace_and_a_pretty_layout_are_refused() {
        assert_eq!(non_canonical_at(br#"{ "a":1}"#), 1);
        assert_eq!(non_canonical_at(br#"{"a": 1}"#), 5);
        // A trailing newline: the JCS form is a strict prefix, so the offset is its length.
        assert_eq!(non_canonical_at(b"{\"a\":1}\n"), 7);
        let doc = DOC.replace(r"\u00e9", "é");
        let v: serde_json::Value = serde_json::from_str(&doc).unwrap();
        let pretty = serde_json::to_string_pretty(&v).unwrap();
        assert_eq!(non_canonical_at(pretty.as_bytes()), 1);
    }

    /// T6: a string escape JCS doesn't write, in a value or a key.
    #[test]
    fn escapes_jcs_does_not_write_are_refused() {
        assert_eq!(non_canonical_at(br#"{"a":"\u0078"}"#), 6);
        assert_eq!(non_canonical_at(br#"{"\u0061":1}"#), 2);
        assert_eq!(non_canonical_at(br#"{"a":"\/"}"#), 6);
        // A non-ASCII character as an escape: JCS writes the UTF-8 bytes.
        assert_eq!(non_canonical_at(br#"{"a":"\u00e9"}"#), 6);
        // The escapes JCS does write are canonical.
        check_canonical(br#"{"a":"\"\\\n\u001f"}"#).unwrap();
    }

    /// T7 and S7 (#901): an integral number spelled as a float, and the other number respellings.
    #[test]
    fn number_respellings_are_refused() {
        // `"a":[64.0`: the stored `.` is where JCS ends the number.
        assert_eq!(non_canonical_at(br#"{"a":[64.0,32]}"#), 8);
        assert_eq!(non_canonical_at(b"1E2"), 1);
        assert_eq!(non_canonical_at(b"[1e2]"), 2);
        assert_eq!(non_canonical_at(b"[-0]"), 1);
        assert_eq!(non_canonical_at(b"[0.0000001]"), 1);
        assert_eq!(non_canonical_at(b"[1.50]"), 4);
        assert_eq!(non_canonical_at(b"[1.5E-10]"), 4);
        // JCS's own forms of the same numbers are canonical.
        for ok in ["[100]", "[0]", "[1e-7]", "[1.5]", "[1.5e-10]"] {
            check_canonical(ok.as_bytes()).unwrap();
        }
    }

    /// The error names the offset and shows both byte runs from it, ASCII-escaped.
    #[test]
    fn the_non_canonical_error_names_the_offset_and_both_spellings() {
        let e = check_canonical(br#"{"a":[64.0]}"#).unwrap_err();
        let Error::NonCanonical {
            offset,
            found,
            canonical,
        } = &e
        else {
            panic!("{e:?}")
        };
        assert_eq!(
            (*offset, found.as_str(), canonical.as_str()),
            (8, ".0]}", "]}")
        );
        assert_eq!(
            e.to_string(),
            "non-canonical bytes: the stored document differs from its own RFC 8785 (JCS) form at \
             byte 8 (stored `.0]}`, JCS `]}`); a sealed JSON document must be stored as exactly \
             its JCS bytes (ADR-0066)"
        );
        // Past the end of the canonical bytes, its run is empty; non-ASCII is escaped.
        let Error::NonCanonical {
            found, canonical, ..
        } = check_canonical("\"é\" ".as_bytes()).unwrap_err()
        else {
            unreachable!()
        };
        assert_eq!((found.as_str(), canonical.as_str()), (" ", ""));
        let Error::NonCanonical { found, .. } = check_canonical(br#"["\u00e9"]"#).unwrap_err()
        else {
            unreachable!()
        };
        assert_eq!(found, r#"\\u00e9\"]"#);
    }

    /// S5b: an integer beyond 2^53 that a double holds exactly is its own JCS form, so the byte
    /// comparison alone would accept it; the strict parse's I-JSON rule refuses it first.
    #[test]
    fn an_exact_double_beyond_2_53_passes_the_byte_comparison_but_is_refused_by_i_json() {
        for text in [
            r#"{"daq_start_ns":1730000000123456800}"#,
            r#"{"seed":9007199254740992}"#,
            r#"{"n":-9007199254740992}"#,
        ] {
            // The byte comparison on its own: raw JCS (no I-JSON check) reproduces the bytes.
            let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
            assert_eq!(
                serde_jcs::to_vec(&parsed).unwrap(),
                text.as_bytes(),
                "{text}"
            );
            // check_canonical refuses it, by I-JSON, naming the path.
            let e = check_canonical(text.as_bytes()).unwrap_err();
            assert!(matches!(e, Error::InexactInteger { .. }), "{text}: {e:?}");
        }
        // The bound itself is exact and canonical.
        check_canonical(br#"{"n":9007199254740991}"#).unwrap();
    }

    /// The strict parse's other refusals come first, as `Serde` errors: a duplicate key, a lone
    /// surrogate, bytes that are not UTF-8, malformed JSON, trailing data, excess nesting.
    #[test]
    fn the_strict_parse_runs_before_the_byte_comparison() {
        let serde = |b: &[u8]| match check_canonical(b) {
            Err(Error::Serde(e)) => e.to_string(),
            other => panic!(
                "{:?}: expected Serde, got {other:?}",
                String::from_utf8_lossy(b)
            ),
        };
        assert!(serde(br#"{"a":1,"a":1}"#).contains("duplicate key"));
        // Compact and key-sorted, so only the parse can refuse it.
        assert!(serde(br#"{"a":"\ud800"}"#).contains("hex escape"));
        assert!(serde(br#"{"a":"\udc00"}"#).contains("surrogate"));
        assert!(serde(b"{\"a\":\"\xff\"}").contains("unicode"));
        assert!(serde(b"[\xc3]").contains("expected value"));
        serde(b"");
        serde(b"{} {}");
        serde(b"[1,]");
    }

    /// Nesting is bounded by `serde_json`'s recursion limit: 127 nested arrays or objects are
    /// accepted, 128 are refused (the `nesting past 127 levels` case of `corpus/seal-cases.json`).
    #[test]
    fn nesting_beyond_127_levels_is_refused() {
        let nest = |n: usize| format!("{}{}", "[".repeat(n), "]".repeat(n));
        check_canonical(nest(127).as_bytes()).unwrap();
        let e = check_canonical(nest(128).as_bytes()).unwrap_err();
        assert!(e.to_string().contains("recursion limit"), "{e}");
    }

    /// The shrunk case of `proptest-regressions/canonical.txt`: JCS has one number type, so the
    /// double `0.0` is written `0` and reads back as the integer 0. The two `Value`s differ; their
    /// bytes, which are what a seal covers, do not. This is why the property below compares bytes.
    #[test]
    fn an_integral_double_reads_back_as_the_integer_jcs_writes() {
        let v = json!([0.0]);
        let bytes = super::to_bytes(&v).unwrap();
        assert_eq!(bytes, b"[0]");
        let back = check_canonical(&bytes).unwrap();
        assert!(back[0].is_u64(), "{back:?}");
        assert_ne!(back, v);
        assert_eq!(super::to_bytes(&back).unwrap(), bytes);
    }

    proptest::proptest! {
        /// Whatever [`super::to_bytes`] writes, `check_canonical` accepts, and the value it reads
        /// back writes the same bytes: the writer's form and the reader's check are one definition.
        /// (The parsed value can differ from `v` as a `Value`, never as JCS: JCS has one number
        /// type, so the double `0.0` is written `0` and reads back as the integer 0.)
        #[test]
        fn to_bytes_output_is_always_canonical(v in json_value()) {
            let bytes = super::to_bytes(&v).unwrap();
            let back = check_canonical(&bytes).unwrap();
            proptest::prop_assert_eq!(super::to_bytes(&back).unwrap(), bytes);
        }
    }

    /// Arbitrary I-JSON values: finite doubles, exact integers, any strings (non-ASCII and control
    /// characters included), nested arrays and objects.
    fn json_value() -> impl proptest::strategy::Strategy<Value = serde_json::Value> {
        use proptest::prelude::*;
        let max = super::MAX_EXACT_INTEGER as i64;
        let leaf = prop_oneof![
            Just(serde_json::Value::Null),
            any::<bool>().prop_map(serde_json::Value::Bool),
            (-max..=max).prop_map(|n| json!(n)),
            any::<f64>()
                .prop_filter("finite, within the bound", |f| {
                    super::unsealable_f64(*f).is_none()
                })
                .prop_map(|f| json!(f)),
            any::<String>().prop_map(serde_json::Value::String),
        ];
        leaf.prop_recursive(4, 32, 6, |inner| {
            prop_oneof![
                proptest::collection::vec(inner.clone(), 0..6).prop_map(serde_json::Value::Array),
                proptest::collection::btree_map(any::<String>(), inner, 0..6)
                    .prop_map(|m| serde_json::Value::Object(m.into_iter().collect())),
            ]
        })
    }
}
