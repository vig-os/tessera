//! CLI e2e for the **blob / "junk" preservation tier** — an opaque vendor file (think Siemens `.l64`)
//! seals bit-faithfully via `tessera ingest junk` (the easter-egg alias of `blob`), survives `verify`
//! (which re-hashes it), and `tessera extract` recovers it **byte-identical**. Runs the real binary via
//! `CARGO_BIN_EXE_tessera`, so it executes inside the hermetic flake check with no cargo at test time.

use std::process::Command;

fn tessera() -> Command {
    Command::new(env!("CARGO_BIN_EXE_tessera"))
}

#[test]
fn junk_alias_seals_a_file_and_extract_is_byte_identical() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();

    // A deterministic "junk" payload: high-entropy bytes with embedded NULs — the kind of opaque
    // vendor dump no codec/parser would touch.
    let bytes: Vec<u8> = (0..30_000u32)
        .map(|k| (k.wrapping_mul(2_654_435_761) >> 11) as u8)
        .collect();
    let src = d.join("ugly.l64");
    std::fs::write(&src, &bytes).unwrap();
    let tsra = d.join("ugly.tsra");

    // Ingest via the `junk` alias (identical to `blob`) — seals an opaque `blob` product.
    let o = tessera()
        .args([
            "ingest",
            "junk",
            src.to_str().unwrap(),
            tsra.to_str().unwrap(),
            "--name",
            "KSB-ugly",
            "--timestamp",
            "2024-01-01T00:00:00Z",
        ])
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "ingest junk failed: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    // schema-driven WARN tier: the `blob` schema marks `study` recommended, so ingesting without it
    // emits a non-fatal nudge on stderr (never blocks) — the FieldSpec severity + engine warn + CLI
    // subscriber, end to end.
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("recommended metadata 'study'"),
        "expected the recommended-field warn on stderr"
    );

    // verify re-hashes every block (the blob's blake3) — a sealed blob is integrity-checked.
    assert!(tessera()
        .args(["verify", tsra.to_str().unwrap()])
        .status()
        .unwrap()
        .success());

    // schema-valid: the built-in `blob` product schema requires the `data` blob block.
    assert!(tessera()
        .args(["schema", tsra.to_str().unwrap()])
        .status()
        .unwrap()
        .success());

    // extract → byte-identical recovery of the original file.
    let out = d.join("out.l64");
    assert!(tessera()
        .args([
            "extract",
            tsra.to_str().unwrap(),
            "data",
            out.to_str().unwrap(),
        ])
        .status()
        .unwrap()
        .success());
    assert_eq!(
        std::fs::read(&out).unwrap(),
        bytes,
        "extracted blob must be byte-identical to the ingested file"
    );
}
