//! `tessera keygen` + a file-based **trust store** (`tessera trust add/list/remove`) + the trust-store
//! resolution that `verify-sig` defaults to (ADR-0037 alpha tier).
//!
//! A public key is **not secret** — trust in it is a *local* decision: a curated set of `<name>.pub`
//! files, NOT a hosted PKI. `verify-sig` looks up the sidecar's `key_id` in the store and answers
//! "intact **and** signed by a key you trust" — closing the impersonation hole (re-signing with the
//! attacker's own key) that a bare `--pubkey` check can't catch. As elsewhere, the verbs write to a
//! caller-supplied `Write` so they're unit-testable; `main` owns stdout.

use std::io::Write;
use std::path::{Path, PathBuf};

use tessera_core::signing::{self, SigningKey};
use tessera_core::{Error, Result};
use tessera_io::sign::read_signature;

// ---- keygen ----

/// Generate a fresh ed25519 keypair: the 32-byte seed (private, mode 0600) to `out`, the public key
/// to `<out>.pub`. Keep the seed secret; the `.pub` (= the `key_id`) travels freely.
pub fn keygen(out: &Path, w: &mut dyn Write) -> Result<()> {
    let mut seed = [0u8; 32];
    getrandom::getrandom(&mut seed)
        .map_err(|e| Error::Invalid(format!("keygen: OS RNG failed: {e}")))?;
    let key = SigningKey::from_bytes(&seed);
    let key_id = signing::verifying_key_hex(&key.verifying_key());
    let pub_path = with_suffix(out, ".pub");
    write_private(out, &signing::signing_key_hex(&key))?;
    std::fs::write(&pub_path, &key_id).map_err(Error::from)?;
    writeln!(
        w,
        "wrote private key {} (keep secret — mode 0600)",
        out.display()
    )
    .map_err(Error::from)?;
    writeln!(w, "wrote public  key {} (share freely)", pub_path.display()).map_err(Error::from)?;
    writeln!(w, "  key_id {key_id}").map_err(Error::from)?;
    Ok(())
}

/// Load an ed25519 **signing** key from `path`, auto-detecting the encoding, and return it with a
/// `key_format` tag (recorded in the signature envelope). Two formats:
///   * **`ssh-ed25519`** — an OpenSSH private key (`-----BEGIN OPENSSH PRIVATE KEY-----`), so a
///     researcher can sign with the same `~/.ssh/id_ed25519` they already hold. The *crypto* is
///     identical (ed25519 over the same envelope) — only the key container differs, so `key_id` is the
///     same raw-pubkey hex either way and any verifier checks the signature unchanged.
///   * **`raw-hex`** — a 32-byte hex seed (what `tessera keygen` writes).
pub fn load_signing_key(path: &Path) -> Result<(SigningKey, &'static str)> {
    let text = std::fs::read_to_string(path).map_err(Error::from)?;
    if text.contains("OPENSSH PRIVATE KEY") {
        let ssh = ssh_key::PrivateKey::from_openssh(text.trim())
            .map_err(|e| Error::Invalid(format!("not a valid OpenSSH private key: {e}")))?;
        if ssh.is_encrypted() {
            return Err(Error::Invalid(
                "the ssh key is passphrase-encrypted; decrypt it first \
                 (`ssh-keygen -p -N '' -f <key>`) or export a raw hex seed"
                    .into(),
            ));
        }
        let pair = ssh.key_data().ed25519().ok_or_else(|| {
            Error::Invalid(
                "the ssh key is not ed25519 — only ssh-ed25519 keys can sign tessera products"
                    .into(),
            )
        })?;
        Ok((
            SigningKey::from_bytes(&pair.private.to_bytes()),
            "ssh-ed25519",
        ))
    } else {
        Ok((signing::signing_key_from_hex(&text)?, "raw-hex"))
    }
}

/// The current instant as an RFC-3339 UTC string — the `signed_at` stamp bound into a signature.
pub fn now_rfc3339() -> String {
    use time::format_description::well_known::Rfc3339;
    time::OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_default()
}

