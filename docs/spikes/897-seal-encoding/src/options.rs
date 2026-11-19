//! The three sealing options as verifiers over one stored manifest.
//!
//! - **A**: tessera-core as PR #894 ships it. The stored text carries `manifest_hash`; the verifier
//!   strict-parses (duplicate keys, I-JSON) and hashes `JCS(parsed − manifest_hash)`.
//! - **A′**: the stored `manifest.json` bytes are exactly `JCS(manifest)` with no `manifest_hash`
//!   key; the seal is `digest(stored bytes)` and lives outside them. The verifier strict-parses,
//!   refuses bytes that are not their own JCS form, compares the seal, then runs the typed layer.
//! - **B**: the stored manifest is profile CBOR (`cbor.rs`) with no `manifest_hash` key; the seal is
//!   `digest(stored bytes)`. The verifier decodes strictly, compares the seal, then runs the typed layer.
//!
//! The typed layer (declared-form check, unique block names, `id` and `content_hash` recomputation) is
//! tessera-core's for every option, so the comparison isolates the encoding and the seal.

use serde_json::Value;
use tessera_core::canonical;
use tessera_core::hash::digest;
use tessera_core::Manifest;

use crate::cbor;

/// What a verifier concluded about one stored manifest.
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// Every check passed; the seal it verified.
    Verified(String),
    /// Refused before or without comparing the seal (parse, canonical form, typed layer).
    Refused(String),
    /// The bytes parsed but the recorded seal is not theirs.
    SealMismatch,
}

impl Verdict {
    pub fn short(&self) -> String {
        match self {
            Verdict::Verified(_) => "verifies".into(),
            Verdict::SealMismatch => "caught: seal mismatch".into(),
            Verdict::Refused(e) => format!("refused: {}", first_line(e)),
        }
    }
}

fn first_line(e: &str) -> String {
    let l = e.lines().next().unwrap_or("");
    if l.chars().count() > 110 {
        format!("{}…", l.chars().take(110).collect::<String>())
    } else {
        l.to_owned()
    }
}

// ------------------------------------------------------------------ A

/// Option A: tessera-core's own reader.
pub fn verify_a(text: &str) -> Verdict {
    match Manifest::from_json_verified(text) {
        Ok(m) => Verdict::Verified(m.manifest_hash.unwrap_or_default()),
        Err(e) => {
            let s = e.to_string();
            if s.contains("(absent)") {
                Verdict::Refused(format!("seal missing: {s}"))
            } else if s.contains("manifest_hash") && s.contains("mismatch") {
                Verdict::SealMismatch
            } else {
                Verdict::Refused(s)
            }
        }
    }
}

// ------------------------------------------------------------------ A′

/// The A′ stored form of a parsed manifest: JCS bytes with the `manifest_hash` key removed.
pub fn aprime_bytes(object: &Value) -> Vec<u8> {
    canonical::bytes_without(object, "manifest_hash").expect("a manifest is a JSON object")
}

/// Option A′ verifier: `seal` is the externally stored seal (a `manifest.seal` member in the
/// prototype container).
pub fn verify_aprime(bytes: &[u8], seal: Option<&str>) -> Verdict {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Verdict::Refused("not UTF-8".into());
    };
    let object = match canonical::parse_strict(text) {
        Ok(v) => v,
        Err(e) => return Verdict::Refused(e.to_string()),
    };
    let jcs = canonical::to_bytes(&object).expect("parse_strict output serialises");
    if jcs != bytes {
        let at = jcs.iter().zip(bytes).position(|(a, b)| a != b).unwrap_or(jcs.len().min(bytes.len()));
        return Verdict::Refused(format!("non-canonical bytes: differ from their JCS form at byte {at}"));
    }
    if object.get("manifest_hash").is_some() {
        return Verdict::Refused("an A′ manifest carries no manifest_hash key".into());
    }
    let Some(seal) = seal else {
        return Verdict::Refused("seal missing".into());
    };
    if digest(bytes) != seal {
        return Verdict::SealMismatch;
    }
    match typed(text) {
        Ok(()) => Verdict::Verified(seal.to_owned()),
        Err(e) => Verdict::Refused(e),
    }
}

/// tessera-core's typed layer over a manifest that carries no `manifest_hash`: declared form,
/// version, unique block names, `id` and `content_hash`.
fn typed(text: &str) -> Result<(), String> {
    let m = Manifest::from_json(text).map_err(|e| e.to_string())?;
    tessera_core::block::check_unique_names(m.blocks.iter().map(|b| &b.name)).map_err(|e| e.to_string())?;
    let id = m.recompute_id().map_err(|e| e.to_string())?;
    if id != m.id {
        return Err(format!("id mismatch: recorded {}, recomputed {id}", m.id));
    }
    let ch = m.recompute_content_hash().map_err(|e| e.to_string())?;
    if m.content_hash.as_deref() != Some(ch.as_str()) {
        return Err("content_hash mismatch".into());
    }
    Ok(())
}

// ------------------------------------------------------------------ B

/// The B stored form of a parsed manifest: profile-`B` CBOR without the `manifest_hash` key.
pub fn b_bytes(object: &Value, digests_as_bytes: bool) -> Vec<u8> {
    b_bytes_as(object, digests_as_bytes, cbor::Profile::B)
}

/// The stored form under either CBOR profile.
pub fn b_bytes_as(object: &Value, digests_as_bytes: bool, p: cbor::Profile) -> Vec<u8> {
    let mut o = object.clone();
    o.as_object_mut().expect("object").remove("manifest_hash");
    cbor::encode_as(&cbor::from_json(&o, digests_as_bytes), p).expect("a JSON manifest encodes")
}

/// Option B verifier. The typed layer is reached through a JSON bridge so it is tessera-core's own;
/// integers beyond ±(2^53−1) are masked to `0` on that bridge only, because the bridge re-enters
/// tessera-core's I-JSON rule, which does not exist under B (the decoded value keeps them exactly).
pub fn verify_b(bytes: &[u8], seal: Option<&str>) -> Verdict {
    verify_b_as(bytes, seal, cbor::Profile::B)
}

/// The B verifier under either CBOR profile (`B0` = no numeric reduction).
pub fn verify_b_as(bytes: &[u8], seal: Option<&str>, p: cbor::Profile) -> Verdict {
    let v = match cbor::decode_strict_as(bytes, p) {
        Ok(v) => v,
        Err(e) => return Verdict::Refused(format!("strict CBOR decoder: {e}")),
    };
    let Some(seal) = seal else {
        return Verdict::Refused("seal missing".into());
    };
    if digest(bytes) != seal {
        return Verdict::SealMismatch;
    }
    let json = match cbor::to_serde_json(&mask_wide_ints(&v)) {
        Ok(j) => j,
        Err(e) => return Verdict::Refused(e),
    };
    match typed(&serde_json::to_string(&json).unwrap()) {
        Ok(()) => Verdict::Verified(seal.to_owned()),
        Err(e) => Verdict::Refused(e),
    }
}

fn mask_wide_ints(v: &cbor::Value) -> cbor::Value {
    const MAX: u64 = (1 << 53) - 1;
    use cbor::Value as C;
    match v {
        C::Uint(n) if *n > MAX => C::Uint(0),
        C::Nint(n) if *n >= MAX => C::Uint(0),
        C::Array(a) => C::Array(a.iter().map(mask_wide_ints).collect()),
        C::Map(m) => C::Map(m.iter().map(|(k, x)| (k.clone(), mask_wide_ints(x))).collect()),
        other => other.clone(),
    }
}
