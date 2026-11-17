//! Attack replay: every bypass class the format has met, built against each option.
//!
//! Two modes, because they fail differently:
//! - **T (tamper after seal):** the attacker edits a sealed product and keeps the old seal (and so
//!   any signature over it). A safe option catches every edit.
//! - **S (attacker-sealed):** the attacker writes and seals the product so that readers disagree on
//!   what it says. A safe option refuses the ambiguous form, so one seal has one meaning.

use serde_json::{json, Value};
use tessera_core::block::array::ArraySpec;
use tessera_core::canonical;
use tessera_core::hash::digest;
use tessera_core::Manifest;

use crate::cbor::{self, Loose};
use crate::options::{self, Verdict};

/// One attack instance per option. `None` = the attack has no counterpart under that option.
pub struct Case {
    pub a: Option<String>,
    pub ap: Option<(Vec<u8>, Option<String>)>,
    pub b: Option<(Vec<u8>, Option<String>)>,
}

pub struct Attack {
    pub id: &'static str,
    pub what: &'static str,
    pub build: fn(&Base) -> Option<Case>,
}

/// One corpus product in every option's stored form.
pub struct Base {
    pub text: String,       // A: the stored manifest.json
    pub obj: Value,         // the parsed object, manifest_hash included
    pub bare: Value,        // the object without manifest_hash
    pub seal: String,       // the seal, identical in A and A′
    pub ap: Vec<u8>,        // A′ stored bytes
    pub b: Vec<u8>,         // B stored bytes
    pub b_seal: String,     // B seal
}

impl Base {
    pub fn new(text: String) -> Self {
        let obj = canonical::parse_strict(&text).expect("corpus manifest parses");
        let mut bare = obj.clone();
        bare.as_object_mut().unwrap().remove("manifest_hash");
        let seal = obj["manifest_hash"].as_str().unwrap().to_owned();
        let ap = options::aprime_bytes(&obj);
        let b = options::b_bytes(&obj, false);
        let b_seal = digest(&b);
        Base { text, obj, bare, seal, ap, b, b_seal }
    }
    fn has_block(&self) -> bool {
        self.bare["blocks"].as_array().is_some_and(|b| !b.is_empty())
    }
    fn has_array_block(&self) -> bool {
        self.bare["blocks"].as_array().is_some_and(|bs| bs.iter().any(|b| b["kind"] == "array"))
    }
}

// --------------------------------------------------------------- helpers

/// Raw JCS (serde_jcs, no I-JSON check) — what an attacker's own tool would hash.
fn raw_jcs(v: &Value) -> Vec<u8> {
    serde_jcs::to_vec(v).unwrap()
}

/// An A text an attacker seals honestly over `bare` (seal = digest(JCS(bare))).
fn a_sealed(bare: &Value) -> String {
    let mut o = bare.clone();
    o["manifest_hash"] = Value::String(digest(&raw_jcs(bare)));
    serde_json::to_string_pretty(&o).unwrap()
}

fn a_with_seal(bare: &Value, seal: &str) -> String {
    let mut o = bare.clone();
    o["manifest_hash"] = Value::String(seal.into());
    serde_json::to_string_pretty(&o).unwrap()
}

fn sealed(bytes: Vec<u8>) -> (Vec<u8>, Option<String>) {
    let s = digest(&bytes);
    (bytes, Some(s))
}

fn b_of(v: &Value) -> Vec<u8> {
    cbor::encode(&cbor::from_json(v, false)).unwrap()
}

/// Insert `ins` right after the first `{` of `text` (a duplicate-key splice).
fn after_brace(text: &str, ins: &str) -> String {
    let i = text.find('{').unwrap() + 1;
    format!("{}{}{}", &text[..i], ins, &text[i..])
}

/// Respell the first number after `"shape":` as a float (`64` → `64.0`), whatever the whitespace.
fn respell_shape(text: &str) -> Option<String> {
    let k = text.find("\"shape\"")?;
    let start = k + text[k..].find(|c: char| c.is_ascii_digit())?;
    let end = start + text[start..].find(|c: char| !c.is_ascii_digit())?;
    Some(format!("{}.0{}", &text[..end], &text[end..]))
}