fn with_suffix(p: &Path, suffix: &str) -> PathBuf {
    let mut s = p.as_os_str().to_os_string();
    s.push(suffix);
    PathBuf::from(s)
}

#[cfg(unix)]
fn write_private(path: &Path, contents: &str) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(Error::from)?;
    f.write_all(contents.as_bytes()).map_err(Error::from)?;
    Ok(())
}

#[cfg(not(unix))]
fn write_private(path: &Path, contents: &str) -> Result<()> {
    std::fs::write(path, contents).map_err(Error::from)
}

/// Generate an `age` X25519 keypair for crypto-shred de-identification (ADR-0047 / `keygen --age`).
/// The secret (`AGE-SECRET-KEY-1…`, mode 0600) goes to `out` — pass it to `tessera reidentify
/// --identity`; the recipient public key (`age1…`) goes to `OUT.pub` — pass it to
/// `tessera ingest … --recipient`. Distinct from the ed25519 **signing** key [`keygen`] writes.
pub fn keygen_age(out: &Path, w: &mut dyn Write) -> Result<()> {
    let (secret, public) = tessera_ingest::identity::generate_keypair();
    let pub_path = with_suffix(out, ".pub");
    write_private(out, &format!("{secret}\n"))?;
    std::fs::write(&pub_path, format!("{public}\n")).map_err(Error::from)?;
    writeln!(
        w,
        "wrote age identity {} (keep secret — mode 0600; for `tessera reidentify --identity`)",
        out.display()
    )
    .map_err(Error::from)?;
    writeln!(
        w,
        "wrote age recipient {} (share freely; for `tessera ingest … --recipient`)",
        pub_path.display()
    )
    .map_err(Error::from)?;
    writeln!(w, "  recipient {public}").map_err(Error::from)?;
    Ok(())
}

// ---- trust store ----

/// Search order for trusted keys: a repo-local `.tessera/trust/`, then the user store
/// (`$XDG_CONFIG_HOME/tessera/trust` or `~/.config/tessera/trust`).
pub fn trust_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![PathBuf::from(".tessera/trust")];
    if let Some(d) = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .map(|base| base.join("tessera/trust"))
    {
        dirs.push(d);
    }
    dirs
}

/// One trusted key in the store.
pub struct TrustEntry {
    /// The handle the operator gave it (`<name>.pub`'s stem).
    pub name: String,
    /// The trusted public key (hex == the sidecar `key_id`).
    pub key_id: String,
    /// The attribution identity recorded alongside it (`<name>.signer`), if any.
    pub signer: Option<String>,
    /// The dir it was found in (repo or user).
    pub dir: PathBuf,
}

/// Add a public key to `dir` as `<name>.pub` (its canonical `key_id` hex), plus `<name>.signer` if a
/// signer identity is given. Validates the key before storing it.
pub fn add(
    dir: &Path,
    pubkey_file: &Path,
    name: &str,
    signer: Option<&str>,
    w: &mut dyn Write,
) -> Result<()> {
    let pub_hex = std::fs::read_to_string(pubkey_file).map_err(Error::from)?;
    let vk = signing::verifying_key_from_hex(pub_hex.trim())?; // validates the point
    let key_id = signing::verifying_key_hex(&vk);
    std::fs::create_dir_all(dir).map_err(Error::from)?;
    std::fs::write(dir.join(format!("{name}.pub")), &key_id).map_err(Error::from)?;
    if let Some(s) = signer {
        std::fs::write(dir.join(format!("{name}.signer")), s).map_err(Error::from)?;
    }
    writeln!(
        w,
        "trusted '{name}'  key_id {key_id}{}",
        signer.map(|s| format!("  signer {s}")).unwrap_or_default()
    )
    .map_err(Error::from)?;
    Ok(())
}

