//! Option B prototype: a deterministic CBOR encoder and a strict decoder for the tessera profile.
//!
//! Profile (RFC 8949 §4.2.1 "core deterministic encoding" plus tessera restrictions):
//! - every argument (integer, length) in its shortest form; no indefinite-length items;
//! - map keys are text strings, sorted by the bytewise order of their encodings, no key twice;
//! - floats in preferred serialization (the shortest of f16/f32/f64 that holds the value exactly,
//!   RFC 8949 §4.2.2); NaN and ±infinity are refused, as I-JSON refuses them today;
//! - **numeric reduction** (profile `B`, as in dCBOR): a float with an integral value in
//!   -2^64..2^64 is written as that integer (`64.0` → `64`, `-0.0` → `0`) and the decoder refuses it
//!   as a float, so integer and float never spell one number twice. Profile `B0` omits this rule; the
//!   spike keeps it to show what the rule buys (see `attacks` S7 and the JS re-encode probe);
//! - no tags, no `undefined`, no simple values but `false`/`true`/`null`;
//! - text strings are valid UTF-8 (so a lone surrogate cannot be expressed at all);
//! - at most 127 nested arrays/maps (SPEC §4's JSON limit, kept for parity);
//! - exactly one item, no trailing bytes.
//!
//! The decoder refuses any other form at the byte that breaks the rule, so a sealed byte string has
//! exactly one decoded value and every value has exactly one byte string.

use std::fmt;

/// Which deterministic profile: `B` reduces integral floats to integers, `B0` does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    B,
    B0,
}

/// The integer an integral float stands for under numeric reduction, if it has one in -2^64..2^64.
pub fn reduce(f: f64) -> Option<Value> {
    const TWO64: f64 = 18_446_744_073_709_551_616.0;
    if f.fract() != 0.0 || !(-TWO64..TWO64).contains(&f) {
        return None;
    }
    Some(if f >= 0.0 { Value::Uint(f as u64) } else { Value::Nint(((-f) as u128 - 1) as u64) })
}

/// The CBOR data model as the profile admits it.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// Major type 0: 0..=2^64-1.
    Uint(u64),
    /// Major type 1: the value is `-1 - n`, so -1..=-2^64.
    Nint(u64),
    /// Major type 2.
    Bytes(Vec<u8>),
    /// Major type 3.
    Text(String),
    /// Major type 4.
    Array(Vec<Value>),
    /// Major type 5, keys text only, kept in canonical order after encode/decode.
    Map(Vec<(String, Value)>),
    /// Simple values 20/21.
    Bool(bool),
    /// Simple value 22.
    Null,
    /// Major type 7, additional info 25/26/27.
    Float(f64),
}

/// Why a byte string is not a profile encoding.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodeError {
    pub offset: usize,
    pub what: String,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "byte {}: {}", self.offset, self.what)
    }
}

pub const MAX_DEPTH: usize = 127;

// ---------------------------------------------------------------- encoder

fn head(out: &mut Vec<u8>, major: u8, arg: u64) {
    let m = major << 5;
    if arg < 24 {
        out.push(m | arg as u8);
    } else if arg <= u8::MAX as u64 {
        out.extend_from_slice(&[m | 24, arg as u8]);
    } else if arg <= u16::MAX as u64 {
        out.push(m | 25);
        out.extend_from_slice(&(arg as u16).to_be_bytes());
    } else if arg <= u32::MAX as u64 {
        out.push(m | 26);
        out.extend_from_slice(&(arg as u32).to_be_bytes());
    } else {
        out.push(m | 27);
        out.extend_from_slice(&arg.to_be_bytes());
    }
}

