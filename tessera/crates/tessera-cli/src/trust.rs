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

/// The repo-local store `tessera trust add/list/remove --repo` manages, relative to the current
/// directory. It is **never** consulted implicitly (#576): whoever can write to the directory a
/// verifier runs in would otherwise widen what that verifier trusts. Pass it to `verify-sig` with
/// `--trust-dir .tessera/trust` (or name its absolute path in [`ENV_TRUST_DIR`]) for it to take effect.
pub const REPO_TRUST_DIR: &str = ".tessera/trust";

/// Env var naming extra trust stores, a PATH-style list (`:`-separated on Unix, `;` on Windows).
/// Used only when no `--trust-dir` flag is given; empty entries are ignored (they never mean the
/// current directory, unlike an empty `PATH` entry). Every entry must be **absolute**: the variable is
/// set once (a shell profile, a CI job) and then applies to every later run, so a relative entry would
/// re-resolve against whatever directory a verifier happens to run in — inside an unpacked archive,
/// `.tessera/trust` is that archive's planted store (#576). A relative entry is an error, never a
/// silent skip. `--trust-dir`, typed for each run, may be relative.
pub const ENV_TRUST_DIR: &str = "TESSERA_TRUST_DIR";

/// Where a trust store came from — reported with every accepting key so a verifier can tell which
/// decision admitted it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreOrigin {
    /// Named on the command line with `--trust-dir`.
    TrustDirFlag,
    /// Named in [`ENV_TRUST_DIR`].
    TrustDirEnv,
    /// The user store (`$XDG_CONFIG_HOME/tessera/trust` or `~/.config/tessera/trust`).
    User,
    /// The repo store [`REPO_TRUST_DIR`], selected with `trust … --repo` (management only).
    Repo,
}

impl StoreOrigin {
    fn label(self) -> &'static str {
        match self {
            StoreOrigin::TrustDirFlag => "--trust-dir",
            StoreOrigin::TrustDirEnv => ENV_TRUST_DIR,
            StoreOrigin::User => "user store",
            StoreOrigin::Repo => "repo store",
        }
    }
}

/// One trust store: a directory of `<name>.pub` (+ optional `<name>.signer`) files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrustStore {
    /// The directory.
    pub path: PathBuf,
    /// How it was selected.
    pub origin: StoreOrigin,
}

impl TrustStore {
    /// The repo store [`REPO_TRUST_DIR`].
    pub fn repo() -> Self {
        TrustStore {
            path: PathBuf::from(REPO_TRUST_DIR),
            origin: StoreOrigin::Repo,
        }
    }

    /// The user store from the process environment; see [`TrustStore::user_from`].
    pub fn user() -> Option<Self> {
        Self::user_from(
            std::env::var_os("XDG_CONFIG_HOME").as_deref(),
            std::env::var_os("HOME").as_deref(),
        )
    }

    /// The user store: `$XDG_CONFIG_HOME/tessera/trust`, else `$HOME/.config/tessera/trust`.
    ///
    /// Only an **absolute** base counts. An empty or relative `XDG_CONFIG_HOME` is ignored, as the
    /// XDG base-directory spec requires, and an empty or relative `HOME` yields no user store at all:
    /// either would otherwise resolve the "user store" under the current directory and reopen the
    /// planted-key hole of #576 under another name.
    pub fn user_from(
        xdg_config_home: Option<&std::ffi::OsStr>,
        home: Option<&std::ffi::OsStr>,
    ) -> Option<Self> {
        let absolute = |v: &std::ffi::OsStr| Some(PathBuf::from(v)).filter(|p| p.is_absolute());
        xdg_config_home
            .and_then(absolute)
            .or_else(|| home.and_then(absolute).map(|h| h.join(".config")))
            .map(|base| TrustStore {
                path: base.join("tessera/trust"),
                origin: StoreOrigin::User,
            })
    }

