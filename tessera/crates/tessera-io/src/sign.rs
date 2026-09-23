//! Sign / verify a sealed **`.tsra` container** end-to-end (S16 / #209 + ADR-0042). Ties
//! [`tessera_core::signing`] (the detached-signature envelope over `manifest_hash`) to a product on
//! disk via **an embedded aux member** `aux/signatures/<key_id>.sig.json` (default, ADR-0042) OR a
//! **`<file>.tsra.sig.json`** detached sidecar (opt-in via `--sidecar`).
//!
//! The embedded form is the "one file to share" default from #259: the signature rides *inside* the
//! zip as a non-sealed aux member (outside `content_hash` / `manifest_hash`, so it can sit inside
//! the container without a circular hash), and `verify-sig` reads it from within. A `.tsra` shared
//! by itself never loses its signature. The detached form stays supported for legacy operators and
//! for the OCI-double-layer distribution path (ADR-0037 §0 bug #2). Both use the SAME `ed25519-env-v1`
//! envelope over `manifest_hash` — only the LOCATION of the signature JSON moves.

use std::path::{Path, PathBuf};

use tessera_core::signing::{self, SignOpts, Signature, SigningKey, VerifyingKey};
use tessera_core::{Error, Result};

use crate::container::{write_aux_members, AuxMember, Reader, AUX_PREFIX, AUX_SIGNATURES_PREFIX};

/// The conventional **detached** sidecar path for a container: `<file>.tsra` → `<file>.tsra.sig.json`.
/// Kept for `tessera sign --sidecar` and for the OCI-double-layer distribution path.
pub fn sidecar_path(tsra: &Path) -> PathBuf {
    let mut s = tsra.as_os_str().to_os_string();
    s.push(".sig.json");
    PathBuf::from(s)
}

/// The **embedded** signature entry name (relative to `aux/`) for a given `key_id`:
/// `aux/signatures/<key_id>.sig.json` inside the `.tsra`. Multiple signers write to distinct entries
/// (one per `key_id`), so a co-signed product carries every signature in the same file.
pub fn embedded_signature_name(key_id: &str) -> String {
    format!("signatures/{key_id}.sig.json")
}

/// Sign a sealed `.tsra` and **embed** the signature as an `aux/signatures/<key_id>.sig.json` member
/// inside the container (ADR-0042 default). Reads the sealed manifest, signs its `manifest_hash`
/// (ed25519) with the given [`SignOpts`], clears any prior `aux/signatures/*` members, and writes
/// the new signature JSON as a non-sealed aux member. Single-signer semantics: **each `sign` call
/// replaces the signature roster**, so a re-sign never leaves a stale signature behind (co-signing
/// is a future verb, not a byproduct of `sign`). The write is seal-preserving by
/// [`write_aux_members`] — `content_hash` and `manifest_hash` are byte-identical to the pre-sign
/// container.
pub fn sign_tsra(tsra: &Path, key: &SigningKey, opts: &SignOpts) -> Result<Signature> {
    let reader = Reader::open(tsra)?;
    let env = signing::sign_manifest(reader.manifest(), key, opts)?;
    drop(reader); // release the read handle before we rewrite the archive
    let json = serde_json::to_vec_pretty(&env).map_err(|e| Error::Container(e.to_string()))?;
    let name = embedded_signature_name(&env.key_id);
    // Sub-path (relative to `aux/`) of the signatures subtree — the mechanism `write_aux_members`
    // uses to drop the whole prior signature roster before landing the new one.
    let signatures_subpath = AUX_SIGNATURES_PREFIX
        .strip_prefix(AUX_PREFIX)
        .unwrap_or(AUX_SIGNATURES_PREFIX);
    write_aux_members(tsra, &[AuxMember::new(name, json)], &[signatures_subpath])?;
    tracing::info!(
        target: "tessera::sign",
        file = %tsra.display(),
        key_id = %env.key_id,
        signer = env.signer.as_deref().unwrap_or("-"),
        embedded = true,
        "signed container"
    );
    Ok(env)
}

/// Sign a sealed `.tsra` and write the **detached** `<file>.tsra.sig.json` sidecar (legacy /
/// `tessera sign --sidecar`). Same crypto/envelope as [`sign_tsra`]; only the location differs.
pub fn sign_tsra_sidecar(tsra: &Path, key: &SigningKey, opts: &SignOpts) -> Result<Signature> {
    let reader = Reader::open(tsra)?;
    let env = signing::sign_manifest(reader.manifest(), key, opts)?;
    let json = serde_json::to_vec_pretty(&env).map_err(|e| Error::Container(e.to_string()))?;
    std::fs::write(sidecar_path(tsra), json)?;
    tracing::info!(
        target: "tessera::sign",
        file = %tsra.display(),
        key_id = %env.key_id,
        signer = env.signer.as_deref().unwrap_or("-"),
        embedded = false,
        "signed container (detached sidecar)"
    );
    Ok(env)
}