/// f32 → f16 bits when the value is exactly representable as a binary16, else `None`.
fn f32_to_f16_exact(f: f32) -> Option<u16> {
    let bits = f.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xff) as i32;
    let mant = bits & 0x7f_ffff;
    if exp == 0 && mant == 0 {
        return Some(sign); // ±0
    }
    if exp == 0 || exp == 0xff {
        return None; // f32 subnormals are far below f16's range; inf/NaN never reach here
    }
    let e = exp - 127;
    if (-14..=15).contains(&e) {
        // binary16 normal: needs the low 13 mantissa bits to be zero
        if mant & 0x1fff != 0 {
            return None;
        }
        return Some(sign | (((e + 15) as u16) << 10) | (mant >> 13) as u16);
    }
    if (-24..=-15).contains(&e) {
        // binary16 subnormal: k * 2^-24 with k < 1024
        let full = 0x80_0000 | mant; // 1.m as a 24-bit integer
        let shift = (-(e + 1)) as u32; // value = full * 2^(e-23) = (full >> shift) * 2^-24
        if full & ((1 << shift) - 1) != 0 {
            return None;
        }
        return Some(sign | (full >> shift) as u16);
    }
    None
}

fn f16_to_f64(h: u16) -> f64 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = ((h >> 10) & 0x1f) as i32;
    let mant = (h & 0x3ff) as f64;
    match exp {
        0 => sign * mant * 2f64.powi(-24),
        31 => {
            if mant == 0.0 {
                sign * f64::INFINITY
            } else {
                f64::NAN
            }
        }
        _ => sign * (1.0 + mant / 1024.0) * 2f64.powi(exp - 15),
    }
}

fn float(out: &mut Vec<u8>, f: f64) -> Result<(), String> {
    if !f.is_finite() {
        return Err(format!("{f} is not a profile float (NaN/±inf are refused)"));
    }
    let f32v = f as f32;
    if f32v as f64 == f {
        if let Some(h) = f32_to_f16_exact(f32v) {
            out.push(0xf9);
            out.extend_from_slice(&h.to_be_bytes());
        } else {
            out.push(0xfa);
            out.extend_from_slice(&f32v.to_bits().to_be_bytes());
        }
    } else {
        out.push(0xfb);
        out.extend_from_slice(&f.to_bits().to_be_bytes());
    }
    Ok(())
}

/// Encode `v` in profile `B`'s one deterministic form. Sorts map keys; refuses a key twice and
/// non-finite floats; writes an integral float as its integer.
pub fn encode(v: &Value) -> Result<Vec<u8>, String> {
    encode_as(v, Profile::B)
}

/// Encode `v` in the given profile.
pub fn encode_as(v: &Value, p: Profile) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    enc(&mut out, v, p)?;
    Ok(out)
}

fn enc(out: &mut Vec<u8>, v: &Value, p: Profile) -> Result<(), String> {
    match v {
        Value::Uint(n) => head(out, 0, *n),
        Value::Nint(n) => head(out, 1, *n),
        Value::Bytes(b) => {
            head(out, 2, b.len() as u64);
            out.extend_from_slice(b);
        }
        Value::Text(s) => {
            head(out, 3, s.len() as u64);
            out.extend_from_slice(s.as_bytes());
        }
        Value::Array(items) => {
            head(out, 4, items.len() as u64);
            for i in items {
                enc(out, i, p)?;
            }
        }
        Value::Map(entries) => {
            let mut encoded: Vec<(Vec<u8>, Vec<u8>)> = Vec::with_capacity(entries.len());
            for (k, val) in entries {
                let mut kb = Vec::new();
                enc(&mut kb, &Value::Text(k.clone()), p)?;
                let mut vb = Vec::new();
                enc(&mut vb, val, p)?;
                encoded.push((kb, vb));
            }
            encoded.sort_by(|a, b| a.0.cmp(&b.0));
            for w in encoded.windows(2) {
                if w[0].0 == w[1].0 {
                    return Err("a map key appears twice".into());
                }
            }
            head(out, 5, encoded.len() as u64);
            for (k, val) in encoded {
                out.extend_from_slice(&k);
                out.extend_from_slice(&val);
            }
        }
        Value::Bool(false) => out.push(0xf4),
        Value::Bool(true) => out.push(0xf5),
        Value::Null => out.push(0xf6),
        Value::Float(f) => match (p, reduce(*f)) {
            (Profile::B, Some(i)) => enc(out, &i, p)?,
            _ => float(out, *f)?,
        },
    }
    Ok(())
}

