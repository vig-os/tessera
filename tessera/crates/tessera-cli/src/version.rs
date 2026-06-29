//! `tessera init` / `import` / `commit` / `log` — copy-on-write versioning over a content-addressed
//! repository (ADR-0036). `id` is the stable lineage handle (model A); `manifest_hash` is the version.
//!
//! Flow: `init` a repository, `import` a sealed `.tsra` as a lineage's first version, then `commit` a
//! metadata delta to mint a new version that **reuses every unchanged block by digest** — a
//! metadata-only edit writes exactly one new object (the manifest). `log` walks the lineage.
//!
//! As in `nav`, the commands write to a caller-supplied `Write` so they're unit-testable and `main`
//! owns stdout.

use std::io::Write;
use std::path::Path;

use serde_json::Value;
use tessera_core::provenance::Source;
use tessera_core::{Error, ProductBuilder, Result};
use tessera_io::{BlockPayload, Reader, Repository};

/// `tessera init REPO` — create the content-addressed repository.
pub fn init(repo: &Path, out: &mut dyn Write) -> Result<()> {
    Repository::init(repo)?;
    writeln!(out, "initialized tessera repository at {}", repo.display()).map_err(Error::from)?;
    Ok(())
}

/// `tessera import REPO FILE` — store a sealed `.tsra` as the first version of its lineage (its `id`).
pub fn import(repo: &Path, tsra: &Path, out: &mut dyn Write) -> Result<()> {
    let repo = Repository::open(repo)?;
    let mut r = Reader::open(tsra)?;
    let manifest = r.manifest().clone();
    let mut payloads = Vec::with_capacity(manifest.blocks.len());
    for name in r.block_names() {
        let bytes = r.read_block(&name)?;
        payloads.push(BlockPayload::new(name, bytes));
    }
    let mh = repo.commit(&manifest.id, &manifest, &payloads)?;
    writeln!(out, "imported lineage {}", manifest.id).map_err(Error::from)?;
    writeln!(out, "  version {mh}").map_err(Error::from)?;
    Ok(())
}

/// `tessera commit REPO LINEAGE --set k=v …` — mint a new version with a metadata delta, reusing
/// every unchanged block (copy ∝ delta). The new manifest carries a `supersedes` edge to the prior tip.
pub fn commit(repo: &Path, lineage: &str, sets: &[String], out: &mut dyn Write) -> Result<()> {
    if sets.is_empty() {
        return Err(Error::Invalid(
            "commit: nothing to change — pass at least one --set key=value".into(),
        ));
    }
    let repo = Repository::open(repo)?;
    let tip = repo.read_ref(lineage)?.ok_or_else(|| {
        Error::Invalid(format!(
            "no lineage '{lineage}' in this repository (import a .tsra first?)"
        ))
    })?;
    let parent = repo.get_manifest(&tip)?;

    let mut b = ProductBuilder::from_manifest(&parent);
    for kv in sets {
        let (k, v) = kv
            .split_once('=')
            .ok_or_else(|| Error::Invalid(format!("--set expects key=value, got '{kv}'")))?;
        // Parse the value as JSON (numbers/bools/objects), falling back to a bare string.
        let value =
            serde_json::from_str::<Value>(v).unwrap_or_else(|_| Value::String(v.to_string()));
        b.with_field(k, value);
    }
    b.add_source(Source::new("supersedes", &tip).with_content_hash(&tip));
    let new_manifest = b.seal()?;
    // Metadata-only: supply no payloads — every block is already stored from the parent.
    let mh = repo.commit(lineage, &new_manifest, &[])?;

    writeln!(out, "committed {lineage}").map_err(Error::from)?;
    writeln!(out, "  version    {mh}").map_err(Error::from)?;
    writeln!(out, "  supersedes {tip}").map_err(Error::from)?;
    Ok(())
}