/// Respell the first character of the `tessera_version` value as a `\u` escape.
fn respell_escape(text: &str) -> String {
    let k = text.find("\"tessera_version\"").unwrap();
    let q = k + "\"tessera_version\"".len() + text[k + 17..].find('"').unwrap() + 1;
    let c = text[q..].chars().next().unwrap();
    format!("{}\\u{:04x}{}", &text[..q], c as u32, &text[q + c.len_utf8()..])
}

fn with_metadata(bare: &Value, key: &str, val: Value) -> Value {
    let mut o = bare.clone();
    let md = o.as_object_mut().unwrap().entry("metadata").or_insert_with(|| json!({}));
    md.as_object_mut().unwrap().insert(key.into(), val);
    o
}

/// Rebuild `bare` after a typed edit: keep `id`, recompute `content_hash` honestly.
fn reseal_blocks(bare: &Value, edit: impl FnOnce(&mut Vec<Value>)) -> Value {
    let mut o = bare.clone();
    edit(o["blocks"].as_array_mut().unwrap());
    let digests: Vec<String> =
        o["blocks"].as_array().unwrap().iter().map(|b| b["digest"].as_str().unwrap_or_default().to_owned()).collect();
    o["content_hash"] = Value::String(tessera_core::hash::merkle_root(&digests));
    o
}

// --------------------------------------------------------------- the attacks

pub fn all() -> Vec<Attack> {
    vec![
        Attack { id: "T1", what: "#573 splice a newer-format top-level key (`actors`), old seal kept", build: t1 },
        Attack { id: "T2", what: "#573 splice a newer-format key into blocks[0], old seal kept", build: t2 },
        Attack { id: "T3", what: "edit a value (name), old seal kept", build: t3 },
        Attack { id: "T4", what: "reorder keys, old seal kept", build: t4 },
        Attack { id: "T5", what: "change whitespace (A: compact + key-sorted rewrite), old seal kept", build: t5 },
        Attack { id: "T6", what: "respell a string with a \\u escape, old seal kept", build: t6 },
        Attack { id: "T7", what: "respell a number (64 → 64.0 / non-shortest CBOR int), old seal kept", build: t7 },
        Attack { id: "T8", what: "duplicate key prepended, old seal kept", build: t8 },
        Attack { id: "T9", what: "seal stripped", build: t9 },
        Attack { id: "T10", what: "#886 bytes with 2^53+1 sealed, served with 2^53", build: t10 },
        Attack { id: "S1", what: "duplicate key, sealed by the attacker (last-wins)", build: s1 },
        Attack { id: "S2", what: "#734 duplicate block name, sealed", build: s2 },
        Attack { id: "S3", what: "#873 BlockRef in array form, sealed", build: s3 },
        Attack { id: "S4", what: "#873 kind spelled as an object, sealed", build: s4 },
        Attack { id: "S5", what: "#886 epoch-ns 1730000000123456789 as a number, sealed", build: s5 },
        Attack { id: "S5b", what: "integer beyond 2^53 that a double holds exactly (1730000000123456800)", build: s5b },
        Attack { id: "S6", what: "lone surrogate U+D800 in description (A: original seal kept)", build: s6 },
        Attack { id: "S7", what: "array shape 64 written as the float 64.0, sealed", build: s7 },
        Attack { id: "S8", what: "non-canonical layout (A: the stored pretty file / pretty JSON / unsorted CBOR map)", build: s8 },
    ]
}

fn splice(base: &Base, f: impl Fn(&mut Value)) -> Case {
    let mut o = base.obj.clone();
    f(&mut o);
    let mut bare = base.bare.clone();
    f(&mut bare);
    Case {
        a: Some(serde_json::to_string_pretty(&o).unwrap()),
        ap: Some((raw_jcs(&bare), Some(base.seal.clone()))),
        b: Some((b_of(&bare), Some(base.b_seal.clone()))),
    }
}

fn t1(base: &Base) -> Option<Case> {
    Some(splice(base, |o| {
        o["actors"] = json!([{"name": "mallory"}]);
    }))
}

fn t2(base: &Base) -> Option<Case> {
    base.has_block().then(|| splice(base, |o| o["blocks"][0]["stats"] = json!({"min": 0})))
}

fn t3(base: &Base) -> Option<Case> {
    Some(splice(base, |o| {
        let n = format!("{}-edited", o["name"].as_str().unwrap());
        o["name"] = Value::String(n);
    }))
}