/// All trusted entries across `dirs` (repo first, then user).
pub fn entries(dirs: &[PathBuf]) -> Result<Vec<TrustEntry>> {
    let mut out = Vec::new();
    for dir in dirs {
        let rd = match std::fs::read_dir(dir) {
            Ok(rd) => rd,
            Err(_) => continue, // absent store dir is fine
        };
        for e in rd {
            let p = e.map_err(Error::from)?.path();
            if p.extension().and_then(|x| x.to_str()) == Some("pub") {
                let name = p
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default()
                    .to_string();
                let key_id = std::fs::read_to_string(&p)
                    .map_err(Error::from)?
                    .trim()
                    .to_string();
                let signer = std::fs::read_to_string(dir.join(format!("{name}.signer")))
                    .ok()
                    .map(|s| s.trim().to_string());
                out.push(TrustEntry {
                    name,
                    key_id,
                    signer,
                    dir: dir.clone(),
                });
            }
        }
    }
    Ok(out)
}

/// `tessera trust list` — every trusted key.
pub fn list(dirs: &[PathBuf], w: &mut dyn Write) -> Result<()> {
    let es = entries(dirs)?;
    if es.is_empty() {
        writeln!(
            w,
            "no trusted keys (add one with `tessera trust add <pubkey> --name <handle>`)"
        )
        .map_err(Error::from)?;
        return Ok(());
    }
    for e in &es {
        writeln!(
            w,
            "{:<16} {}{}",
            e.name,
            e.key_id,
            e.signer
                .as_deref()
                .map(|s| format!("  {s}"))
                .unwrap_or_default()
        )
        .map_err(Error::from)?;
    }
    Ok(())
}

/// `tessera trust remove` — by handle or by `key_id`.
pub fn remove(dirs: &[PathBuf], target: &str, w: &mut dyn Write) -> Result<()> {
    let es = entries(dirs)?;
    let hit = es
        .iter()
        .find(|e| e.name == target || e.key_id == target)
        .ok_or_else(|| Error::Invalid(format!("no trusted key '{target}'")))?;
    std::fs::remove_file(hit.dir.join(format!("{}.pub", hit.name))).map_err(Error::from)?;
    let _ = std::fs::remove_file(hit.dir.join(format!("{}.signer", hit.name)));
    writeln!(w, "removed '{}' ({})", hit.name, hit.key_id).map_err(Error::from)?;
    Ok(())
}

/// Look up a `key_id` across the trust dirs.
pub fn lookup(dirs: &[PathBuf], key_id: &str) -> Result<Option<TrustEntry>> {
    Ok(entries(dirs)?.into_iter().find(|e| e.key_id == key_id))
}

// ---- verify-sig (trust-store-defaulting) ----

