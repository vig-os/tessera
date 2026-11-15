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