/// A non-deterministic encoder, the way a generic CBOR library (or a careless writer) may encode the
/// same value: map keys in canonical order (reversed when `reverse_keys`), duplicates kept, integers
/// with an 8-byte argument when `wide_ints`, every float as f64 when `f64_floats`. Used to build the
/// alternative encodings the strict decoder must refuse.
#[derive(Clone, Copy, Default)]
pub struct Loose {
    pub reverse_keys: bool,
    pub wide_ints: bool,
    pub f64_floats: bool,
}

pub fn encode_loose(v: &Value, o: Loose) -> Vec<u8> {
    let mut out = Vec::new();
    enc_loose(&mut out, v, o);
    out
}

fn head_loose(out: &mut Vec<u8>, major: u8, arg: u64, o: Loose) {
    if o.wide_ints && (major == 0 || major == 1) && arg >= 24 {
        out.push(major << 5 | 27);
        out.extend_from_slice(&arg.to_be_bytes());
    } else {
        head(out, major, arg);
    }
}

fn enc_loose(out: &mut Vec<u8>, v: &Value, o: Loose) {
    match v {
        Value::Uint(n) => head_loose(out, 0, *n, o),
        Value::Nint(n) => head_loose(out, 1, *n, o),
        Value::Array(items) => {
            head(out, 4, items.len() as u64);
            for i in items {
                enc_loose(out, i, o);
            }
        }
        Value::Map(entries) => {
            head(out, 5, entries.len() as u64);
            let mut es: Vec<&(String, Value)> = entries.iter().collect();
            // canonical order of text keys = by encoded bytes = length first, then bytes
            es.sort_by(|a, b| (a.0.len(), a.0.as_bytes()).cmp(&(b.0.len(), b.0.as_bytes())));
            if o.reverse_keys {
                es.reverse();
            }
            for (k, x) in es {
                head(out, 3, k.len() as u64);
                out.extend_from_slice(k.as_bytes());
                enc_loose(out, x, o);
            }
        }
        Value::Float(f) if o.f64_floats => {
            out.push(0xfb);
            out.extend_from_slice(&f.to_bits().to_be_bytes());
        }
        other => enc(out, other, Profile::B0).expect("scalar"),
    }
}

// ---------------------------------------------------------------- strict decoder

struct Dec<'a> {
    b: &'a [u8],
    pos: usize,
    profile: Profile,
}

