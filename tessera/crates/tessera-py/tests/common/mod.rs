//! Shared by the committed-fixture tests (`partitioned_fixture.rs`, `nullable_fixture.rs`): each
//! fixture under `tests/fixtures/` is rebuilt from its recipe and must equal the committed bytes, so
//! a committed file can't drift from the code that claims to produce it.

use std::path::{Path, PathBuf};

use tessera_core::block::table::Column;

/// The committed file `tests/fixtures/<name>`.
pub fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// A table column of `dtype`, nullable or not.
pub fn col(name: &str, dtype: &str, nullable: bool) -> Column {
    Column {
        name: name.into(),
        dtype: dtype.into(),
        codec: None,
        nullable,
        ..Default::default()
    }
}

/// Require the committed `tests/fixtures/<name>` to equal what `build` writes. With
/// `TESSERA_PY_FIXTURE_REGEN` set, `build` first rewrites the committed file; `then` names what
/// else to update after a regeneration.
pub fn assert_committed(name: &str, build: impl Fn(&Path), then: &str) {
    let committed = fixture_path(name);
    if std::env::var_os("TESSERA_PY_FIXTURE_REGEN").is_some() {
        build(&committed);
    }
    let fresh = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    build(&fresh);
    let want = std::fs::read(&fresh).unwrap();
    let have = std::fs::read(&committed).unwrap_or_else(|e| {
        panic!(
            "{}: {e}; regenerate with TESSERA_PY_FIXTURE_REGEN=1",
            committed.display()
        )
    });
    assert!(
        have == want,
        "{} differs from its recipe; regenerate with TESSERA_PY_FIXTURE_REGEN=1 and {then}",
        committed.display()
    );
}
