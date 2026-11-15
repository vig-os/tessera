//! Trust-store **resolution** for `verify-sig` (#576, ADR-0037 amendment of 2026-10-25), driven through
//! the real binary (`CARGO_BIN_EXE_tessera`, so it runs in the hermetic flake check with no cargo).
//!
//! The default trust set is the user store only. A repo store such as `./.tessera/trust/` counts only
//! when it is named: `--trust-dir <path>` (repeatable) or `TESSERA_TRUST_DIR`. Every success names the
//! accepting key and where it came from on stderr. Each test runs in its own tempdir with an isolated
//! `HOME` / `XDG_CONFIG_HOME` and no inherited `TESSERA_TRUST_DIR`, so the host's real stores never
//! leak in.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tessera_core::block::array::ArraySpec;
use tessera_core::ProductBuilder;
use tessera_io::array::{self, ArrayData};
use tessera_io::pack;

/// A sandbox: a tempdir that is the cwd, `HOME` and (via `.config`) the user config root.
struct Sandbox {
    dir: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    /// `tessera` with the cwd, `HOME` and `XDG_CONFIG_HOME` pinned into the sandbox.
    fn cmd(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_tessera"));
        c.current_dir(self.path())
            .env("HOME", self.path())
            .env("XDG_CONFIG_HOME", self.path().join(".config"))
            .env_remove("TESSERA_TRUST_DIR");
        c
    }

    fn run(&self, args: &[&str]) -> Output {
        self.cmd().args(args).output().unwrap()
    }

    fn ok(&self, args: &[&str]) -> Output {
        let o = self.run(args);
        assert!(
            o.status.success(),
            "expected success: tessera {args:?}\nstderr: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        o
    }

    /// The repo store as `verify-sig` reports it once named: canonicalized.
    fn repo_store(&self) -> PathBuf {
        self.path().canonicalize().unwrap().join(".tessera/trust")
    }

    /// The user store this sandbox's `XDG_CONFIG_HOME` resolves to.
    fn user_store(&self) -> PathBuf {
        self.path().join(".config/tessera/trust")
    }

    /// A sealed `forged.tsra` signed by a freshly generated key `keys/other`, claiming the
    /// `anatomist-operator` identity: the issue's attacker setup. Returns the `.pub` path.
    fn forged(&self) -> PathBuf {
        let mut spec = ArraySpec::new(vec![8, 8, 8], "int16");
        spec.codec = "pcodec".into();
        let data = ArrayData::I16((0..512).map(|k| k as i16).collect());
        let (bref, payload) = array::array_block("volume", &spec, &data).unwrap();
        let mut b = ProductBuilder::new("recon", "DP", "forged", "2024-01-01T00:00:00Z");
        b.add_block_ref(bref);
        pack(
            &b.seal().unwrap(),
            &[payload],
            &self.path().join("forged.tsra"),
        )
        .unwrap();
        std::fs::create_dir_all(self.path().join("keys")).unwrap();
        self.ok(&["keygen", "keys/other"]);
        self.ok(&[
            "sign",
            "forged.tsra",
            "--key",
            "keys/other",
            "--signer",
            "anatomist-operator",
        ]);
        self.path().join("keys/other.pub")
    }

    /// The key_id (= public key hex) of `keys/other`.
    fn key_id(&self) -> String {
        std::fs::read_to_string(self.path().join("keys/other.pub"))
            .unwrap()
            .trim()
            .to_string()
    }
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// The issue's repro verbatim: a key planted in `./.tessera/trust/` of the directory the verifier runs
/// in must NOT be trusted. Before #576 the second `verify-sig` exited 0 ("accepted through
/// ./.tessera/trust/"), and the claimed `anatomist-operator` identity rode along with it.
#[test]
fn a_key_planted_in_the_cwd_store_is_not_trusted() {
    let s = Sandbox::new();
    s.forged();

    // empty user store: nothing trusted (this was already right).
    let o = s.run(&["verify-sig", "forged.tsra"]);
    assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));

    // the attacker plants their key in the cwd store.
    s.ok(&[
        "trust",
        "add",
        "--repo",
        "--name",
        "planted",
        "keys/other.pub",
    ]);
    assert!(s.path().join(".tessera/trust/planted.pub").exists());