impl Dec<'_> {
    fn err<T>(&self, at: usize, what: impl Into<String>) -> Result<T, DecodeError> {
        Err(DecodeError { offset: at, what: what.into() })
    }

    fn take(&mut self, n: usize) -> Result<&[u8], DecodeError> {
        if self.b.len() - self.pos < n {
            return self.err(self.pos, format!("truncated: {n} more bytes expected"));
        }
        let s = &self.b[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    /// Read a head's argument, refusing every non-shortest form and indefinite length.
    fn arg(&mut self, at: usize, ai: u8) -> Result<u64, DecodeError> {
        let (n, min) = match ai {
            0..=23 => return Ok(ai as u64),
            24 => (u64::from(self.take(1)?[0]), 24u64),
            25 => (u64::from(u16::from_be_bytes(self.take(2)?.try_into().unwrap())), 0x100),
            26 => (u64::from(u32::from_be_bytes(self.take(4)?.try_into().unwrap())), 0x1_0000),
            27 => (u64::from_be_bytes(self.take(8)?.try_into().unwrap()), 0x1_0000_0000),
            31 => return self.err(at, "indefinite-length item (RFC 8949 §4.2.1 forbids it)"),
            _ => return self.err(at, format!("reserved additional information {ai}")),
        };
        if n < min {
            return self.err(at, format!("argument {n} not in its shortest form (additional information {ai})"));
        }
        Ok(n)
    }

    fn item(&mut self, depth: usize) -> Result<Value, DecodeError> {
        let at = self.pos;
        let ib = self.take(1)?[0];
        let (major, ai) = (ib >> 5, ib & 0x1f);
        if major == 7 {
            return match ai {
                20 => Ok(Value::Bool(false)),
                21 => Ok(Value::Bool(true)),
                22 => Ok(Value::Null),
                25 => {
                    let h = u16::from_be_bytes(self.take(2)?.try_into().unwrap());
                    self.float_ok(at, f16_to_f64(h), 25)
                }
                26 => {
                    let f = f32::from_bits(u32::from_be_bytes(self.take(4)?.try_into().unwrap())) as f64;
                    self.float_ok(at, f, 26)
                }
                27 => {
                    let f = f64::from_bits(u64::from_be_bytes(self.take(8)?.try_into().unwrap()));
                    self.float_ok(at, f, 27)
                }
                23 => self.err(at, "`undefined` is not in the profile"),
                31 => self.err(at, "a break outside an indefinite-length item"),
                _ => self.err(at, format!("simple value {ai} is not in the profile")),
            };
        }
        let n = self.arg(at, ai)?;
        match major {
            0 => Ok(Value::Uint(n)),
            1 => Ok(Value::Nint(n)),
            2 => Ok(Value::Bytes(self.take(len(n))?.to_vec())),
            3 => {
                let raw = self.take(len(n))?;
                match std::str::from_utf8(raw) {
                    Ok(s) => Ok(Value::Text(s.to_owned())),
                    Err(e) => self.err(at, format!("text string is not valid UTF-8 ({e})")),
                }
            }
            4 | 5 if depth >= MAX_DEPTH => self.err(at, format!("nested deeper than {MAX_DEPTH}")),
            4 => {
                let mut items = Vec::new();
                for _ in 0..n {
                    items.push(self.item(depth + 1)?);
                }
                Ok(Value::Array(items))
            }
            5 => {
                let mut entries: Vec<(String, Value)> = Vec::new();
                let mut prev: Option<&[u8]> = None;
                for _ in 0..n {
                    let kstart = self.pos;
                    let key = match self.item(depth + 1)? {
                        Value::Text(s) => s,
                        _ => return self.err(kstart, "a map key that is not a text string"),
                    };
                    let kbytes = &self.b[kstart..self.pos];
                    if let Some(p) = prev {
                        match p.cmp(kbytes) {
                            std::cmp::Ordering::Equal => {
                                return self.err(kstart, format!("map key {key:?} appears twice"))
                            }
                            std::cmp::Ordering::Greater => {
                                return self.err(kstart, format!("map key {key:?} out of canonical order"))
                            }
                            std::cmp::Ordering::Less => {}
                        }
                    }
                    prev = Some(kbytes);
                    let val = self.item(depth + 1)?;
                    entries.push((key, val));
                }
                Ok(Value::Map(entries))
            }
            6 => self.err(at, "tags are not in the profile"),
            _ => unreachable!("major type is three bits"),
        }
    }

    fn float_ok(&self, at: usize, f: f64, ai: u8) -> Result<Value, DecodeError> {
        if !f.is_finite() {
            return self.err(at, "NaN/±infinity is not in the profile");
        }
        let mut pref = Vec::new();
        float(&mut pref, f).expect("finite");
        let width = match pref[0] {
            0xf9 => 25,
            0xfa => 26,
            _ => 27,
        };
        if width != ai {
            return self.err(at, format!("float {f} not in preferred serialization (ai {ai}, shortest is {width})"));
        }
        if self.profile == Profile::B && reduce(f).is_some() {
            return self.err(at, format!("float {f} is integral: numeric reduction writes it as an integer"));
        }
        Ok(Value::Float(f))
    }
}

fn len(n: u64) -> usize {
    usize::try_from(n).unwrap_or(usize::MAX)
}

/// Decode exactly one profile-`B` item, refusing every non-deterministic or out-of-profile form.
pub fn decode_strict(b: &[u8]) -> Result<Value, DecodeError> {
    decode_strict_as(b, Profile::B)
}

/// Decode exactly one item of the given profile.
pub fn decode_strict_as(b: &[u8], profile: Profile) -> Result<Value, DecodeError> {
    let mut d = Dec { b, pos: 0, profile };
    let v = d.item(0)?;
    if d.pos != b.len() {
        return Err(DecodeError { offset: d.pos, what: format!("{} trailing bytes", b.len() - d.pos) });
    }
    Ok(v)
}

// ---------------------------------------------------------------- JSON bridge

/// How a JSON manifest maps onto CBOR. `digests_as_bytes` stores a `blake3:<64 hex>` string as a
/// 32-byte byte string (a size measurement for a typed digest field; a real profile would do this per
/// schema field, not by pattern).
pub fn from_json(v: &serde_json::Value, digests_as_bytes: bool) -> Value {
    use serde_json::Value as J;
    match v {
        J::Null => Value::Null,
        J::Bool(b) => Value::Bool(*b),
        J::Number(n) => {
            if let Some(u) = n.as_u64() {
                Value::Uint(u)
            } else if let Some(i) = n.as_i64() {
                Value::Nint((-1 - i) as u64)
            } else {
                Value::Float(n.as_f64().expect("serde_json numbers are u64, i64 or f64"))
            }
        }
        J::String(s) => match s.strip_prefix("blake3:") {
            Some(hex) if digests_as_bytes && hex.len() == 64 && hex.bytes().all(|c| c.is_ascii_hexdigit()) => {
                Value::Bytes((0..32).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap()).collect())
            }
            _ => Value::Text(s.clone()),
        },
        J::Array(a) => Value::Array(a.iter().map(|x| from_json(x, digests_as_bytes)).collect()),
        J::Object(o) => Value::Map(o.iter().map(|(k, x)| (k.clone(), from_json(x, digests_as_bytes))).collect()),
    }
}

