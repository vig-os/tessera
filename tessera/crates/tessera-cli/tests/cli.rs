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