/// Read a [`Signature`] for a container: **embedded first** (`aux/signatures/<key_id>.sig.json`
/// inside the `.tsra`), then falls back to the detached `<file>.tsra.sig.json` sidecar. This is the
/// symmetry ADR-0042 needs — `verify-sig` doesn't care where the signature lives, only that the
/// envelope binds the sealed `manifest_hash`. When multiple embedded signatures are present (a
/// co-signed product), the FIRST in central-directory order is returned; a caller that needs a
/// specific `key_id` can iterate via [`Reader::aux_names`] and [`Reader::read_aux`].
pub fn read_signature(tsra: &Path) -> Result<Signature> {
    // 1. Embedded (ADR-0042). Prefer this over any detached sidecar so a "shared just the .tsra"
    //    workflow always resolves to what the file actually carries.
    if let Some(env) = read_embedded_signature(tsra)? {
        return Ok(env);
    }
    // 2. Detached (legacy). A file that was signed with `--sidecar` (or an older Tessera) still
    //    verifies unchanged — the mechanism ladder from ADR-0037 §0 bug #2 stays intact.
    let bytes = std::fs::read(sidecar_path(tsra))?;
    serde_json::from_slice(&bytes).map_err(|e| Error::Container(e.to_string()))
}

/// Read the embedded signature (`aux/signatures/<first>.sig.json`) from a `.tsra` if present; `Ok(None)`
/// if the container carries no embedded signature. The whole entry is the JSON of a `Signature`.
pub fn read_embedded_signature(tsra: &Path) -> Result<Option<Signature>> {
    let mut r = Reader::open(tsra)?;
    let names = r.aux_names();
    // Any aux member under `signatures/` is a signature — first one wins (deterministic:
    // central-directory order = write order = single-signer's only entry, or the earliest signer
    // for a co-signed product).
    let sig_name = match names.into_iter().find(|n| {
        n.strip_prefix("signatures/")
            .is_some_and(|rest| rest.ends_with(".sig.json"))
    }) {
        Some(n) => n,
        None => return Ok(None),
    };
    let bytes = r.read_aux(&sig_name)?;
    let env: Signature =
        serde_json::from_slice(&bytes).map_err(|e| Error::Container(e.to_string()))?;
    Ok(Some(env))
}

/// True when the `.tsra` carries an embedded aux signature under `aux/signatures/`. Cheap surface
/// for `tree` / `inspect` — no signature parsing.
pub fn has_embedded_signature(tsra: &Path) -> Result<bool> {
    let r = Reader::open(tsra)?;
    Ok(r.aux_names()
        .into_iter()
        .any(|n| n.starts_with(AUX_SIGNATURES_PREFIX.trim_start_matches("aux/"))))
}