/// The derived JSON view of a decoded value (`tessera inspect --json` would print this). Integers are
/// written as exact decimal text, so the view is exact for any reader that keeps big integers
/// (Python, jq ≥ 1.7) and rounds in one that does not (JavaScript `JSON.parse`).
pub fn to_json_text(v: &Value) -> String {
    let mut s = String::new();
    view(&mut s, v);
    s
}

fn view(s: &mut String, v: &Value) {
    match v {
        Value::Uint(n) => s.push_str(&n.to_string()),
        Value::Nint(n) => s.push_str(&(-1i128 - *n as i128).to_string()),
        Value::Bytes(b) => {
            s.push_str("\"h'");
            for x in b {
                s.push_str(&format!("{x:02x}"));
            }
            s.push_str("'\"");
        }
        Value::Text(t) => s.push_str(&serde_json::to_string(t).unwrap()),
        Value::Array(a) => {
            s.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                view(s, x);
            }
            s.push(']');
        }
        Value::Map(m) => {
            s.push('{');
            for (i, (k, x)) in m.iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                s.push_str(&serde_json::to_string(k).unwrap());
                s.push(':');
                view(s, x);
            }
            s.push('}');
        }
        Value::Bool(b) => s.push_str(if *b { "true" } else { "false" }),
        Value::Null => s.push_str("null"),
        Value::Float(f) => s.push_str(&serde_json::to_string(f).unwrap()),
    }
}

