//! Docs-as-tests for the `tessera` CLI: every `tests/cmd/*.trycmd` walkthrough runs the real binary
//! and diffs its output against the documented transcript, so the CLI's UX cannot drift from the docs
//! (the trycmd layer of the guardrails docs-as-tests convention). The bin is resolved via
//! `CARGO_BIN_EXE_tessera`, so no `cargo` runs at test time — it works in the hermetic flake check.
//!
//! Regenerate snapshots after an intentional UX change: `TRYCMD=overwrite cargo test -p tessera-cli`.

#[test]
fn cli_walkthroughs() {
    // The bin is `tessera` but the package is `tessera-cli`, so trycmd's package-name auto-detection
    // doesn't find it — register it explicitly by the `CARGO_BIN_EXE_tessera` path cargo sets for this
    // integration test (no cargo invocation at test time → works in the hermetic flake check).
    trycmd::TestCases::new()
        .register_bin(
            "tessera",
            std::path::PathBuf::from(env!("CARGO_BIN_EXE_tessera")),
        )
        .case("tests/cmd/*.trycmd");
}

/// `TRYCMD=overwrite` cannot tell an intended error from a regression: it records whatever the binary
/// printed. When these walkthroughs' content-addressed ids changed, three `versioning.trycmd` cases
/// (`diff` / `publish` / `verify`) were re-blessed against a **stale version hash** and so began
/// asserting `error: io: No such file or directory` as their expected output. The suite stayed green
/// while documenting three verbs as broken — and because these files are `{{#include}}`d into the book,
/// the rendered versioning chapter showed readers exactly that (#489).
///
/// A blessed snapshot is only as good as the read afterwards, so this gate does the read. Some
/// walkthroughs teach a real failure on purpose — a clap flag conflict, the `sql` feature gate, a
/// rejected impersonation — so a nonzero exit is not itself suspicious. A **filesystem** error is: no
/// walkthrough deliberately teaches "the file I just told you to create is not there".
#[test]
fn no_walkthrough_asserts_an_accidental_io_error() {
    // Phrases that only ever appear when a walkthrough's own earlier step failed to produce something
    // a later step needs. None of these is ever the lesson.
    const NEVER_INTENDED: [&str; 3] = [
        "No such file or directory",
        "os error 2",
        "io: No such file",
    ];
    let dir = std::path::Path::new("tests/cmd");
    let mut offenders = Vec::new();
    let mut checked = 0;
    for entry in std::fs::read_dir(dir).expect("tests/cmd must exist") {
        let path = entry.expect("readable dir entry").path();
        if path.extension().is_none_or(|e| e != "trycmd") {
            continue;
        }
        checked += 1;
        let text = std::fs::read_to_string(&path).expect("a .trycmd is UTF-8");
        for (n, line) in text.lines().enumerate() {
            if let Some(hit) = NEVER_INTENDED.iter().find(|p| line.contains(**p)) {
                offenders.push(format!(
                    "{}:{}: expects {hit:?} — a walkthrough step is broken, not teaching",
                    path.display(),
                    n + 1
                ));
            }
        }
    }
    assert!(checked > 0, "found no .trycmd walkthroughs to check");
    assert!(
        offenders.is_empty(),
        "walkthroughs assert accidental io errors as expected output ({checked} files scanned):\n{}",
        offenders.join("\n")
    );
}
