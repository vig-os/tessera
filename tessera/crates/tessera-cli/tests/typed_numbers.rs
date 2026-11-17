//! CLI e2e for a **typed number a seal cannot hold** (SPEC §1, I-JSON RFC 7493 §2.2, #886): a value a
//! person types as a JSON number (`--set k=v`, `--meta k=v`) stays a number, so one beyond ±(2^53−1)
//! (or past a double's range) is refused with a non-zero exit, naming the key, and nothing is
//! written. It is never sealed as a string instead. Runs the real binary via `CARGO_BIN_EXE_tessera`.

use std::path::Path;
use std::process::{Command, Output};

fn tessera(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tessera"))
        .args(args)
        .output()
        .unwrap()
}

fn ok(args: &[&str]) -> String {
    let o = tessera(args);
    assert!(
        o.status.success(),
        "{args:?} failed: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8(o.stdout).unwrap()
}

/// Assert `args` exits non-zero with `expected` on stderr.
fn refused(args: &[&str], expected: &str) {
    let o = tessera(args);
    let stderr = String::from_utf8_lossy(&o.stderr);
    assert!(
        !o.status.success(),
        "{args:?} exited 0 (stdout: {})",
        String::from_utf8_lossy(&o.stdout)
    );
    assert!(stderr.contains(expected), "{args:?}: stderr was {stderr}");
}

/// Every file under `dir`, recursively (the repository's object store + refs).
fn files(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            out.extend(files(&p));
        } else {
            out.push(p);
        }
    }
    out.sort();
    out
}

const ADVICE: &str = "exceeds ±(2^53−1) (I-JSON, RFC 7493 §2.2)";

fn ingest_blob(d: &Path, out: &Path, meta: &str) -> Vec<String> {
    let src = d.join("payload.bin");
    std::fs::write(&src, b"opaque").unwrap();
    [
        "ingest",
        "blob",
        src.to_str().unwrap(),
        out.to_str().unwrap(),
        "--name",
        "p",
        "--timestamp",
        "2024-01-01T00:00:00Z",
        "--meta",
        meta,
    ]
    .map(str::to_owned)
    .to_vec()
}

#[test]
fn commit_set_refuses_a_number_a_seal_cannot_hold() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let tsra = d.join("p.tsra");
    let args = ingest_blob(d, &tsra, "study=S1");
    ok(&args.iter().map(String::as_str).collect::<Vec<_>>());
    let repo = d.join("repo");
    ok(&["init", repo.to_str().unwrap()]);
    let imported = ok(&["import", repo.to_str().unwrap(), tsra.to_str().unwrap()]);
    let lineage = imported
        .lines()
        .find_map(|l| l.strip_prefix("imported lineage "))
        .unwrap()
        .to_owned();
    let before = files(&repo);

    let big = "9".repeat(401);
    for value in [big.as_str(), "1e400", "-1e400"] {
        let set = format!("x={value}");
        refused(
            &["commit", repo.to_str().unwrap(), &lineage, "--set", &set],
            &format!("--set x = {value} {ADVICE}"),
        );
        assert_eq!(files(&repo), before, "a refused --set {set} stores nothing");
    }

    // Text that is not a JSON number is still a string, as before.
    let committed = ok(&[
        "commit",
        repo.to_str().unwrap(),
        &lineage,
        "--set",
        "x=hello",
    ]);
    let version = committed
        .lines()
        .find_map(|l| l.trim().strip_prefix("version"))
        .unwrap()
        .trim();
    let out = d.join("published.tsra");
    ok(&[
        "publish",
        repo.to_str().unwrap(),
        version,
        out.to_str().unwrap(),
    ]);
    let manifest: serde_json::Value =
        serde_json::from_str(&ok(&["inspect", out.to_str().unwrap(), "--json"])).unwrap();
    assert_eq!(manifest["metadata"]["x"], "hello", "{manifest}");
}

#[test]
fn ingest_meta_refuses_a_number_a_seal_cannot_hold() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let out = d.join("p.tsra");
    let big = "9".repeat(401);
    let args = ingest_blob(d, &out, &format!("t={big}"));
    refused(
        &args.iter().map(String::as_str).collect::<Vec<_>>(),
        &format!("--meta t = {big} {ADVICE}"),
    );
    assert!(
        !out.exists(),
        "a refused ingest leaves no {}",
        out.display()
    );
}
