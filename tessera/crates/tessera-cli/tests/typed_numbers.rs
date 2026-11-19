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

const OUT_OF_RANGE: &str = "is outside the range a sealed JSON number can hold";
const PAST_DOUBLE: &str = "is past a double's range (it reads as infinity)";

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
            &format!("--set x = {value} {PAST_DOUBLE}"),
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
        &format!("--meta t = {big} {PAST_DOUBLE}"),
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
    stored_x_of(d, repo, lineage, "x", text)
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

/// One header edit for [`nifti_with`]: an `f32` at a byte offset, an `i16`, or a byte.
enum Put {
    F(usize, f32),
    I(usize, i16),
    B(usize, u8),
}

/// A minimal NIfTI-1 `.nii` (int16, `dims`, index space unless `edits` set a transform), with the header
/// fields in `edits` written over the defaults.
fn nifti_with(path: &Path, dims: &[i16], edits: &[Put]) {
    let mut h = vec![0u8; 352];
    h[0..4].copy_from_slice(&348_i32.to_le_bytes());
    let mut all = vec![i16::try_from(dims.len()).unwrap()];
    all.extend_from_slice(dims);
    all.resize(8, 1);
    for (k, dim) in all.iter().enumerate() {
        h[40 + 2 * k..42 + 2 * k].copy_from_slice(&dim.to_le_bytes());
    }
    h[70..72].copy_from_slice(&4_i16.to_le_bytes()); // datatype int16
    h[72..74].copy_from_slice(&16_i16.to_le_bytes()); // bitpix
    for k in 0..4 {
        h[76 + 4 * k..80 + 4 * k].copy_from_slice(&1.0_f32.to_le_bytes()); // pixdim
    }
    h[108..112].copy_from_slice(&352.0_f32.to_le_bytes()); // vox_offset
    h[344..348].copy_from_slice(b"n+1\0");
    for e in edits {
        match *e {
            Put::F(o, v) => h[o..o + 4].copy_from_slice(&v.to_le_bytes()),
            Put::I(o, v) => h[o..o + 2].copy_from_slice(&v.to_le_bytes()),
            Put::B(o, v) => h[o] = v,
        }
    }
    let voxels: i16 = dims.iter().product();
    for v in 0..voxels {
        h.extend_from_slice(&v.to_le_bytes());
    }
    std::fs::write(path, h).unwrap();
}

/// A NIfTI whose `scl_slope` header field is `slope`.
fn nifti_with_scl_slope(path: &Path, slope: f32) {
    nifti_with(path, &[2, 2, 2], &[Put::F(112, slope)]);
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

/// A non-zero number that reads as 0.0 (`5e-400`) used to seal as `0.0` (rc=0): refused at `--meta` and
/// in JSON, while zero in any spelling and an exact denormal stay (#886 round 3).
#[test]
fn ingest_meta_refuses_an_underflowing_number_and_keeps_zero_and_denormals() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    for (i, text) in ["5e-400", "-5e-400", r#"{"a":5e-400}"#]
        .into_iter()
        .enumerate()
    {
        let out = d.join(format!("u{i}.tsra"));
        let args = ingest_blob(d, &out, &format!("k={text}"));
        refused(
            &args.iter().map(String::as_str).collect::<Vec<_>>(),
            "is too small for a double (it reads as 0",
        );
        assert!(!out.exists(), "{text}: a refused ingest leaves no output");
    }
    for (i, text) in ["1e-320", "0e5", "0", "-0", "0.0"].into_iter().enumerate() {
        let out = d.join(format!("z{i}.tsra"));
        let args = ingest_blob(d, &out, &format!("k={text}"));
        ok(&args.iter().map(String::as_str).collect::<Vec<_>>());
    }
}

/// The same refusal in a TOML spec's metadata, naming the key (the TOML parser keeps only the double,
/// so the token is read from the source text).
#[test]
fn ingest_spec_refuses_an_underflowing_toml_float_and_keeps_zero_and_denormals() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let src = d.join("payload.bin");
    std::fs::write(&src, b"opaque").unwrap();
    let run = |line: &str, tag: &str| {
        let spec = spec_file(d, "blob", &src, line);
        let out = d.join(format!("out-{tag}"));
        let args = [
            "ingest",
            "--spec",
            spec.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ];
        (args.map(str::to_owned), out)
    };
    let (args, out) = run("x = 5e-400", "tiny");
    refused(
        &args.each_ref().map(String::as_str),
        "product[0].metadata.x = \"5e-400\" is too small for a double",
    );
    assert!(!out.join("collection.json").exists());
    // The span-aware parse used to fail on a dotted key or an implicit table and the check then passed
    // the document (#886 round 4): each of these sealed `0.0` with rc=0.
    for (i, (line, path)) in [
        ("x = 5e-400\ndeep.z = 1", "x"),
        ("x = 5e-400\n[product.metadata.deep.deeper]\nz = 1", "x"),
        (
            "[product.metadata.deep.deeper]\ny = 1e-400",
            "deep.deeper.y",
        ),
        ("deep.a.b = 1e-400", "deep.a.b"),
        ("deep = { a = [ { b = 1e-400 } ] }", "deep.a[0].b"),
    ]
    .into_iter()
    .enumerate()
    {
        let (args, out) = run(line, &format!("dotted{i}"));
        refused(
            &args.each_ref().map(String::as_str),
            &format!("{path} = \""),
        );
        assert!(!out.join("collection.json").exists(), "{line}");
    }
    // In-range dotted and implicit tables still ingest.
    let (args, _) = run(
        "deep.a.b = 1.5\n[product.metadata.other.deeper]\nz = 1",
        "dotted-ok",
    );
    ok(&args.each_ref().map(String::as_str));
    for (i, line) in ["x = 1e-320", "x = 0e5", "x = 0.0", "x = [0.0, 1e-320]"]
        .into_iter()
        .enumerate()
    {
        let (args, _) = run(line, &format!("ok{i}"));
        ok(&args.each_ref().map(String::as_str));
    }
}