    // the planted key is NOT consulted: still untrusted, exit 1.
    let o = s.run(&["verify-sig", "forged.tsra"]);
    assert_eq!(
        o.status.code(),
        Some(1),
        "a cwd-relative store must never widen trust implicitly\nstderr: {}",
        stderr(&o)
    );
    assert!(stderr(&o).contains("untrusted"), "{}", stderr(&o));
}

/// Plant the issue's key in the repo store and return the stderr of `trust add --repo` (the note).
fn plant(s: &Sandbox) -> String {
    stderr(&s.ok(&[
        "trust",
        "add",
        "--repo",
        "--name",
        "planted",
        "keys/other.pub",
    ]))
}

#[test]
fn trust_repo_verbs_say_the_store_must_be_passed_explicitly() {
    let s = Sandbox::new();
    s.forged();
    let note = "note: .tessera/trust is not consulted implicitly — pass `verify-sig --trust-dir \
                .tessera/trust`, or set TESSERA_TRUST_DIR=$PWD/.tessera/trust (absolute) if you \
                mean it, for it to take effect\n";
    assert_eq!(plant(&s), note);
    let o = s.ok(&["trust", "list", "--repo"]);
    assert_eq!(stderr(&o), note);
    assert!(String::from_utf8_lossy(&o.stdout).starts_with(".tessera/trust (repo store)\n"));
    assert_eq!(
        stderr(&s.ok(&["trust", "remove", "--repo", "planted"])),
        note
    );
    // the user-store verbs print no such note.
    assert_eq!(
        stderr(&s.ok(&["trust", "add", "--name", "lab", "keys/other.pub"])),
        ""
    );
}

#[test]
fn an_explicit_trust_dir_accepts_the_key_and_is_reported_on_stderr() {
    let s = Sandbox::new();
    s.forged();
    plant(&s);
    let o = s.ok(&["verify-sig", "forged.tsra", "--trust-dir", ".tessera/trust"]);
    assert!(String::from_utf8_lossy(&o.stdout)
        .starts_with("OK  forged.tsra — trusted as 'planted' + 1 blocks intact\n"));
    assert_eq!(
        stderr(&o),
        format!(
            "accepted key_id {} via trust store {} (--trust-dir)\n",
            s.key_id(),
            s.repo_store().display()
        )
    );
}

#[test]
fn tessera_trust_dir_is_honoured_and_reported() {
    let s = Sandbox::new();
    s.forged();
    plant(&s);
    // a PATH-style list of ABSOLUTE entries: a missing-key store first, then the planted one.
    let empty = s.path().join("empty");
    std::fs::create_dir(&empty).unwrap();
    let repo = s.path().join(".tessera/trust");
    let o = s
        .cmd()
        .args(["verify-sig", "forged.tsra"])
        .env(
            "TESSERA_TRUST_DIR",
            std::env::join_paths([&empty, &repo]).unwrap(),
        )
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(
        stderr(&o),
        format!(
            "accepted key_id {} via trust store {} (TESSERA_TRUST_DIR)\n",
            s.key_id(),
            s.repo_store().display()
        )
    );

    // a --trust-dir replaces the env var: the planted store is no longer consulted.
    let o = s
        .cmd()
        .args(["verify-sig", "forged.tsra", "--trust-dir", "empty"])
        .env("TESSERA_TRUST_DIR", &repo)
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));
}