    /// `<path> (<origin>)`, the form every report uses.
    pub fn describe(&self) -> String {
        format!("{} ({})", self.path.display(), self.origin.label())
    }
}

/// The trust stores `verify-sig` consults, in lookup order (#576, ADR-0037 amendment 2026-10-25):
///
/// 1. every `--trust-dir` (`flag_dirs`, in the order given, relative ones against the cwd) — or,
///    when there is none, every entry of [`ENV_TRUST_DIR`] (`env`), each of which must be absolute;
///    the flag replaces the env var, it does not add to it;
/// 2. the user store (`user`), if `$XDG_CONFIG_HOME`/`$HOME` resolves one.
///
/// Nothing else, and nothing relative to the current directory unless it was named. An explicitly
/// named store must be an existing directory — a typo is an error, not a silently empty store — and
/// is canonicalized, so the store reported for an acceptance is the absolute directory that was
/// read, not the spelling it was named by. The user store may be absent (= no keys). Pure over its
/// inputs apart from that filesystem check; [`trust_stores`] supplies the environment.
pub fn resolve_trust_stores(
    flag_dirs: &[PathBuf],
    env: Option<&std::ffi::OsStr>,
    user: Option<TrustStore>,
) -> Result<Vec<TrustStore>> {
    let explicit: Vec<TrustStore> = if flag_dirs.is_empty() {
        let mut from_env = Vec::new();
        for path in env.map(std::env::split_paths).into_iter().flatten() {
            if path.as_os_str().is_empty() {
                continue;
            }
            if !path.is_absolute() {
                return Err(Error::Invalid(format!(
                    "{ENV_TRUST_DIR} entry {} is relative — give an absolute path (it would \
                     otherwise resolve against whatever directory verify-sig runs in); for a \
                     one-off relative store use --trust-dir",
                    path.display()
                )));
            }
            from_env.push(TrustStore {
                path,
                origin: StoreOrigin::TrustDirEnv,
            });
        }
        from_env
    } else {
        flag_dirs
            .iter()
            .map(|path| TrustStore {
                path: path.clone(),
                origin: StoreOrigin::TrustDirFlag,
            })
            .collect()
    };
    let mut stores = Vec::with_capacity(explicit.len() + 1);
    for s in explicit {
        if !s.path.is_dir() {
            return Err(Error::Invalid(format!(
                "trust store {} is not a directory",
                s.describe()
            )));
        }
        stores.push(TrustStore {
            path: s.path.canonicalize().map_err(Error::from)?,
            origin: s.origin,
        });
    }
    stores.extend(user);
    Ok(stores)
}

/// [`resolve_trust_stores`] over the process environment.
pub fn trust_stores(flag_dirs: &[PathBuf]) -> Result<Vec<TrustStore>> {
    resolve_trust_stores(
        flag_dirs,
        std::env::var_os(ENV_TRUST_DIR).as_deref(),
        TrustStore::user(),
    )
}

/// The advisory every `trust … --repo` verb prints: managing the repo store does not make
/// `verify-sig` consult it.
pub fn repo_store_note(w: &mut dyn Write) -> Result<()> {
    writeln!(
        w,
        "note: {REPO_TRUST_DIR} is not consulted implicitly — pass `verify-sig --trust-dir \
         {REPO_TRUST_DIR}`, or set {ENV_TRUST_DIR}=$PWD/{REPO_TRUST_DIR} (absolute) if you mean \
         it, for it to take effect"
    )
    .map_err(Error::from)
}