/// `--set` of an integer beyond the bound keeps the advice a person can follow (the value is free-form
/// metadata, which accepts a string).
#[test]
fn commit_set_beyond_the_bound_advises_the_quoted_string() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let (repo, lineage) = repo_with_product(d);
    refused(
        &[
            "commit",
            repo.to_str().unwrap(),
            &lineage,
            "--set",
            "x=9007199254740993",
        ],
        &format!("--set x = 9007199254740993 {OUT_OF_RANGE}"),
    );
    refused(
        &[
            "commit",
            repo.to_str().unwrap(),
            &lineage,
            "--set",
            "x=9007199254740993",
        ],
        "; write it as a string",
    );
}

/// `--column-meta` `scale` of `nan`, `inf`, `1e16` or an underflow is refused naming the flag and the
/// column, with no output (#886 round 3).
#[test]
fn ingest_table_refuses_an_unsealable_column_meta_scale() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let csv = d.join("t.csv");
    std::fs::write(&csv, "a,b\n1,2\n3,4\n").unwrap();
    let run = |scale: &str, tag: &str| {
        let meta = d.join(format!("cm-{tag}.toml"));
        std::fs::write(&meta, format!("[a]\nscale = {scale}\n")).unwrap();
        let out = d.join(format!("out-{tag}.tsra"));
        let args = [
            "ingest",
            "table",
            csv.to_str().unwrap(),
            out.to_str().unwrap(),
            "--from",
            "csv",
            "--column",
            "a:i4",
            "--column",
            "b:i4",
            "--name",
            "t",
            "--timestamp",
            "2024-01-01T00:00:00Z",
            "--column-meta",
            meta.to_str().unwrap(),
        ];
        (args.map(str::to_owned), out)
    };
    for (scale, tag, why) in [
        ("nan", "nan", "\"NaN\" is not a finite number"),
        ("inf", "inf", "\"inf\" is not a finite number"),
        (
            "1e16",
            "big",
            "\"10000000000000000\" is outside the range a sealed JSON number can hold",
        ),
        ("5e-400", "tiny", "\"5e-400\" is too small for a double"),
    ] {
        let (args, out) = run(scale, tag);
        // An underflow is read from the TOML source, which names the key by its path (`a.scale`).
        let named = if tag == "tiny" {
            "a.scale"
        } else {
            "column 'a' scale"
        };
        refused(
            &args.each_ref().map(String::as_str),
            &format!("--column-meta: {named} = {why}"),
        );
        assert!(
            !out.exists(),
            "scale = {scale}: a refused ingest leaves no output"
        );
    }
    // The dotted-key form of the same file (`a.scale = …`) is inspected too.
    {
        let meta = d.join("cm-dotted.toml");
        std::fs::write(&meta, "a.scale = 5e-400\n").unwrap();
        let out = d.join("out-dotted-tiny.tsra");
        let args = [
            "ingest",
            "table",
            csv.to_str().unwrap(),
            out.to_str().unwrap(),
            "--from",
            "csv",
            "--column",
            "a:i4",
            "--column",
            "b:i4",
            "--name",
            "t",
            "--timestamp",
            "2024-01-01T00:00:00Z",
            "--column-meta",
            meta.to_str().unwrap(),
        ];
        refused(
            &args,
            "--column-meta: a.scale = \"5e-400\" is too small for a double",
        );
        assert!(!out.exists(), "a refused ingest leaves no output");
    }
    let (args, _) = run("0.5", "ok");
    ok(&args.each_ref().map(String::as_str));
}