/// `TESSERA_TRUST_DIR` is set once (a profile, a CI job) and then applies to every later run, so a
/// RELATIVE entry would re-resolve against whatever directory the verifier happens to run in — inside
/// an unpacked archive, `.tessera/trust` is the archive's planted store (#576 again). A relative entry
/// is a usage error even when a matching key is present; only `--trust-dir`, typed per run, may be
/// relative.
#[test]
fn a_relative_tessera_trust_dir_entry_is_refused() {
    let s = Sandbox::new();
    s.forged();
    plant(&s);
    let abs = s.path().join("empty");
    std::fs::create_dir(&abs).unwrap();
    for value in [
        std::ffi::OsString::from(".tessera/trust"),
        std::env::join_paths([abs.as_path(), std::path::Path::new(".tessera/trust")]).unwrap(),
    ] {
        let o = s
            .cmd()
            .args(["verify-sig", "forged.tsra"])
            .env("TESSERA_TRUST_DIR", &value)
            .output()
            .unwrap();
        assert_eq!(
            o.status.code(),
            Some(1),
            "TESSERA_TRUST_DIR={value:?} must be refused\nstderr: {}",
            stderr(&o)
        );
        assert!(
            stderr(&o).contains(
                "TESSERA_TRUST_DIR entry .tessera/trust is relative — give an absolute path"
            ),
            "{}",
            stderr(&o)
        );
        assert!(!stderr(&o).contains("accepted"), "{}", stderr(&o));
    }
}

#[test]
fn the_user_store_is_the_default_and_is_reported() {
    let s = Sandbox::new();
    s.forged();
    s.ok(&["trust", "add", "--name", "lab", "keys/other.pub"]);
    let o = s.ok(&["verify-sig", "forged.tsra"]);
    assert_eq!(
        stderr(&o),
        format!(
            "accepted key_id {} via trust store {} (user store)\n",
            s.key_id(),
            s.user_store().display()
        )
    );
}

#[test]
fn a_pubkey_acceptance_is_reported_as_pubkey() {
    let s = Sandbox::new();
    s.forged();
    let o = s.ok(&[
        "verify-sig",
        "forged.tsra",
        "--pubkey",
        "keys/other.pub",
        "--require-signer",
        "anatomist-operator",
    ]);
    assert_eq!(
        stderr(&o),
        format!(
            "accepted key_id {} via --pubkey keys/other.pub\n",
            s.key_id()
        )
    );
    // --pubkey consults no store: a TESSERA_TRUST_DIR (even a nonexistent one) is not even resolved.
    let o = s
        .cmd()
        .args(["verify-sig", "forged.tsra", "--pubkey", "keys/other.pub"])
        .env("TESSERA_TRUST_DIR", "no-such-store")
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(
        stderr(&o).contains("via --pubkey keys/other.pub"),
        "{}",
        stderr(&o)
    );
    // --pubkey and --trust-dir contradict each other: a usage error (exit 2), not silently resolved.
    let o = s.run(&[
        "verify-sig",
        "forged.tsra",
        "--pubkey",
        "keys/other.pub",
        "--trust-dir",
        ".",
    ]);
    assert_eq!(o.status.code(), Some(2), "{}", stderr(&o));
    assert!(stderr(&o).contains("cannot be used with"), "{}", stderr(&o));
}

/// An empty or relative `XDG_CONFIG_HOME` / `HOME` must not turn the "user store" into a directory
/// under the cwd (`./tessera/trust`, `./.config/tessera/trust`) — the same planted-key hole as #576.
#[test]
fn an_empty_or_relative_config_home_never_makes_a_cwd_user_store() {
    let s = Sandbox::new();
    s.forged();
    for planted in [
        "tessera/trust",
        ".config/tessera/trust",
        "rel/tessera/trust",
    ] {
        let d = s.path().join(planted);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::copy(s.path().join("keys/other.pub"), d.join("planted.pub")).unwrap();
    }
    for (xdg, home) in [("", ""), ("rel", ""), ("", "."), ("rel", "rel")] {
        let o = s
            .cmd()
            .args(["verify-sig", "forged.tsra"])
            .env("XDG_CONFIG_HOME", xdg)
            .env("HOME", home)
            .output()
            .unwrap();
        assert_eq!(
            o.status.code(),
            Some(1),
            "XDG_CONFIG_HOME={xdg:?} HOME={home:?}\nstderr: {}",
            stderr(&o)
        );
    }
}

#[test]
fn a_named_trust_dir_that_does_not_exist_is_an_error() {
    let s = Sandbox::new();
    s.forged();
    let o = s.run(&["verify-sig", "forged.tsra", "--trust-dir", "no-such-store"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(
        stderr(&o).contains("trust store no-such-store (--trust-dir) is not a directory"),
        "{}",
        stderr(&o)
    );
}
