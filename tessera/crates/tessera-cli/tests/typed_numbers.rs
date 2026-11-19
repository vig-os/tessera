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

/// Import a blob product into a fresh repo and return `(repo, lineage)`.
fn repo_with_product(d: &Path) -> (std::path::PathBuf, String) {
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
    (repo, lineage)
}

/// Commit `x=<text>` and read back what the seal holds under `metadata.x`.
fn stored_x(d: &Path, repo: &Path, lineage: &str, text: &str) -> serde_json::Value {
    let set = format!("x={text}");
    let committed = ok(&["commit", repo.to_str().unwrap(), lineage, "--set", &set]);
    let version = committed
        .lines()
        .find_map(|l| l.trim().strip_prefix("version"))
        .unwrap()
        .trim()
        .to_owned();
    let out = d.join(format!("published-{}.tsra", version.replace(':', "_")));
    ok(&[
        "publish",
        repo.to_str().unwrap(),
        &version,
        out.to_str().unwrap(),
    ]);
    let manifest: serde_json::Value =
        serde_json::from_str(&ok(&["inspect", out.to_str().unwrap(), "--json"])).unwrap();
    manifest["metadata"]["x"].clone()
}

/// A text Rust reads as a number but JSON does not spell as one (`+5`, `Infinity`, `+1e400`) used to be
/// stored as a *string* (rc=0). It is refused, naming the flag and key, and nothing is stored; the
/// quoted form stores the text (#886 round 2).
#[test]
fn commit_set_refuses_a_number_spelled_loosely_and_the_quoted_form_stores_the_text() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let (repo, lineage) = repo_with_product(d);
    let before = files(&repo);
    for text in [
        "+9007199254740993",
        "+1e400",
        "Infinity",
        "-Infinity",
        "+5",
        "inf",
        "NaN",
    ] {
        let set = format!("x={text}");
        refused(
            &["commit", repo.to_str().unwrap(), &lineage, "--set", &set],
            &format!("--set x = {text} "),
        );
        assert_eq!(files(&repo), before, "a refused --set {set} stores nothing");
    }
    // The refusal says how to keep the text.
    refused(
        &["commit", repo.to_str().unwrap(), &lineage, "--set", "x=+5"],
        "quote it as a JSON string: --set x='\"+5\"'",
    );

    // The quoted form is a JSON string: the text is stored as it is.
    assert_eq!(stored_x(d, &repo, &lineage, "\"+5\""), "+5");
    assert_eq!(stored_x(d, &repo, &lineage, "\"Infinity\""), "Infinity");
    // Non-numeric text stays a bare string; plain JSON numbers stay numbers.
    assert_eq!(stored_x(d, &repo, &lineage, "hello"), "hello");
    assert_eq!(stored_x(d, &repo, &lineage, "0x10"), "0x10");
    assert_eq!(stored_x(d, &repo, &lineage, "5"), serde_json::json!(5));
    assert_eq!(
        stored_x(d, &repo, &lineage, "9007199254740991"),
        serde_json::json!(9_007_199_254_740_991_u64)
    );
}

#[test]
fn ingest_meta_refuses_a_number_spelled_loosely() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    for text in ["+5", "Infinity", "-Infinity", "+1e400", "+9007199254740993"] {
        let out = d.join(format!("p{}.tsra", text.len()));
        let args = ingest_blob(d, &out, &format!("t={text}"));
        refused(
            &args.iter().map(String::as_str).collect::<Vec<_>>(),
            &format!("--meta t = {text} "),
        );
        assert!(
            !out.exists(),
            "a refused ingest leaves no {}",
            out.display()
        );
    }
    // The quoted form is accepted and stored as text.
    let out = d.join("quoted.tsra");
    let args = ingest_blob(d, &out, "t=\"+5\"");
    ok(&args.iter().map(String::as_str).collect::<Vec<_>>());
    let manifest: serde_json::Value =
        serde_json::from_str(&ok(&["inspect", out.to_str().unwrap(), "--json"])).unwrap();
    assert_eq!(manifest["metadata"]["t"], "+5");
}

