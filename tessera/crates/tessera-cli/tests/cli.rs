//! Docs-as-tests for the `tessera` CLI: every `tests/cmd/*.trycmd` walkthrough runs the real binary
//! and diffs its output against the documented transcript, so the CLI's UX cannot drift from the docs
//! (the trycmd layer of the guardrails docs-as-tests convention). The bin is resolved via
//! `CARGO_BIN_EXE_tessera`, so no `cargo` runs at test time — it works in the hermetic flake check.
//!
//! Regenerate snapshots after an intentional UX change: `TRYCMD=overwrite cargo test -p tessera-cli`.

#[test]
fn cli_walkthroughs() {
    // Before any walkthrough reads it, so a regenerated fixture is the one they run against.
    logical_tables_fixture();
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

/// The committed input of `logical_tables.trycmd`, a product with a partitioned table (ADR-0064):
/// `pairs/0000..0002` (3 + 3 + 2 rows, each leaf marked), a one-block table `markers`, and a node
/// `notes/` that holds a blob and no table. No writer produces partitioned tables yet (#712), so it
/// is built here through the public builder API and checked byte for byte against the committed
/// file, which keeps it reproducible; `TRYCMD=overwrite` rewrites it with the snapshots.
fn logical_tables_fixture() {
    use tessera_core::block::partition::{leaf_name, Partition};
    use tessera_core::block::table::{Column, TableSpec};
    use tessera_core::ProductBuilder;
    use tessera_io::table::{table_block, TableData};
    use tessera_io::{blob::blob_block, pack, ColumnData};

    let table = |name: &str, t: Vec<u32>, partition: Option<Partition>| {
        let rows = t.len() as u64;
        let e = t.iter().map(|&v| v as f32 / 2.0).collect();
        let spec = TableSpec {
            columns: vec![Column::new("t", "u4"), Column::new("e", "f4")],
            rows,
            row_index: None,
            partition,
        };
        let data: TableData = vec![
            ("t".into(), ColumnData::U32(t)),
            ("e".into(), ColumnData::F32(e)),
        ];
        table_block(name, &spec, &data).unwrap()
    };
    let leaves = [vec![1, 2, 3], vec![4, 5, 6], vec![7, 8]];
    let last = leaves.len() - 1;
    let mut blocks: Vec<_> = leaves
        .into_iter()
        .enumerate()
        .map(|(i, t)| {
            let index = i as u64;
            let name = leaf_name("pairs", index).unwrap();
            let marker = Partition {
                index,
                last: i == last,
            };
            table(name.as_str(), t, Some(marker))
        })
        .collect();
    blocks.push(table("markers", vec![2, 7], None));
    blocks.push(
        blob_block(
            "notes/readme",
            "readme.txt",
            Some("text/plain"),
            b"scan notes\n".to_vec(),
        )
        .unwrap(),
    );

    let mut b = ProductBuilder::new(
        "listmode",
        "scan",
        "a partitioned scan",
        "2024-01-01T00:00:00Z",
    );
    b.with_field("coincidence_mode", serde_json::json!("prompt-coincidence"));
    // Pinned, so the bytes depend on no build env (the default stamp reads `TESSERA_GIT_COMMIT`).
    b.with_producer(tessera_core::Producer::new("logical-tables-fixture", "1"));
    for (r, _) in &blocks {
        b.add_block_ref(r.clone());
    }
    let sealed = b.seal().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let built = dir.path().join("scan.tsra");
    let payloads: Vec<_> = blocks.into_iter().map(|(_, p)| p).collect();
    pack(&sealed, &payloads, &built).unwrap();
    let built = std::fs::read(built).unwrap();

    let committed = std::path::Path::new("tests/cmd/logical_tables.in/scan.tsra");
    if std::env::var_os("TRYCMD").is_some_and(|v| v == "overwrite") {
        std::fs::write(committed, &built).unwrap();
    }
    assert!(
        std::fs::read(committed).is_ok_and(|c| c == built),
        "{} differs from the product this test builds; regenerate it with `TRYCMD=overwrite`",
        committed.display()
    );
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
