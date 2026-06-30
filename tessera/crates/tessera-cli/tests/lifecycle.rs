//! Full-scale **persona lifecycle** end-to-end test — drives the real `tessera` binary through the
//! whole journey, capturing ids from one command to feed the next (what the trycmd doc-walkthrough
//! can't express) and asserting the security rejections:
//!
//!   instrument scientist  keygen + sign the freshly-acquired study (air-gapped)
//!   data scientist        verify-sig refuses an UNKNOWN key (untrusted)
//!   data custodian        trust the key → verify-sig passes; an IMPERSONATOR re-signing with their own
//!                         key + the same `signer` is still rejected (the trust-store defence)
//!   researcher            version a metadata correction, diff it, publish a history-free artifact, verify
//!
//! Runs via `CARGO_BIN_EXE_tessera`, so it executes in the hermetic flake check with no cargo at test time.

use std::path::Path;
use std::process::{Command, Output};

use tessera_core::block::array::ArraySpec;
use tessera_core::ProductBuilder;
use tessera_io::array::{self, ArrayData};
use tessera_io::pack;

fn tessera() -> Command {
    Command::new(env!("CARGO_BIN_EXE_tessera"))
}

/// Build a small sealed `recon` `.tsra` fixture.
fn sealed(out: &Path, name: &str) {
    let mut spec = ArraySpec::new(vec![8, 8, 8], "int16");
    spec.codec = "pcodec".into();
    let data = ArrayData::I16((0..512).map(|k| k as i16).collect());
    let (bref, payload) = array::array_block("volume", &spec, &data).unwrap();
    let mut b = ProductBuilder::new("recon", name, "e2e", "2024-01-01T00:00:00Z");
    b.add_block_ref(bref);
    let m = b.seal().unwrap();
    pack(&m, &[payload], out).unwrap();
}

fn run(args: &[&str]) -> Output {
    tessera().args(args).output().unwrap()
}

/// Run, assert exit 0, return stdout.
fn ok(args: &[&str]) -> String {
    let o = run(args);
    assert!(
        o.status.success(),
        "expected success: tessera {args:?}\nstderr: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8(o.stdout).unwrap()
}

/// Run, assert non-zero exit.
fn fails(args: &[&str]) {
    assert!(
        !run(args).status.success(),
        "expected failure: tessera {args:?}"
    );
}

#[test]
fn persona_lifecycle_e2e() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    // Sandbox the trust store + repo: cwd-local `.tessera/trust` + `repo`, and an empty user store.
    std::env::set_current_dir(d).unwrap();
    std::env::set_var("HOME", d);
    std::env::remove_var("XDG_CONFIG_HOME");

    let study = d.join("study.tsra");
    sealed(&study, "DP06");
    let study = study.to_str().unwrap();
    let signer = "https://orcid.org/0000-0002-1825-0097";

    // 1. INSTRUMENT SCIENTIST — keygen + sign the freshly-acquired study.
    let key = d.join("instrument.key");
    let keys = key.to_str().unwrap();
    ok(&["keygen", keys]);
    let pubf = d.join("instrument.key.pub");
    assert!(pubf.exists(), "keygen wrote the .pub");
    ok(&["sign", study, "--key", keys, "--signer", signer]);

    // 2. DATA SCIENTIST — an unknown key is untrusted.
    fails(&["verify-sig", study]);

    // 3. DATA CUSTODIAN — vet + trust the key, then it verifies.
    ok(&[
        "trust",
        "add",
        pubf.to_str().unwrap(),
        "--name",
        "instrument-lab",
        "--signer",
        signer,
        "--repo",
    ]);
    assert!(ok(&["verify-sig", study]).contains("trusted as 'instrument-lab'"));

    // IMPERSONATION — an attacker re-signs with their OWN key + the same `signer`; the signature is
    // cryptographically valid but the key is NOT trusted, so verify-sig rejects it.
    let attacker = d.join("attacker.key");
    ok(&["keygen", attacker.to_str().unwrap()]);
    ok(&[
        "sign",
        study,
        "--key",
        attacker.to_str().unwrap(),
        "--signer",
        signer,
    ]);
    fails(&["verify-sig", study]);
    // restore the genuine signature for the rest of the journey.
    ok(&["sign", study, "--key", keys, "--signer", signer]);
    assert!(ok(&["verify-sig", study]).contains("trusted as"));

    // 4. RESEARCHER — version a metadata correction, inspect it, publish a clean artifact, verify.
    ok(&["init", "repo"]);
    let imported = ok(&["import", "repo", study]);
    let lineage = imported
        .lines()
        .find_map(|l| l.strip_prefix("imported lineage "))
        .expect("import prints the lineage id")
        .trim()
        .to_string();

    assert!(ok(&["commit", "repo", &lineage, "--set", "tracer=FLT"]).contains("supersedes"));
    let tip = std::fs::read_to_string(format!("repo/refs/{}", lineage.replace([':', '/'], "_")))
        .unwrap()
        .trim()
        .to_string();

    let diff = ok(&["diff", "repo", &tip]);
    assert!(
        diff.contains("NEW supersedes OLD"),
        "lineage verdict: {diff}"
    );
    assert!(
        diff.contains("tracer"),
        "the metadata edit shows in the diff: {diff}"
    );

    ok(&["publish", "repo", &tip, "published.tsra"]);
    ok(&["verify", "published.tsra"]);
    // the published product carries the metadata edit and is a self-contained `recon`.
    assert!(ok(&["tree", "published.tsra"]).contains("product=recon"));
}