fn t4(base: &Base) -> Option<Case> {
    // A: the stored text is in struct order; a key-sorted rewrite is the same object.
    let a = serde_json::to_string_pretty(&base.obj).unwrap();
    // A′: the top-level members of the JCS text, reversed.
    let Value::Object(m) = &base.bare else { unreachable!() };
    let members: Vec<String> =
        m.iter().map(|(k, v)| format!("{}:{}", serde_json::to_string(k).unwrap(), String::from_utf8(raw_jcs(v)).unwrap())).collect();
    let rev = format!("{{{}}}", members.into_iter().rev().collect::<Vec<_>>().join(","));
    let b = cbor::encode_loose(&cbor::from_json(&base.bare, false), Loose { reverse_keys: true, ..Default::default() });
    Some(Case { a: Some(a), ap: Some((rev.into_bytes(), Some(base.seal.clone()))), b: Some((b, Some(base.b_seal.clone()))) })
}

fn t5(base: &Base) -> Option<Case> {
    let a = serde_json::to_string(&base.obj).unwrap(); // compact instead of pretty
    let mut ap = base.ap.clone();
    ap.insert(1, b' ');
    Some(Case { a: Some(a), ap: Some((ap, Some(base.seal.clone()))), b: None })
}

fn t6(base: &Base) -> Option<Case> {
    let ap = respell_escape(std::str::from_utf8(&base.ap).unwrap()).into_bytes();
    Some(Case { a: Some(respell_escape(&base.text)), ap: Some((ap, Some(base.seal.clone()))), b: None })
}

fn t7(base: &Base) -> Option<Case> {
    if !base.has_array_block() {
        return None;
    }
    let a = respell_shape(&base.text)?;
    let ap = respell_shape(std::str::from_utf8(&base.ap).unwrap())?.into_bytes();
    let b = cbor::encode_loose(&cbor::from_json(&base.bare, false), Loose { wide_ints: true, ..Default::default() });
    Some(Case { a: Some(a), ap: Some((ap, Some(base.seal.clone()))), b: Some((b, Some(base.b_seal.clone()))) })
}

fn t8(base: &Base) -> Option<Case> {
    let a = after_brace(&base.text, "\"name\": \"mallory\",");
    let ap = after_brace(std::str::from_utf8(&base.ap).unwrap(), "\"name\":\"mallory\",").into_bytes();
    let b = dup_name_cbor(&base.bare);
    Some(Case { a: Some(a), ap: Some((ap, Some(base.seal.clone()))), b: Some((b, Some(base.b_seal.clone()))) })
}

/// A CBOR map with `name` twice, keys otherwise in canonical order.
fn dup_name_cbor(bare: &Value) -> Vec<u8> {
    let cbor::Value::Map(mut es) = cbor::from_json(bare, false) else { unreachable!() };
    es.push(("name".into(), cbor::Value::Text("mallory".into())));
    cbor::encode_loose(&cbor::Value::Map(es), Loose::default())
}

fn t9(base: &Base) -> Option<Case> {
    Some(Case {
        a: Some(serde_json::to_string_pretty(&base.bare).unwrap()),
        ap: Some((base.ap.clone(), None)),
        b: Some((base.b.clone(), None)),
    })
}

/// The JCS text of `bare` with the integer text `from` replaced by `to` (a writer that keeps the
/// literal it was given instead of JCS's double form).
fn literal(bare: &Value, from: &str, to: &str) -> Vec<u8> {
    let t = String::from_utf8(raw_jcs(bare)).unwrap();
    assert!(t.contains(from), "{from}");
    t.replacen(from, to, 1).into_bytes()
}

fn t10(base: &Base) -> Option<Case> {
    // The sealed bytes hold the literal 9007199254740993; the served bytes hold 9007199254740992.
    let sealed_obj = with_metadata(&base.bare, "seed", json!(9007199254740993u64));
    let served = with_metadata(&base.bare, "seed", json!(9007199254740992u64));
    // A: JCS writes both as 9007199254740992, so the seal over the sealed object IS the seal over the
    // served one; only the I-JSON rule stands between this and a verify (results/numbers.txt).
    let a_seal = digest(&raw_jcs(&sealed_obj));
    let ap_sealed = literal(&sealed_obj, "9007199254740992", "9007199254740993");
    Some(Case {
        a: Some(a_with_seal(&served, &a_seal)),
        ap: Some((raw_jcs(&served), Some(digest(&ap_sealed)))),
        b: Some((b_of(&served), Some(digest(&b_of(&sealed_obj))))),
    })
}