/// Every NIfTI header value that feeds the manifest is refused at the door, naming the header field and
/// its value, with no output: the sform entries, the qform spacing, the time step and `toffset` (#886
/// round 3). Before the fix each reached the seal-time check, which named a JSON path and advised "write
/// it as a string" (or, for a NaN spacing, was clamped to 1.0 and sealed with rc=0).
#[test]
fn ingest_nifti_refuses_every_unsealable_header_value_naming_the_field() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let (big, big_text) = (1e16_f32, "10000000000000000");
    let sform = |o: usize, v: f32| {
        vec![
            Put::I(254, 1),
            Put::F(280, 1.0),
            Put::F(300, 1.0),
            Put::F(320, 1.0),
            Put::F(o, v),
        ]
    };
    let qform = |o: usize, v: f32| vec![Put::I(252, 1), Put::F(o, v)];
    // 8 = seconds: the 4-D series' time axis is in use.
    let time = |o: usize, v: f32| vec![Put::B(123, 10), Put::F(92, 2.5), Put::F(o, v)];
    /// A case: label, dims, header edits, expected field, expected value text.
    type Case<'a> = (&'a str, &'a [i16], Vec<Put>, &'a str, String);
    let mut cases: Vec<Case> = vec![
        (
            "srow_x[0]",
            &[2, 2, 2],
            sform(280, big),
            "srow_x[0]",
            big_text.into(),
        ),
        (
            "srow_z[3]",
            &[2, 2, 2],
            sform(324, big),
            "srow_z[3]",
            big_text.into(),
        ),
        (
            "pixdim[1]=1e16",
            &[2, 2, 2],
            qform(80, big),
            "pixdim[1]",
            big_text.into(),
        ),
        (
            "pixdim[1]=NaN",
            &[2, 2, 2],
            qform(80, f32::NAN),
            "pixdim[1]",
            "NaN".into(),
        ),
        (
            "pixdim[4]=1e16",
            &[2, 2, 2, 2],
            time(92, big),
            "pixdim[4]",
            big_text.into(),
        ),
        (
            "pixdim[4]=NaN",
            &[2, 2, 2, 2],
            time(92, f32::NAN),
            "pixdim[4]",
            "NaN".into(),
        ),
        (
            "toffset=1e16",
            &[2, 2, 2, 2],
            time(136, big),
            "toffset",
            big_text.into(),
        ),
        (
            "toffset=NaN",
            &[2, 2, 2, 2],
            time(136, f32::NAN),
            "toffset",
            "NaN".into(),
        ),
    ];
    for (label, dims, edits, field, text) in cases.drain(..) {
        let nii = d.join(format!("{}.nii", label.replace(['[', ']', '='], "_")));
        nifti_with(&nii, dims, &edits);
        let spec = spec_file(d, "nifti", &nii, "");
        let out = d.join(format!("out-{}", label.replace(['[', ']', '='], "_")));
        let args = [
            "ingest",
            "--spec",
            spec.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ];
        refused(&args, &format!("nifti: {field} = \"{text}\" "));
        let o = tessera(&args);
        assert!(
            !String::from_utf8_lossy(&o.stderr).contains("as a string"),
            "{label}: a header field cannot be rewritten as a string"
        );
        assert!(
            !out.exists() || files(&out).is_empty(),
            "{label}: a refused ingest writes no output"
        );
    }
    // An in-range header with both transforms and a time axis ingests as before.
    let nii = d.join("fine.nii");
    let mut edits = sform(280, 2.0);
    edits.extend(time(136, 1.0));
    nifti_with(&nii, &[2, 2, 2, 2], &edits);
    let out = d.join("out-fine");
    ok(&[
        "ingest",
        "--spec",
        spec_file(d, "nifti", &nii, "").to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
}

/// `commit --set` holds a value to the range its field declares in the schema the version carries
/// (#895): a corpus `recon` product embeds `recon` 1.1, whose `kvp` is ranged 0–1000 kV, so
/// `--set kvp=5000` is refused naming the flag and nothing is stored, while `--set kvp=120` commits.
#[test]
fn commit_set_refuses_a_value_outside_its_schema_range_naming_the_flag() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let corpus = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../corpus/files/recon_int16.tsra"
    );
    let repo = d.join("repo");
    ok(&["init", repo.to_str().unwrap()]);
    let imported = ok(&["import", repo.to_str().unwrap(), corpus]);
    let lineage = imported
        .lines()
        .find_map(|l| l.strip_prefix("imported lineage "))
        .unwrap()
        .to_owned();
    let before = files(&repo);
    refused(
        &[
            "commit",
            repo.to_str().unwrap(),
            &lineage,
            "--set",
            "kvp=5000",
        ],
        "--set kvp: field 'kvp' of schema 'recon': 5000 is above its maximum 1000",
    );
    assert_eq!(files(&repo), before, "a refused --set stores nothing");
    assert_eq!(stored_x_of(d, &repo, &lineage, "kvp", "120"), 120);
}

/// Commit `key=<text>` and read back what the seal holds under `metadata.<key>`.
fn stored_x_of(d: &Path, repo: &Path, lineage: &str, key: &str, text: &str) -> serde_json::Value {
    let set = format!("{key}={text}");
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
    manifest["metadata"][key].clone()
}