/// Verify a sealed `.tsra` against its signature (embedded first, detached fallback) + the signer's
/// Ed25519 public key. Opens the container, reads its sealed manifest, and checks the signature —
/// `true` only if the product is unaltered since signing and the key matches. Any tamper (a swapped
/// block, edited metadata, a forged signature) fails.
pub fn verify_tsra(tsra: &Path, key: &VerifyingKey) -> Result<bool> {
    let reader = Reader::open(tsra)?;
    let manifest = reader.manifest().clone();
    drop(reader);
    let env = read_signature(tsra)?;
    Ok(signing::verify_manifest(&manifest, &env, key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::array::{self, ArrayData};
    use crate::container::pack;
    use tessera_core::block::array::ArraySpec;
    use tessera_core::ProductBuilder;

    fn sealed_tsra(dir: &Path, name: &str, n: i16) -> PathBuf {
        let mut spec = ArraySpec::new(vec![8, 8, 8], "int16");
        spec.codec = "pcodec".into();
        let data = ArrayData::I16((0..512).map(|k| k as i16 + n).collect());
        let (bref, payload) = array::array_block("volume", &spec, &data).unwrap();
        let mut b = ProductBuilder::new("recon", "DP", "signed", "2024-01-01T00:00:00Z");
        b.add_block_ref(bref);
        let manifest = b.seal().unwrap();
        let out = dir.join(name);
        pack(&manifest, &[payload], &out).unwrap();
        out
    }

    #[test]
    fn sign_embeds_the_signature_and_verify_round_trips() {
        // ADR-0042 default: sign_tsra writes the signature INSIDE the .tsra as an aux member,
        // and verify_tsra reads it from within. The sealed manifest is unchanged (same file, same
        // hashes) — the whole point of the embedded form.
        let dir = tempfile::tempdir().unwrap();
        let key = SigningKey::from_bytes(&[42; 32]);
        let tsra = sealed_tsra(dir.path(), "data.tsra", 0);

        // Snapshot the pre-sign sealed hashes — must survive the sign.
        let (pre_ch, pre_mh) = {
            let r = Reader::open(&tsra).unwrap();
            (
                r.manifest().content_hash.clone(),
                r.manifest().manifest_hash.clone(),
            )
        };

        let env = sign_tsra(
            &tsra,
            &key,
            &SignOpts {
                signer: Some("https://orcid.org/0000-0002-1825-0097".into()),
                ..Default::default()
            },
        )
        .unwrap();
        // The detached sidecar does NOT exist — signature lives inside the container.
        assert!(!sidecar_path(&tsra).exists());
        // The embedded aux member is present under `aux/signatures/<key_id>.sig.json`.
        assert!(has_embedded_signature(&tsra).unwrap());
        // The sealed hashes are byte-identical (seal-ignore invariant).
        let post = Reader::open(&tsra).unwrap();
        assert_eq!(post.manifest().content_hash, pre_ch);
        assert_eq!(post.manifest().manifest_hash, pre_mh);
        drop(post);
        // Verify succeeds against the embedded signature.
        assert!(verify_tsra(&tsra, &key.verifying_key()).unwrap());
        // read_signature prefers the embedded envelope and returns exactly what sign_tsra wrote.
        assert_eq!(read_signature(&tsra).unwrap(), env);

        // A different signer's key fails.
        let other = SigningKey::from_bytes(&[7; 32]);
        assert!(!verify_tsra(&tsra, &other.verifying_key()).unwrap());
    }

    #[test]
    fn detached_sidecar_still_works_and_is_fallback_read() {
        // sign_tsra_sidecar (opt-in via --sidecar) writes the legacy <file>.tsra.sig.json, and
        // read_signature falls back to it when no embedded signature is present.
        let dir = tempfile::tempdir().unwrap();
        let key = SigningKey::from_bytes(&[43; 32]);
        let tsra = sealed_tsra(dir.path(), "sidecar.tsra", 0);
        let env = sign_tsra_sidecar(
            &tsra,
            &key,
            &SignOpts {
                signer: Some("https://orcid.org/0000-0002-1825-0097".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(sidecar_path(&tsra).exists());
        assert!(!has_embedded_signature(&tsra).unwrap());
        assert!(verify_tsra(&tsra, &key.verifying_key()).unwrap());
        assert_eq!(read_signature(&tsra).unwrap(), env);

        // A different product with the same sidecar copied over fails — the attacker can't move a
        // valid signature onto a different product. (Same guarantee as before, at the detached level.)
        let tampered = sealed_tsra(dir.path(), "tampered.tsra", 1);
        std::fs::copy(sidecar_path(&tsra), sidecar_path(&tampered)).unwrap();
        assert!(!verify_tsra(&tampered, &key.verifying_key()).unwrap());
    }

    #[test]
    fn re_sign_replaces_the_prior_embedded_signature() {
        // Single-signer semantics: each `sign` call clears any previous `aux/signatures/*` before
        // writing the new signature. So a re-sign with a different key produces a `.tsra` that
        // verifies ONLY under the second key — no stale signatures survive to confuse `verify-sig`.
        let dir = tempfile::tempdir().unwrap();
        let old_key = SigningKey::from_bytes(&[9; 32]);
        let new_key = SigningKey::from_bytes(&[10; 32]);
        let tsra = sealed_tsra(dir.path(), "resign.tsra", 0);
        sign_tsra(&tsra, &old_key, &SignOpts::default()).unwrap();
        sign_tsra(&tsra, &new_key, &SignOpts::default()).unwrap();
        assert!(verify_tsra(&tsra, &new_key.verifying_key()).unwrap());
        assert!(!verify_tsra(&tsra, &old_key.verifying_key()).unwrap());
        // Only ONE embedded signature remains.
        let r = Reader::open(&tsra).unwrap();
        let sig_count = r
            .aux_names()
            .into_iter()
            .filter(|n| n.starts_with("signatures/"))
            .count();
        assert_eq!(
            sig_count, 1,
            "re-sign must leave exactly one embedded signature"
        );
    }

    #[test]
    fn embedded_takes_precedence_over_a_detached_sidecar() {
        // Ambiguity resolution: if BOTH forms are present, the embedded signature wins (a .tsra
        // that carries its own signature is the source of truth).
        let dir = tempfile::tempdir().unwrap();
        let embedded_key = SigningKey::from_bytes(&[5; 32]);
        let detached_key = SigningKey::from_bytes(&[6; 32]);
        let tsra = sealed_tsra(dir.path(), "both.tsra", 0);
        // Sign detached with one key, then embed with a different one.
        sign_tsra_sidecar(&tsra, &detached_key, &SignOpts::default()).unwrap();
        sign_tsra(&tsra, &embedded_key, &SignOpts::default()).unwrap();
        // Verify passes with the EMBEDDED key (chosen), fails with the DETACHED-only key.
        assert!(verify_tsra(&tsra, &embedded_key.verifying_key()).unwrap());
        assert!(!verify_tsra(&tsra, &detached_key.verifying_key()).unwrap());
    }
}