fn s1(base: &Base) -> Option<Case> {
    // The attacker seals the last-wins reading: `name` = the original, a first `name` = mallory.
    let a = after_brace(&base.text, "\"name\": \"mallory\",");
    let ap = after_brace(std::str::from_utf8(&base.ap).unwrap(), "\"name\":\"mallory\",").into_bytes();
    Some(Case { a: Some(a), ap: Some(sealed(ap)), b: Some(sealed(dup_name_cbor(&base.bare))) })
}

fn honest(bare: &Value) -> Case {
    Case { a: Some(a_sealed(bare)), ap: Some(sealed(raw_jcs(bare))), b: Some(sealed(b_of(bare))) }
}

fn s2(base: &Base) -> Option<Case> {
    if !base.has_block() {
        return None;
    }
    Some(honest(&reseal_blocks(&base.bare, |bs| {
        let first = bs[0].clone();
        bs.push(first);
    })))
}

fn s3(base: &Base) -> Option<Case> {
    if !base.has_block() {
        return None;
    }
    let mut o = base.bare.clone();
    let b0 = o["blocks"][0].as_object().unwrap().clone();
    let positional: Vec<Value> = ["name", "kind", "digest", "spec"].iter().map(|k| b0[*k].clone()).collect();
    o["blocks"][0] = Value::Array(positional);
    Some(honest(&o))
}

fn s4(base: &Base) -> Option<Case> {
    if !base.has_block() {
        return None;
    }
    let mut o = base.bare.clone();
    let k = o["blocks"][0]["kind"].as_str().unwrap().to_owned();
    o["blocks"][0]["kind"] = json!({ k: null });
    Some(honest(&o))
}

fn s5(base: &Base) -> Option<Case> {
    // The attacker's writer keeps the literal; JCS would have rounded it to …800.
    let o = with_metadata(&base.bare, "daq_start_ns", json!(1_730_000_000_123_456_789u64));
    let mut c = honest(&o);
    c.ap = Some(sealed(literal(&o, "1730000000123456800", "1730000000123456789")));
    Some(c)
}

fn s5b(base: &Base) -> Option<Case> {
    // An integer beyond 2^53 that a double holds exactly: its text IS its own JCS form (numbers.txt), so
    // the A′ byte comparison would pass it; the I-JSON rule (run inside parse_strict, first) refuses it.
    Some(honest(&with_metadata(&base.bare, "daq_start_ns", json!(1_730_000_000_123_456_800u64))))
}

fn s6(base: &Base) -> Option<Case> {
    let mut o = base.bare.clone();
    o["description"] = Value::String("XYZ".into()); // 3 bytes, patched below
    let jcs = String::from_utf8(raw_jcs(&o)).unwrap().replacen("\"XYZ\"", "\"\\ud800\"", 1);
    let mut b = b_of(&o);
    let at = b.windows(4).position(|w| w == [0x63, b'X', b'Y', b'Z']).unwrap();
    b[at + 1..at + 4].copy_from_slice(&[0xed, 0xa0, 0x80]); // CESU-8 for U+D800: not UTF-8
    let a = a_with_seal(&o, &base.seal).replacen("\"XYZ\"", "\"\\ud800\"", 1);
    Some(Case { a: Some(a), ap: Some(sealed(jcs.into_bytes())), b: Some(sealed(b)) })
}

fn s7(base: &Base) -> Option<Case> {
    if !base.has_array_block() {
        return None;
    }
    let a = respell_shape(&a_sealed(&base.bare))?; // the seal over 64.0 equals the seal over 64
    let ap = respell_shape(std::str::from_utf8(&base.ap).unwrap())?.into_bytes();
    let mut o = base.bare.clone();
    let i = o["blocks"].as_array().unwrap().iter().position(|b| b["kind"] == "array").unwrap();
    let s0 = o["blocks"][i]["spec"]["shape"][0].as_u64().unwrap();
    o["blocks"][i]["spec"]["shape"][0] = json!(s0 as f64);
    // B: the attacker writes the float on purpose (profile B would reduce it to the integer).
    let b = cbor::encode_as(&cbor::from_json(&o, false), cbor::Profile::B0).unwrap();
    Some(Case { a: Some(a), ap: Some(sealed(ap)), b: Some(sealed(b)) })
}

