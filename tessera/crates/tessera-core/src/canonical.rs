//! Canonical JSON encoding (RFC 8785 JCS) — the one serialization Tessera hashes over.
//!
//! Any byte difference changes a content hash, so hashing must be over a *canonical* form:
//! lexicographically sorted object keys, no insignificant whitespace, ECMAScript-canonical
//! numbers. We use [`serde_jcs`] (RFC 8785). Reproducible by any language's JCS implementation
//! — the basis for cross-implementation + cross-version hash agreement (see ADR-0020).

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::Serialize;
use serde_json::{Map, Value};

/// Serialize `value` to canonical RFC 8785 JCS bytes.
pub fn to_bytes<T: Serialize>(value: &T) -> crate::Result<Vec<u8>> {
    serde_jcs::to_vec(value).map_err(|e| crate::Error::Canonicalization(e.to_string()))
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
/// # Errors
/// [`crate::Error::Serde`] for malformed JSON or a duplicate key (the message names the key).
pub fn parse_strict(s: &str) -> crate::Result<Value> {
    let mut de = serde_json::Deserializer::from_str(s);
    let StrictValue(v) = StrictValue::deserialize(&mut de)?;
    de.end()?;
    Ok(v)
}

/// Deserialize a parsed value into the typed view `T`, **refusing any spelling but the declared
/// one**: a struct only as a JSON object, a unit-variant name only as a string (SPEC §4 "objects
/// only").
///
/// serde accepts two spellings the sealed formats don't define. It reads a struct from a JSON array
/// positionally (`["t","u8"]` for `{"name":"t","dtype":"u8"}`), and a unit enum variant from a
/// one-key object (`{"table": null}` for `"table"`). Either is a second spelling of one sealed
/// value, and an array's "keys" are invisible to the declared-field walk that names a newer
/// writer's fields (ADR-0060 §2.2), so an unknown rule inside an array-form parent would be dropped
/// instead of refused. So after `T` is built, its own serialization is walked alongside `v`:
/// wherever either side holds an object or an array, the other must hold the same kind. That covers
/// every type `T` is made of, internally tagged and untagged enums included, without a per-type
/// list. Opaque values (`serde_json::Value` fields) serialize as received, so they never differ.
///
/// The walk relies on `T` emitting every value the input held. A struct-typed field that is
/// skipped on serialization (a `skip_serializing_if` on a struct, not on an `Option`, `Vec`, map,
/// `bool` or unit enum) would not be checked; no type in the manifest, collection or product
/// schema has one.
///
/// Manifest, collection and embedded product schema all deserialize through this.
///
/// # Errors
/// A [`serde_json::Error`]: `T`'s own deserialization error (a known field of the wrong type), or
/// one naming the first value in an undeclared spelling by its dotted path (`blocks[0]`,
/// `tables.events`, `members[0].role`).
pub(crate) fn from_declared_form<T>(v: &Value) -> Result<T, serde_json::Error>
where
    T: serde::de::DeserializeOwned + Serialize,
{
    let typed = T::deserialize(v)?;
    if let Some((path, got, declared)) =
        form_mismatch(v, &serde_json::to_value(&typed)?, String::new())
    {
        let at = if path.is_empty() {
            "the top-level value".to_owned()
        } else {
            format!("`{path}`")
        };
        return Err(de::Error::custom(format!(
            "{at} is {} where the format defines {} (one spelling per value: a record is an \
             object, never an array; a name is a string, never an object)",
            json_type(got),
            json_type(declared)
        )));
    }
    Ok(typed)
}

/// The first place, by dotted path, where `received` and `typed` (the serialization of the view
/// built from `received`) disagree on being an object or an array, with both values. Keys `typed`
/// omits (defaults) are not compared.
fn form_mismatch<'a>(
    received: &'a Value,
    typed: &'a Value,
    path: String,
) -> Option<(String, &'a Value, &'a Value)> {
    match (received, typed) {
        (Value::Object(r), Value::Object(t)) => t.iter().find_map(|(k, tv)| {
            let at = if path.is_empty() {
                k.clone()
            } else {
                format!("{path}.{k}")
            };
            r.get(k).and_then(|rv| form_mismatch(rv, tv, at))
        }),
        (Value::Array(r), Value::Array(t)) => r
            .iter()
            .zip(t)
            .enumerate()
            .find_map(|(i, (rv, tv))| form_mismatch(rv, tv, format!("{path}[{i}]"))),
        (Value::Object(_) | Value::Array(_), _) | (_, Value::Object(_) | Value::Array(_)) => {
            Some((path, received, typed))
        }
        _ => None,
    }
}

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
}