/// One trusted key in the store.
pub struct TrustEntry {
    /// The handle the operator gave it (`<name>.pub`'s stem).
    pub name: String,
    /// The trusted public key (hex == the sidecar `key_id`).
    pub key_id: String,
    /// The attribution identity recorded alongside it (`<name>.signer`), if any.
    pub signer: Option<String>,
    /// The store it was found in.
    pub store: TrustStore,
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

/// The entries of one store, sorted by handle. An absent directory has none.
fn store_entries(store: &TrustStore) -> Result<Vec<TrustEntry>> {
    let rd = match std::fs::read_dir(&store.path) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(Error::from(e)),
    };
    let mut out = Vec::new();
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
            let signer = std::fs::read_to_string(store.path.join(format!("{name}.signer")))
                .ok()
                .map(|s| s.trim().to_string());
            out.push(TrustEntry {
                name,
                key_id,
                signer,
                store: store.clone(),
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// All trusted entries across `stores`, in store order.
pub fn entries(stores: &[TrustStore]) -> Result<Vec<TrustEntry>> {
    let mut out = Vec::new();
    for s in stores {
        out.extend(store_entries(s)?);
    }
    Ok(out)
}

/// `tessera trust list` — each store (as `<path> (<origin>)`) and the keys it holds.
pub fn list(stores: &[TrustStore], w: &mut dyn Write) -> Result<()> {
    if stores.is_empty() {
        writeln!(w, "no trust store ($HOME/$XDG_CONFIG_HOME unset)").map_err(Error::from)?;
    }
    for s in stores {
        writeln!(w, "{}", s.describe()).map_err(Error::from)?;
        let es = store_entries(s)?;
        if es.is_empty() {
            writeln!(
                w,
                "  (no keys — add one with `tessera trust add <pubkey> --name <handle>`)"
            )
            .map_err(Error::from)?;
        }
        for e in &es {
            writeln!(
                w,
                "  {:<16} {}{}",
                e.name,
                e.key_id,
                e.signer
                    .as_deref()
                    .map(|s| format!("  {s}"))
                    .unwrap_or_default()
            )
            .map_err(Error::from)?;
        }
    }
    Ok(())
}

/// `tessera trust remove` — by handle or by `key_id`, from `store`.
pub fn remove(store: &TrustStore, target: &str, w: &mut dyn Write) -> Result<()> {
    let es = store_entries(store)?;
    let hit = es
        .iter()
        .find(|e| e.name == target || e.key_id == target)
        .ok_or_else(|| {
            Error::Invalid(format!("no trusted key '{target}' in {}", store.describe()))
        })?;
    std::fs::remove_file(store.path.join(format!("{}.pub", hit.name))).map_err(Error::from)?;
    let _ = std::fs::remove_file(store.path.join(format!("{}.signer", hit.name)));
    writeln!(w, "removed '{}' ({})", hit.name, hit.key_id).map_err(Error::from)?;
    Ok(())
}

/// Look up a `key_id` across `stores`; the first store holding it wins.
pub fn lookup(stores: &[TrustStore], key_id: &str) -> Result<Option<TrustEntry>> {
    Ok(entries(stores)?.into_iter().find(|e| e.key_id == key_id))
}

// ---- verify-sig (trust-store-defaulting) ----

/// `tessera verify-sig` — verify a `.tsra` against its signature. With `--pubkey`, verify against
/// that explicit key. Otherwise resolve the signature's `key_id` in `stores` (see
/// [`resolve_trust_stores`]), answering "intact **and** signed by a key you trust." `require_signer`
/// additionally asserts the envelope's attribution identity.
///
/// The verdict goes to `w`; on success, `log` (stderr in `main`) names the accepting key and where
/// it came from — the store path and origin, or `--pubkey <file>` — so a verifier can audit which
/// trust decision admitted the file.
pub fn verify_sig(
    file: &Path,
    pubkey: Option<&Path>,
    require_signer: Option<&str>,
    stores: &[TrustStore],
    w: &mut dyn Write,
    log: &mut dyn Write,
) -> Result<()> {
    let env = read_signature(file)?;
    let (vk, label, source) = match pubkey {
        Some(p) => {
            let hex = std::fs::read_to_string(p).map_err(Error::from)?;
            (
                signing::verifying_key_from_hex(hex.trim())?,
                "explicit --pubkey".to_string(),
                format!("--pubkey {}", p.display()),
            )
        }
        None => {
            let entry = lookup(stores, &env.key_id)?.ok_or_else(|| {
                Error::Invalid(format!(
                    "untrusted: key_id {} is not in any consulted trust store [{}]{} — \
                     `tessera trust add <pubkey> --name <handle>` if you recognize this signer; \
                     a repo store counts only when passed with --trust-dir",
                    env.key_id,
                    stores
                        .iter()
                        .map(TrustStore::describe)
                        .collect::<Vec<_>>()
                        .join(", "),
                    env.signer
                        .as_deref()
                        .map(|s| format!(" (claims signer {s})"))
                        .unwrap_or_default()
                ))
            })?;
            (
                signing::verifying_key_from_hex(&entry.key_id)?,
                format!("trusted as '{}'", entry.name),
                format!("trust store {}", entry.store.describe()),
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
    writeln!(
        log,
        "accepted key_id {} via {source}",
        signing::verifying_key_hex(&vk)
    )
    .map_err(Error::from)?;
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

    /// Build a throwaway, unencrypted OpenSSH ed25519 private key around `seed` — the same 32 bytes
    /// the raw-hex container carries, so the two on-disk formats provably hold the *same* key.
    /// Generated per test run and never committed: a parseable private key checked into a public
    /// repo is key material, and the loader is what is under test — not which key it happens to hold.
    fn openssh_ed25519_pem(seed: &[u8; 32]) -> String {
        use ssh_key::private::{Ed25519Keypair, Ed25519PrivateKey};
        let mut key =
            ssh_key::PrivateKey::from(Ed25519Keypair::from(Ed25519PrivateKey::from_bytes(seed)));
        key.set_comment("tessera-test");
        key.to_openssh(ssh_key::LineEnding::LF).unwrap().to_string()
    }

    #[test]
    fn loads_an_openssh_ed25519_key_and_signs_with_the_same_key_id() {
        let dir = tempfile::tempdir().unwrap();
        let mut seed = [0u8; 32];
        getrandom::getrandom(&mut seed).unwrap();

        // an OpenSSH `id_ed25519` loads with key_format = "ssh-ed25519".
        let sshf = dir.path().join("id_ed25519");
        std::fs::write(&sshf, openssh_ed25519_pem(&seed)).unwrap();
        let (sk, fmt) = load_signing_key(&sshf).unwrap();
        assert_eq!(fmt, "ssh-ed25519");
        let ssh_key_id = signing::verifying_key_hex(&sk.verifying_key());
        // a signature made with the ssh-loaded key verifies under its public key.
        let env = signing::sign_ed25519("blake3:abcd", &sk, &signing::SignOpts::default()).unwrap();
        assert!(signing::verify("blake3:abcd", &env, &sk.verifying_key()));

        // the SAME seed as a raw hex seed (what keygen writes) loads with key_format = "raw-hex" and
        // yields the identical key_id — the container-independence the `ssh-ed25519` path promises:
        // the key_id is the raw ed25519 pubkey hex either way, so any verifier checks it unchanged.
        let samef = dir.path().join("same.key");
        std::fs::write(
            &samef,
            signing::signing_key_hex(&SigningKey::from_bytes(&seed)),
        )
        .unwrap();
        let (raw_sk, raw_fmt) = load_signing_key(&samef).unwrap();
        assert_eq!(raw_fmt, "raw-hex");
        assert_eq!(
            ssh_key_id,
            signing::verifying_key_hex(&raw_sk.verifying_key())
        );

        // a keygen-written key (its own fresh seed) also loads with key_format = "raw-hex".
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
        let dirs = vec![TrustStore {
            path: store.clone(),
            origin: StoreOrigin::TrustDirFlag,
        }];
        let (tsra, pub_hex) = signed_tsra(dir.path(), "study");

        // before trusting the key: untrusted, and the error names the consulted store.
        let err =
            verify_sig(&tsra, None, None, &dirs, &mut Vec::new(), &mut Vec::new()).unwrap_err();
        assert!(format!("{err}").contains("untrusted"), "{err}");
        assert!(
            format!("{err}").contains(&format!("{} (--trust-dir)", store.display())),
            "{err}"
        );

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
        let (mut out, mut log) = (Vec::new(), Vec::new());
        verify_sig(&tsra, None, None, &dirs, &mut out, &mut log).unwrap();
        assert!(String::from_utf8(out)
            .unwrap()
            .contains("trusted as 'instrument-lab'"));
        // the accepting key + its store are reported on the log stream.
        assert_eq!(
            String::from_utf8(log).unwrap(),
            format!(
                "accepted key_id {} via trust store {} (--trust-dir)\n",
                pub_hex.trim(),
                store.display()
            )
        );

        // an explicit --pubkey is reported as such.
        let mut log = Vec::new();
        verify_sig(&tsra, Some(&pf), None, &[], &mut Vec::new(), &mut log).unwrap();
        assert_eq!(
            String::from_utf8(log).unwrap(),
            format!(
                "accepted key_id {} via --pubkey {}\n",
                pub_hex.trim(),
                pf.display()
            )
        );

        // a require-signer mismatch is rejected.
        let err = verify_sig(
            &tsra,
            None,
            Some("https://orcid.org/9999"),
            &dirs,
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(format!("{err}").contains("signer policy"), "{err}");
    }

    #[test]
    fn add_list_remove_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("trust");
        let ts = TrustStore {
            path: store.clone(),
            origin: StoreOrigin::User,
        };
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
        list(std::slice::from_ref(&ts), &mut listed).unwrap();
        let listed = String::from_utf8(listed).unwrap();
        assert!(
            listed.starts_with(&format!("{} (user store)\n", store.display())),
            "{listed}"
        );
        assert!(listed.contains("alice"), "{listed}");

        remove(&ts, "alice", &mut Vec::new()).unwrap();
        assert!(entries(std::slice::from_ref(&ts)).unwrap().is_empty());
        assert!(remove(&ts, "alice", &mut Vec::new()).is_err());
    }

    fn user_at(p: &Path) -> TrustStore {
        TrustStore {
            path: p.to_path_buf(),
            origin: StoreOrigin::User,
        }
    }

    #[test]
    fn default_trust_set_is_the_user_store_only() {
        // no flag, no env: exactly the user store — never `.tessera/trust` (#576).
        let user = user_at(Path::new("/nonexistent/user/trust"));
        let stores = resolve_trust_stores(&[], None, Some(user.clone())).unwrap();
        assert_eq!(stores, vec![user]);
        // with no resolvable user store, the set is empty (verification then fails closed).
        assert!(resolve_trust_stores(&[], None, None).unwrap().is_empty());
    }

    #[test]
    fn explicit_stores_come_first_and_the_flag_replaces_the_env_var() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b, c) = (
            dir.path().join("a"),
            dir.path().join("b"),
            dir.path().join("c"),
        );
        for d in [&a, &b, &c] {
            std::fs::create_dir(d).unwrap();
        }
        let user = user_at(&dir.path().join("user"));
        let env = std::env::join_paths([&b, &c]).unwrap();
        let (a, b, c) = (
            a.canonicalize().unwrap(),
            b.canonicalize().unwrap(),
            c.canonicalize().unwrap(),
        );

        // env only: its entries, in order, then the user store.
        let stores = resolve_trust_stores(&[], Some(&env), Some(user.clone())).unwrap();
        let got: Vec<_> = stores.iter().map(|s| (s.path.clone(), s.origin)).collect();
        assert_eq!(
            got,
            vec![
                (b.clone(), StoreOrigin::TrustDirEnv),
                (c.clone(), StoreOrigin::TrustDirEnv),
                (user.path.clone(), StoreOrigin::User),
            ]
        );

        // a flag replaces the env var entirely.
        let stores =
            resolve_trust_stores(std::slice::from_ref(&a), Some(&env), Some(user.clone())).unwrap();
        let got: Vec<_> = stores.iter().map(|s| (s.path.clone(), s.origin)).collect();
        assert_eq!(
            got,
            vec![
                (a, StoreOrigin::TrustDirFlag),
                (user.path, StoreOrigin::User),
            ]
        );
    }

    #[test]
    fn the_user_store_never_resolves_under_the_current_directory() {
        use std::ffi::OsStr;
        let abs = if cfg!(windows) { r"C:\cfg" } else { "/cfg" };
        let home = if cfg!(windows) { r"C:\home" } else { "/home/u" };
        let at = |x: Option<&str>, h: Option<&str>| {
            TrustStore::user_from(x.map(OsStr::new), h.map(OsStr::new)).map(|s| s.path)
        };
        assert_eq!(
            at(Some(abs), Some(home)),
            Some(Path::new(abs).join("tessera/trust"))
        );
        // empty or relative XDG_CONFIG_HOME is ignored (XDG spec) → $HOME/.config.
        for xdg in ["", "rel", "./x"] {
            assert_eq!(
                at(Some(xdg), Some(home)),
                Some(Path::new(home).join(".config/tessera/trust")),
                "{xdg:?}"
            );
        }
        // an empty or relative HOME gives no user store at all, rather than one under the cwd.
        for h in ["", "rel", "."] {
            assert_eq!(at(Some(""), Some(h)), None, "{h:?}");
            assert_eq!(at(None, Some(h)), None, "{h:?}");
        }
        assert_eq!(at(None, None), None);
    }

    #[test]
    fn empty_env_entries_never_mean_the_current_directory() {
        // `PATH` treats an empty entry as the cwd; a trust list must not, or `TESSERA_TRUST_DIR=":"`
        // would reopen the planted-key hole.
        let sep = if cfg!(windows) { ";" } else { ":" };
        for v in [String::new(), sep.to_string(), sep.repeat(2)] {
            let stores = resolve_trust_stores(&[], Some(std::ffi::OsStr::new(&v)), None).unwrap();
            assert!(stores.is_empty(), "{v:?} → {stores:?}");
        }
    }

    #[test]
    fn a_relative_env_entry_is_an_error_but_a_relative_flag_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let abs = dir.path().canonicalize().unwrap();
        for v in [
            std::ffi::OsString::from("rel"),
            std::env::join_paths([abs.as_path(), Path::new("rel")]).unwrap(),
        ] {
            let err = resolve_trust_stores(&[], Some(&v), None).unwrap_err();
            assert!(
                format!("{err}").contains("TESSERA_TRUST_DIR entry rel is relative"),
                "{err}"
            );
        }
        // the same relative spelling on the command line is resolved against the cwd.
        let stores = resolve_trust_stores(&[PathBuf::from(".")], None, None).unwrap();
        assert_eq!(
            stores[0].path,
            std::env::current_dir().unwrap().canonicalize().unwrap()
        );
        assert!(stores[0].path.is_absolute());
    }

    #[test]
    fn a_named_store_that_is_not_a_directory_is_an_error() {
        let missing = PathBuf::from("/nonexistent/tessera/trust");
        let err = resolve_trust_stores(std::slice::from_ref(&missing), None, None).unwrap_err();
        assert!(
            format!("{err}")
                .contains("/nonexistent/tessera/trust (--trust-dir) is not a directory"),
            "{err}"
        );
        let err = resolve_trust_stores(&[], Some(missing.as_os_str()), None).unwrap_err();
        assert!(format!("{err}").contains("(TESSERA_TRUST_DIR)"), "{err}");
    }
}