/// A one-product spec (`blob` or `nifti`) with extra `[product.metadata]` lines; returns its path.
fn spec_file(d: &Path, format: &str, input: &Path, metadata: &str) -> std::path::PathBuf {
    // `blob` needs a media type; the `recon` schema needs a modality, which a NIfTI header doesn't name.
    let (schema, product_extra, modality) = if format == "nifti" {
        (
            "recon",
            "",
            "modality = { _vocabulary = \"DICOM\", _code = \"MR\" }",
        )
    } else {
        ("blob", "media_type = \"application/octet-stream\"", "")
    };
    let text = format!(
        r#"[collection]
name = "C"
description = "d"
timestamp = "2024-01-01T00:00:00Z"
study = "S"

[spec]
description = "d"

[[product]]
name = "p"
role = "raw"
schema = "{schema}"
description = "d"
format = "{format}"
input = "{input}"
{product_extra}

[product.metadata]
study = "S"
{modality}
{metadata}
"#,
        input = input.display()
    );
    let path = d.join(format!("spec-{format}.toml"));
    std::fs::write(&path, text).unwrap();
    path
}

/// TOML has `inf`, `-inf` and `nan`; deserialising one into JSON gave `null`, which sealed with rc=0 and
/// verified. It is refused before anything is written, naming the key (#886 round 2).
#[test]
fn ingest_spec_refuses_a_non_finite_toml_float() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let src = d.join("payload.bin");
    std::fs::write(&src, b"opaque").unwrap();
    for (line, shown) in [("x = inf", "inf"), ("x = -inf", "-inf"), ("x = nan", "NaN")] {
        let spec = spec_file(d, "blob", &src, line);
        let out = d.join(format!("out-{shown}"));
        refused(
            &[
                "ingest",
                "--spec",
                spec.to_str().unwrap(),
                "--out",
                out.to_str().unwrap(),
            ],
            &format!("product[0].metadata.x = {shown} is not a finite number"),
        );
        assert!(
            !out.join("collection.json").exists(),
            "{line}: a refused spec writes no collection"
        );
    }
    // A finite float is still sealed as the number it is.
    let spec = spec_file(d, "blob", &src, "x = 1.5");
    let out = d.join("out-ok");
    ok(&[
        "ingest",
        "--spec",
        spec.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
}

/// A minimal NIfTI-1 `.nii` (int16, 2×2×2, index space) whose `scl_slope` header field is `slope`.
fn nifti_with_scl_slope(path: &Path, slope: f32) {
    let mut h = vec![0u8; 352];
    h[0..4].copy_from_slice(&348_i32.to_le_bytes());
    for (k, dim) in [3_i16, 2, 2, 2, 1, 1, 1, 1].iter().enumerate() {
        h[40 + 2 * k..42 + 2 * k].copy_from_slice(&dim.to_le_bytes());
    }
    h[70..72].copy_from_slice(&4_i16.to_le_bytes()); // datatype int16
    h[72..74].copy_from_slice(&16_i16.to_le_bytes()); // bitpix
    for k in 0..4 {
        h[76 + 4 * k..80 + 4 * k].copy_from_slice(&1.0_f32.to_le_bytes()); // pixdim
    }
    h[108..112].copy_from_slice(&352.0_f32.to_le_bytes()); // vox_offset
    h[112..116].copy_from_slice(&slope.to_le_bytes()); // scl_slope
    h[344..348].copy_from_slice(b"n+1\0");
    for v in 0..8_i16 {
        h.extend_from_slice(&v.to_le_bytes());
    }
    std::fs::write(path, h).unwrap();
}

/// `scl_slope` of `inf`, `nan` or `1e16` is refused at the NIfTI door, naming the header field and no
/// output file, instead of sealing `rescale_slope: null` (rc=0) or advising "write it as a string".
#[test]
fn ingest_nifti_refuses_an_unsealable_scl_slope() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    for (slope, shown, why) in [
        (f32::INFINITY, "inf", "is not a finite number"),
        (f32::NAN, "NaN", "is not a finite number"),
        (
            1e16,
            "10000000000000000",
            "is outside the range a sealed JSON number can hold",
        ),
    ] {
        let nii = d.join(format!("scl-{shown}.nii"));
        nifti_with_scl_slope(&nii, slope);
        let spec = spec_file(d, "nifti", &nii, "");
        let out = d.join(format!("out-{shown}"));
        refused(
            &[
                "ingest",
                "--spec",
                spec.to_str().unwrap(),
                "--out",
                out.to_str().unwrap(),
            ],
            &format!("nifti: scl_slope = \"{shown}\" {why}"),
        );
        assert!(
            !out.exists() || files(&out).is_empty(),
            "scl_slope={shown}: a refused ingest writes no output"
        );
    }
    let nii = d.join("scl-ok.nii");
    nifti_with_scl_slope(&nii, 2.0);
    let out = d.join("out-ok");
    ok(&[
        "ingest",
        "--spec",
        spec_file(d, "nifti", &nii, "").to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
}