/// Bridge a decoded value to `serde_json` so tessera-core's typed layer (declared-form check, unique
/// block names, id/content_hash recomputation) runs unchanged. Integers outside u64/i64 have no
/// `serde_json` number and come back as `Err`.
pub fn to_serde_json(v: &Value) -> Result<serde_json::Value, String> {
    use serde_json::Value as J;
    Ok(match v {
        Value::Uint(n) => J::from(*n),
        Value::Nint(n) => {
            let i = -1i128 - *n as i128;
            J::from(i64::try_from(i).map_err(|_| format!("{i} is below i64::MIN"))?)
        }
        Value::Bytes(_) => return Err("a byte string has no JSON-typed-model counterpart here".into()),
        Value::Text(t) => J::String(t.clone()),
        Value::Array(a) => J::Array(a.iter().map(to_serde_json).collect::<Result<_, _>>()?),
        Value::Map(m) => {
            let mut o = serde_json::Map::new();
            for (k, x) in m {
                o.insert(k.clone(), to_serde_json(x)?);
            }
            J::Object(o)
        }
        Value::Bool(b) => J::Bool(*b),
        Value::Null => J::Null,
        Value::Float(f) => J::from(*f),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rt(v: Value) {
        let b = encode_as(&v, Profile::B0).unwrap();
        assert_eq!(decode_strict_as(&b, Profile::B0).unwrap(), v, "{b:02x?}");
        if !matches!(v, Value::Float(_)) {
            assert_eq!(decode_strict(&encode(&v).unwrap()).unwrap(), v);
        }
    }

    #[test]
    fn integers_round_trip_exactly_at_the_64_bit_edges() {
        for v in [0, 23, 24, 255, 256, 65535, 65536, u32::MAX as u64, u32::MAX as u64 + 1, u64::MAX] {
            rt(Value::Uint(v));
            rt(Value::Nint(v));
        }
    }

    #[test]
    fn floats_take_the_shortest_exact_width() {
        let b0 = |f: f64| encode_as(&Value::Float(f), Profile::B0).unwrap();
        assert_eq!(encode(&Value::Float(1.5)).unwrap(), [0xf9, 0x3e, 0x00]);
        assert_eq!(b0(-0.0), [0xf9, 0x80, 0x00]);
        assert_eq!(b0(65504.0), [0xf9, 0x7b, 0xff]);
        assert_eq!(encode(&Value::Float(5.960464477539063e-8)).unwrap(), [0xf9, 0x00, 0x01]); // f16 min subnormal
        assert_eq!(encode(&Value::Float(3.0517578125e-5)).unwrap(), [0xf9, 0x02, 0x00]); // f16 subnormal 2^-15
        assert_eq!(b0(100000.0), [0xfa, 0x47, 0xc3, 0x50, 0x00]);
        assert_eq!(encode(&Value::Float(100000.5)).unwrap()[0], 0xfa);
        assert_eq!(encode(&Value::Float(0.1)).unwrap()[0], 0xfb);
        for f in [0.1, 1e300, 5e-324, 3.4028234663852886e38, 6.103515625e-5, 0.333251953125] {
            rt(Value::Float(f));
        }
    }

    #[test]
    fn numeric_reduction_writes_integral_floats_as_integers_and_refuses_them_as_floats() {
        assert_eq!(encode(&Value::Float(64.0)).unwrap(), [0x18, 0x40]);
        assert_eq!(encode(&Value::Float(-0.0)).unwrap(), [0x00]);
        assert_eq!(encode(&Value::Float(-1024.0)).unwrap(), [0x39, 0x03, 0xff]);
        assert_eq!(encode(&Value::Float(-18446744073709551616.0)).unwrap(), encode(&Value::Nint(u64::MAX)).unwrap());
        assert_eq!(encode(&Value::Float(1e300)).unwrap()[0], 0xfb); // integral but beyond 2^64: stays a float
        let e = decode_strict(&[0xf9, 0x54, 0x00]).unwrap_err(); // 64.0 as f16
        assert!(e.what.contains("integral"), "{e}");
        assert_eq!(decode_strict_as(&[0xf9, 0x54, 0x00], Profile::B0).unwrap(), Value::Float(64.0));
    }

    #[test]
    fn the_strict_decoder_refuses_every_alternative_form() {
        let cases: &[(&[u8], &str)] = &[
            (&[0x18, 0x05], "shortest"),             // 5 with a 1-byte argument
            (&[0x19, 0x00, 0x40], "shortest"),       // 64 with a 2-byte argument
            (&[0x7f, 0x61, 0x61, 0xff], "indefinite"), // indefinite text
            (&[0xa2, 0x61, 0x62, 0x01, 0x61, 0x61, 0x02], "order"),
            (&[0xa2, 0x61, 0x61, 0x01, 0x61, 0x61, 0x02], "twice"),
            (&[0xfa, 0x3f, 0xc0, 0x00, 0x00], "preferred"), // 1.5 as f32
            (&[0xf9, 0x7e, 0x00], "NaN"),
            (&[0xc1, 0x00], "tags"),
            (&[0xc2, 0x41, 0x01], "tags"), // bignum
            (&[0xf7], "undefined"),
            (&[0x63, 0xed, 0xa0, 0x80], "UTF-8"), // CESU-8 lone surrogate U+D800
            (&[0xa1, 0x01, 0x01], "not a text"),
            (&[0x01, 0x01], "trailing"),
        ];
        for (b, want) in cases {
            let e = decode_strict(b).unwrap_err();
            assert!(e.what.contains(want), "{b:02x?}: {e}");
        }
    }
}