/// `tessera log REPO LINEAGE` — the lineage's versions, newest first.
pub fn log(repo: &Path, lineage: &str, out: &mut dyn Write) -> Result<()> {
    let repo = Repository::open(repo)?;
    let entries = repo.log(lineage)?;
    if entries.is_empty() {
        writeln!(out, "no commits on lineage '{lineage}'").map_err(Error::from)?;
        return Ok(());
    }
    for (i, e) in entries.iter().rev().enumerate() {
        let tag = if i == 0 { "  (tip)" } else { "" };
        writeln!(out, "{}{tag}", e.manifest_hash).map_err(Error::from)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tessera_core::block::table::{Column, TableSpec};
    use tessera_core::ProductBuilder as PB;
    use tessera_io::{pack, table::table_block, ColumnData};

    /// Build + pack a sealed `.tsra` with one real table block and a `rev` metadata field.
    fn write_tsra(path: &Path) {
        let spec = TableSpec {
            columns: vec![Column {
                name: "x".into(),
                dtype: "u4".into(),
                codec: None,
            }],
            rows: 3,
            row_index: None,
        };
        let data = vec![("x".into(), ColumnData::U32(vec![1, 2, 3]))];
        let (block_ref, payload) = table_block("events", &spec, &data).unwrap();
        let mut b = PB::new("listmode", "DP06", "d", "2024-01-01T00:00:00Z");
        b.add_block_ref(block_ref);
        b.with_field("tracer", serde_json::json!("FDG"));
        let sealed = b.seal().unwrap();
        pack(&sealed, &[payload], path).unwrap();
    }

    fn count_objects(root: &Path) -> usize {
        fn rec(d: &Path) -> usize {
            let mut n = 0;
            if let Ok(rd) = std::fs::read_dir(d) {
                for e in rd.flatten() {
                    let p = e.path();
                    if p.is_dir() {
                        n += rec(&p);
                    } else {
                        n += 1;
                    }
                }
            }
            n
        }
        rec(&root.join("objects"))
    }

    #[test]
    fn import_then_metadata_commit_dedups_and_logs() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let tsra = dir.path().join("p.tsra");
        write_tsra(&tsra);

        init(&repo, &mut Vec::new()).unwrap();

        // import → lineage id + one version
        let mut buf = Vec::new();
        import(&repo, &tsra, &mut buf).unwrap();
        let out = String::from_utf8(buf).unwrap();
        let lineage = out
            .lines()
            .find_map(|l| l.strip_prefix("imported lineage "))
            .unwrap()
            .to_string();
        let after_import = count_objects(&repo); // 1 block + 1 manifest = 2

        // metadata-only commit → exactly ONE new object (the manifest); block reused
        commit(&repo, &lineage, &["tracer=FLT".into()], &mut Vec::new()).unwrap();
        assert_eq!(
            count_objects(&repo),
            after_import + 1,
            "metadata edit must not recopy the data block"
        );

        // the new tip's metadata reflects the edit, id (lineage) unchanged
        let r = Repository::open(&repo).unwrap();
        let tip = r.read_ref(&lineage).unwrap().unwrap();
        let m = r.get_manifest(&tip).unwrap();
        assert_eq!(m.id, lineage, "id is the stable lineage handle");
        assert_eq!(m.metadata.get("tracer"), Some(&serde_json::json!("FLT")));

        // log shows two versions, newest first
        let mut lbuf = Vec::new();
        log(&repo, &lineage, &mut lbuf).unwrap();
        let log_out = String::from_utf8(lbuf).unwrap();
        assert_eq!(log_out.lines().count(), 2);
        assert!(log_out.lines().next().unwrap().contains("(tip)"));
    }

    #[test]
    fn commit_without_import_errors() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        init(&repo, &mut Vec::new()).unwrap();
        let err = commit(&repo, "blake3:nope", &["a=b".into()], &mut Vec::new()).unwrap_err();
        assert!(format!("{err}").contains("no lineage"));
    }
}