/// `tessera verify-sig` — verify a `.tsra` against its sidecar. With `--pubkey`, verify against that
/// explicit key (the old behaviour). Otherwise resolve the sidecar's `key_id` in the TRUST STORE,
/// answering "intact **and** signed by a key you trust." `require_signer` additionally asserts the
/// envelope's attribution identity.
pub fn verify_sig(
    file: &Path,
    pubkey: Option<&Path>,
    require_signer: Option<&str>,
    dirs: &[PathBuf],
    w: &mut dyn Write,
) -> Result<()> {
    let env = read_signature(file)?;
    let (vk, label) = match pubkey {
        Some(p) => {
            let hex = std::fs::read_to_string(p).map_err(Error::from)?;
            (
                signing::verifying_key_from_hex(hex.trim())?,
                "explicit --pubkey".to_string(),
            )
        }
        None => {
            let entry = lookup(dirs, &env.key_id)?.ok_or_else(|| {
                Error::Invalid(format!(
                    "untrusted: key_id {} is not in the trust store — \
                     `tessera trust add <pubkey> --name <handle>` if you recognize this signer{}",
                    env.key_id,
                    env.signer
                        .as_deref()
                        .map(|s| format!(" (claims signer {s})"))
                        .unwrap_or_default()
                ))
            })?;
            (
                signing::verifying_key_from_hex(&entry.key_id)?,
                format!("trusted as '{}'", entry.name),
            )
        }
    };

    // 1. the signature attests the manifest (and thus every block digest + metadata).
    if !tessera_io::verify_tsra(file, &vk)? {
        return Err(Error::Invalid(format!(
            "signature INVALID for {}",
            file.display()
        )));
    }
    // 2. re-read every block vs its recorded digest (verify-sig = authentic AND intact).
    let mut r = tessera_io::Reader::open(file)?;
    let n = r.manifest().blocks.len();
    for name in r.block_names() {
        r.read_block(&name)?;
    }
    // 3. optional signer policy.
    if let Some(req) = require_signer {
        if env.signer.as_deref() != Some(req) {
            return Err(Error::Invalid(format!(
                "signer policy: required '{req}', sidecar attributes to {:?}",
                env.signer
            )));
        }
    }
    writeln!(w, "OK  {} — {label} + {n} blocks intact", file.display()).map_err(Error::from)?;
    if let Some(s) = &env.signer {
        writeln!(w, "  signer {s} (attribution; not a trust anchor)").map_err(Error::from)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tessera_core::block::array::{ArrayBlock, ArraySpec};
    use tessera_core::ProductBuilder;
    use tessera_io::{pack, BlockPayload};

    fn signed_tsra(dir: &Path, name: &str) -> (PathBuf, String) {
        // build + pack a tiny product, then keygen a key and sign it via the sign module
        let vol = ArrayBlock::new("volume", ArraySpec::new(vec![4, 4, 4], "int16"));
        let payload = serde_json::to_vec(&vol.spec).unwrap();
        let mut b = ProductBuilder::new("recon", name, "d", "2024-01-01T00:00:00Z");
        b.add_block(&vol).unwrap();
        let sealed = b.seal().unwrap();
        let tsra = dir.join(format!("{name}.tsra"));
        pack(&sealed, &[BlockPayload::new("volume", payload)], &tsra).unwrap();

        let keyfile = dir.join(format!("{name}.key"));
        keygen(&keyfile, &mut Vec::new()).unwrap();
        let seed_hex = std::fs::read_to_string(&keyfile).unwrap();
        let sk = signing::signing_key_from_hex(&seed_hex).unwrap();
        tessera_io::sign_tsra(
            &tsra,
            &sk,
            &signing::SignOpts {
                signer: Some("https://orcid.org/0000-0002-1825-0097".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let pub_hex = std::fs::read_to_string(with_suffix(&keyfile, ".pub")).unwrap();
        (tsra, pub_hex)
    }

    /// A throwaway, unencrypted OpenSSH ed25519 private key (generated offline with `ssh-keygen -t
    /// ed25519 -N ''`). Its raw 32-byte public key is `438ddd…2acf6faf` — the `key_id` the loader
    /// must derive, identical to a raw-hex key with the same pubkey (only the container differs).
    const SSH_ED25519_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\n\
        b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW\n\
        QyNTUxOQAAACBDjd0RWR+en8LrDAfDQLQB1mZIE5JKxQzen9pLKs9vrwAAAJDCcCx4wnAs\n\
        eAAAAAtzc2gtZWQyNTUxOQAAACBDjd0RWR+en8LrDAfDQLQB1mZIE5JKxQzen9pLKs9vrw\n\
        AAAEBe9p6oxAJOzfIpyQL96Ns+SnJxFNKYx5F+xRkgw9lFXUON3RFZH56fwusMB8NAtAHW\n\
        ZkgTkkrFDN6f2ksqz2+vAAAADHRlc3NlcmEtdGVzdAE=\n\
        -----END OPENSSH PRIVATE KEY-----\n";

    #[test]
    fn loads_an_openssh_ed25519_key_and_signs_with_the_same_key_id() {
        let dir = tempfile::tempdir().unwrap();
        // an OpenSSH `id_ed25519` loads with key_format = "ssh-ed25519".
        let sshf = dir.path().join("id_ed25519");
        std::fs::write(&sshf, SSH_ED25519_KEY).unwrap();
        let (sk, fmt) = load_signing_key(&sshf).unwrap();
        assert_eq!(fmt, "ssh-ed25519");
        // the key_id is the raw ed25519 pubkey hex — container-independent, so any verifier checks it
        // exactly like a keygen key (the crypto is unchanged; only the on-disk key wrapper differs).
        assert_eq!(
            signing::verifying_key_hex(&sk.verifying_key()),
            "438ddd11591f9e9fc2eb0c07c340b401d6664813924ac50cde9fda4b2acf6faf"
        );
        // a signature made with the ssh-loaded key verifies under its public key.
        let env = signing::sign_ed25519("blake3:abcd", &sk, &signing::SignOpts::default()).unwrap();
        assert!(signing::verify("blake3:abcd", &env, &sk.verifying_key()));

        // a raw-hex seed (what keygen writes) loads with key_format = "raw-hex".
        let rawf = dir.path().join("raw.key");
        keygen(&rawf, &mut Vec::new()).unwrap();
        assert_eq!(load_signing_key(&rawf).unwrap().1, "raw-hex");

        // a non-ed25519 / malformed key is rejected, not panicked.
        let badf = dir.path().join("bad");
        std::fs::write(&badf, "-----BEGIN OPENSSH PRIVATE KEY-----\nnonsense\n").unwrap();
        assert!(load_signing_key(&badf).is_err());
    }

    #[test]
    fn keygen_writes_a_usable_keypair() {
        let dir = tempfile::tempdir().unwrap();
        let kf = dir.path().join("id");
        keygen(&kf, &mut Vec::new()).unwrap();
        let seed = std::fs::read_to_string(&kf).unwrap();
        let pub_hex = std::fs::read_to_string(dir.path().join("id.pub")).unwrap();
        // the seed loads + its public key matches the written .pub
        let sk = signing::signing_key_from_hex(&seed).unwrap();
        assert_eq!(
            signing::verifying_key_hex(&sk.verifying_key()),
            pub_hex.trim()
        );
    }

    #[test]
    fn verify_sig_via_trust_store_accepts_trusted_rejects_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("trust");
        let dirs = vec![store.clone()];
        let (tsra, pub_hex) = signed_tsra(dir.path(), "study");

        // before trusting the key: untrusted.
        let err = verify_sig(&tsra, None, None, &dirs, &mut Vec::new()).unwrap_err();
        assert!(format!("{err}").contains("untrusted"), "{err}");

        // trust it, then verify-sig passes against the store.
        let pf = dir.path().join("study.pub");
        std::fs::write(&pf, &pub_hex).unwrap();
        add(
            &store,
            &pf,
            "instrument-lab",
            Some("https://orcid.org/0000-0002-1825-0097"),
            &mut Vec::new(),
        )
        .unwrap();
        let mut out = Vec::new();
        verify_sig(&tsra, None, None, &dirs, &mut out).unwrap();
        assert!(String::from_utf8(out)
            .unwrap()
            .contains("trusted as 'instrument-lab'"));

        // a require-signer mismatch is rejected.
        let err = verify_sig(
            &tsra,
            None,
            Some("https://orcid.org/9999"),
            &dirs,
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(format!("{err}").contains("signer policy"), "{err}");
    }

    #[test]
    fn add_list_remove_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("trust");
        let dirs = vec![store.clone()];
        let kf = dir.path().join("k");
        keygen(&kf, &mut Vec::new()).unwrap();
        add(
            &store,
            &with_suffix(&kf, ".pub"),
            "alice",
            None,
            &mut Vec::new(),
        )
        .unwrap();

        let mut listed = Vec::new();
        list(&dirs, &mut listed).unwrap();
        assert!(String::from_utf8(listed).unwrap().contains("alice"));

        remove(&dirs, "alice", &mut Vec::new()).unwrap();
        assert!(entries(&dirs).unwrap().is_empty());
        assert!(remove(&dirs, "alice", &mut Vec::new()).is_err());
    }
}
