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
        n.as_f64()
            .is_some_and(|f| f.abs() > MAX_EXACT_INTEGER as f64)
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
/// - §2.1 UTF-8 only, no unpaired surrogate: the text is a `&str`, and `serde_json` refuses an
///   escaped lone surrogate (`"\ud800"`) in a string or key;
/// - §2.2 numbers: [`check_i_json`] refuses an integer beyond ±(2^53−1); `NaN`/`Infinity` aren't
///   JSON and a number beyond a double's range (`1e400`) is a parse error;
/// - §2.3 unique names: the duplicate-key refusal above.
///
/// # Errors
/// [`crate::Error::Serde`] for malformed JSON, a lone surrogate or a duplicate key (the message
/// names the key); [`crate::Error::InexactInteger`] for an integer beyond ±(2^53−1).
pub fn parse_strict(s: &str) -> crate::Result<Value> {
    let mut de = serde_json::Deserializer::from_str(s);
    let StrictValue(v) = StrictValue::deserialize(&mut de)?;
    de.end()?;
    check_i_json(&v)?;
    Ok(v)
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
            Err(crate::Error::InexactInteger { path, value }) => (path, value),
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
            "id_inputs.daq_start_ns = 1730000000000000000 exceeds ±(2^53−1) (I-JSON, RFC 7493 \
             §2.2): a sealed JSON number is an IEEE-754 double (RFC 8785 §3.2.2.3), which can't \
             hold this integer exactly; write it as a string"
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
}