/// S7's B bytes verified under profile B0 (no numeric reduction): what the reduction rule buys.
pub fn s7_b0(base: &Base) -> Option<String> {
    let c = s7(base)?;
    let (b, seal) = c.b?;
    let v = options::verify_b_as(&b, seal.as_deref(), cbor::Profile::B0);
    Some(post(v, || cbor::decode_strict_as(&b, cbor::Profile::B0).ok().and_then(|c| cbor::to_serde_json(&c).ok())))
}

fn s8(base: &Base) -> Option<Case> {
    let pretty = serde_json::to_string_pretty(&base.bare).unwrap().into_bytes();
    let b = cbor::encode_loose(&cbor::from_json(&base.bare, false), Loose { reverse_keys: true, ..Default::default() });
    Some(Case { a: Some(base.text.clone()), ap: Some(sealed(pretty)), b: Some(sealed(b)) })
}

// --------------------------------------------------------------- running

/// After a seal verifies: can the Rust typed block layer read every array spec? This is where a
/// respelled number that kept its seal surfaces (Rust refuses `64.0` for a `u64`, JavaScript reads 64).
fn array_specs(obj: &Value) -> Result<(), String> {
    for b in obj["blocks"].as_array().into_iter().flatten() {
        if b["kind"] == "array" {
            serde_json::from_value::<ArraySpec>(b["spec"].clone()).map_err(|e| format!("typed block read: {e}"))?;
        }
    }
    Ok(())
}

pub fn run_a(text: &str) -> String {
    let v = options::verify_a(text);
    post(v, || serde_json::from_str::<Value>(text).ok())
}

pub fn run_ap(bytes: &[u8], seal: Option<&str>) -> String {
    let v = options::verify_aprime(bytes, seal);
    post(v, || serde_json::from_slice::<Value>(bytes).ok())
}

pub fn run_b(bytes: &[u8], seal: Option<&str>) -> String {
    let v = options::verify_b(bytes, seal);
    post(v, || cbor::decode_strict(bytes).ok().and_then(|c| cbor::to_serde_json(&c).ok()))
}

fn post(v: Verdict, obj: impl FnOnce() -> Option<Value>) -> String {
    match &v {
        Verdict::Verified(_) => match obj().map(|o| array_specs(&o)) {
            Some(Err(e)) => format!("verifies, then {e}"),
            _ => "verifies".into(),
        },
        _ => v.short(),
    }
}

/// A verdict reduced to its class, so 13 products' outcomes can be counted together.
pub fn class(s: &str) -> String {
    let rules: &[(&str, &str)] = &[
        ("typed block read", "verifies; typed read refuses"),
        ("caught: seal mismatch", "caught: seal mismatch"),
        ("non-canonical", "refused: non-canonical bytes"),
        ("listed more than once", "refused: duplicate block name (typed rule)"),
        ("duplicate key", "refused: duplicate key"),
        ("appears twice", "refused: duplicate key"),
        ("hex escape", "refused: lone surrogate (JSON parser)"),
        ("integral: numeric reduction", "refused: integral float (numeric reduction)"),
        ("2^53", "refused: I-JSON integer"),
        ("surrogate", "refused: lone surrogate"),
        ("not valid UTF-8", "refused: invalid UTF-8"),
        ("out of canonical order", "refused: CBOR key order"),
        ("shortest form", "refused: CBOR non-shortest int"),
        ("seal missing", "refused: seal missing"),
        ("expected", "refused: typed layer (declared form)"),
        ("spelling", "refused: typed layer (declared form)"),
        ("invalid type", "refused: typed layer (declared form)"),
        ("verifies", "verifies"),
    ];
    for (needle, cls) in rules {
        if s.contains(needle) {
            return (*cls).into();
        }
    }
    s.chars().take(70).collect()
}

/// Sanity: tessera-core reads the unmodified A text (used by the runner's self-check).
pub fn a_reads(text: &str) -> bool {
    Manifest::from_json_verified(text).is_ok()
}
