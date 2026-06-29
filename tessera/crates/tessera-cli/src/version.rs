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
use tessera_core::{Error, Manifest, ProductBuilder, Result};
use tessera_io::{pack, BlockPayload, Reader, Repository};

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

/// `tessera commit REPO LINEAGE [--set k=v] [--add-block …] [--remove-block N]` — mint a new version
/// with a delta, reusing every unchanged block (copy ∝ delta). The new manifest carries a `supersedes`
/// edge to the prior tip.
///
/// All three deltas are **pure object-store + manifest work over already-encoded blocks** — no codec
/// runs in the versioning layer:
/// - `--set k=v` — write one new manifest object (metadata).
/// - `--remove-block N` — drop a `BlockRef` (manifest edit; the object stays for other versions).
/// - `--add-block [N=]SRC.tsra:BLK` — copy an already-encoded + digested block from another `.tsra`
///   (store the object if absent + add a `BlockRef`). Composition, **not** encoding.
///
/// Encoding *raw* data into a new block (array→zarr/pcodec, table→Vortex) is `ingest`/`pack`'s job —
/// the "extra handling" tier, deliberately kept out of the versioning layer.
pub fn commit(
    repo: &Path,
    lineage: &str,
    sets: &[String],
    adds: &[String],
    removes: &[String],
    out: &mut dyn Write,
) -> Result<()> {
    if sets.is_empty() && adds.is_empty() && removes.is_empty() {
        return Err(Error::Invalid(
            "commit: nothing to change — pass --set / --add-block / --remove-block".into(),
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

    // Track live block names so a duplicate `--add-block` is rejected (start from the parent's blocks).
    let mut names: std::collections::BTreeSet<String> =
        parent.blocks.iter().map(|bl| bl.name.clone()).collect();

    // Removes first — manifest edit only; the data object is left for other versions / gc.
    for name in removes {
        if !b.remove_block(name) {
            return Err(Error::Invalid(format!(
                "--remove-block: no block '{name}' in this version"
            )));
        }
        names.remove(name);
    }

    // Adds — copy an already-encoded block from another .tsra (file i/o, no re-encode).
    let mut payloads = Vec::new();
    for spec in adds {
        let (new_ref, payload) = load_block(spec)?;
        if !names.insert(new_ref.name.clone()) {
            return Err(Error::Invalid(format!(
                "--add-block: a block named '{}' already exists (rename, or --remove-block it first)",
                new_ref.name
            )));
        }
        b.add_block_ref(new_ref);
        payloads.push(payload);
    }

    // Metadata sets.
    for kv in sets {
        let (k, v) = kv
            .split_once('=')
            .ok_or_else(|| Error::Invalid(format!("--set expects key=value, got '{kv}'")))?;
        let value =
            serde_json::from_str::<Value>(v).unwrap_or_else(|_| Value::String(v.to_string()));
        b.with_field(k, value);
    }

    b.add_source(Source::new("supersedes", &tip).with_content_hash(&tip));
    let new_manifest = b.seal()?;
    // commit stores the new block payloads content-addressed (dedup if already present) + the manifest.
    let mh = repo.commit(lineage, &new_manifest, &payloads)?;

    writeln!(out, "committed {lineage}").map_err(Error::from)?;
    writeln!(out, "  version    {mh}").map_err(Error::from)?;
    writeln!(out, "  supersedes {tip}").map_err(Error::from)?;
    if !payloads.is_empty() || !removes.is_empty() {
        writeln!(out, "  blocks     +{} -{}", payloads.len(), removes.len())
            .map_err(Error::from)?;
    }
    Ok(())
}

/// Load an already-encoded block from another sealed `.tsra` for `--add-block`. `spec` is
/// `[NEWNAME=]SOURCE.tsra:SRCBLOCK`; the block's bytes (digest-verified on read) and its descriptor
/// (kind/digest/spec) are copied verbatim — no re-encoding.
fn load_block(spec: &str) -> Result<(tessera_core::block::BlockRef, BlockPayload)> {
    let (newname, locator) = match spec.split_once('=') {
        Some((n, rest)) => (Some(n), rest),
        None => (None, spec),
    };
    let (path, srcblock) = locator.rsplit_once(':').ok_or_else(|| {
        Error::Invalid(format!(
            "--add-block expects [NAME=]SOURCE.tsra:BLOCK, got '{spec}'"
        ))
    })?;
    let newname = newname.unwrap_or(srcblock).to_string();
    let mut r = Reader::open(Path::new(path))?;
    let src_ref = r
        .manifest()
        .blocks
        .iter()
        .find(|b| b.name == srcblock)
        .ok_or_else(|| Error::Invalid(format!("no block '{srcblock}' in {path}")))?
        .clone();
    let bytes = r.read_block(srcblock)?; // digest-verified on read
    let new_ref = tessera_core::block::BlockRef {
        name: newname.clone(),
        kind: src_ref.kind,
        digest: src_ref.digest,
        spec: src_ref.spec,
    };
    Ok((new_ref, BlockPayload::new(newname, bytes)))
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

/// `blake3:<hex>` → a glanceable `blake3:1a2b3c4d…` prefix.
fn short(addr: &str) -> String {
    match addr.split_once(':') {
        Some((alg, hex)) if hex.len() > 8 => format!("{alg}:{}…", &hex[..8]),
        _ => addr.to_string(),
    }
}

/// `tessera diff REPO BASE [TARGET]` — structural delta between two versions (blocks added/removed/
/// changed, metadata fields changed) plus the **lineage verdict** (does TARGET's `supersedes` edge pin
/// BASE?). With one ref, diffs that version against its `supersedes` parent ("what this version changed").
pub fn diff(repo: &Path, first: &str, second: Option<&str>, out: &mut dyn Write) -> Result<()> {
    let repo = Repository::open(repo)?;
    let (old_mh, new_mh) = match second {
        Some(s) => (first.to_string(), s.to_string()),
        None => {
            // One ref → diff its parent → it. Resolve the parent from the supersedes edge.
            let m = repo.get_manifest(first)?;
            let parent = m
                .sources
                .iter()
                .find(|s| s.role == "supersedes")
                .and_then(|s| s.content_hash.clone())
                .ok_or_else(|| {
                    Error::Invalid(format!(
                        "{} has no supersedes parent — pass an explicit BASE",
                        short(first)
                    ))
                })?;
            (parent, first.to_string())
        }
    };
    let old = repo.get_manifest(&old_mh)?;
    let new = repo.get_manifest(&new_mh)?;

    writeln!(out, "diff {} → {}", short(&old_mh), short(&new_mh)).map_err(Error::from)?;

    // Lineage verdict: does NEW carry a supersedes edge pinning OLD's manifest_hash?
    let linked = new
        .sources
        .iter()
        .any(|s| s.role == "supersedes" && s.content_hash.as_deref() == Some(old_mh.as_str()));
    let verdict = if linked {
        "NEW supersedes OLD ✓".to_string()
    } else if old.id == new.id {
        "same lineage (id), but NEW has no direct supersedes edge to OLD".to_string()
    } else {
        format!(
            "different products (id {} vs {})",
            short(&old.id),
            short(&new.id)
        )
    };
    writeln!(out, "lineage: {verdict}").map_err(Error::from)?;

    // Blocks, keyed by name → digest.
    let ob: std::collections::BTreeMap<&str, &str> = old
        .blocks
        .iter()
        .map(|b| (b.name.as_str(), b.digest.as_deref().unwrap_or("-")))
        .collect();
    let nb: std::collections::BTreeMap<&str, &str> = new
        .blocks
        .iter()
        .map(|b| (b.name.as_str(), b.digest.as_deref().unwrap_or("-")))
        .collect();
    let mut block_names: std::collections::BTreeSet<&str> = ob.keys().copied().collect();
    block_names.extend(nb.keys().copied());
    let mut block_lines = 0u32;
    let mut unchanged = 0u32;
    let mut bbuf = String::new();
    for name in &block_names {
        match (ob.get(name), nb.get(name)) {
            (Some(o), Some(n)) if o != n => {
                bbuf.push_str(&format!("  ~ {name}  {} → {}\n", short(o), short(n)));
                block_lines += 1;
            }
            (None, Some(n)) => {
                bbuf.push_str(&format!("  + {name}  {}\n", short(n)));
                block_lines += 1;
            }
            (Some(o), None) => {
                bbuf.push_str(&format!("  - {name}  {}\n", short(o)));
                block_lines += 1;
            }
            _ => unchanged += 1,
        }
    }
    writeln!(out, "blocks: {block_lines} changed, {unchanged} unchanged").map_err(Error::from)?;
    write!(out, "{bbuf}").map_err(Error::from)?;

    // Metadata fields.
    let mut keys: std::collections::BTreeSet<&String> = old.metadata.keys().collect();
    keys.extend(new.metadata.keys());
    let mut meta_lines = 0u32;
    let mut mbuf = String::new();
    for k in &keys {
        match (old.metadata.get(*k), new.metadata.get(*k)) {
            (Some(o), Some(n)) if o != n => {
                mbuf.push_str(&format!("  ~ {k}  {o} → {n}\n"));
                meta_lines += 1;
            }
            (None, Some(n)) => {
                mbuf.push_str(&format!("  + {k}  {n}\n"));
                meta_lines += 1;
            }
            (Some(o), None) => {
                mbuf.push_str(&format!("  - {k}  {o}\n"));
                meta_lines += 1;
            }
            _ => {}
        }
    }
    writeln!(out, "metadata: {meta_lines} changed").map_err(Error::from)?;
    write!(out, "{mbuf}").map_err(Error::from)?;
    Ok(())
}

/// Collect every block a manifest references from the repository's object store, ready to pack.
fn gather_payloads(repo: &Repository, m: &Manifest) -> Result<Vec<BlockPayload>> {
    let mut payloads = Vec::with_capacity(m.blocks.len());
    for b in &m.blocks {
        let digest = b
            .digest
            .as_deref()
            .ok_or_else(|| Error::Invalid(format!("block '{}' has no digest", b.name)))?;
        payloads.push(BlockPayload::new(b.name.clone(), repo.read_object(digest)?));
    }
    Ok(payloads)
}

/// `tessera seal REPO VERSION OUT` (= `git bundle`) — export a version to a standalone `.tsra`
/// **with its history intact**: the manifest is emitted exactly as stored (same `manifest_hash`,
/// `supersedes`/derivation edges preserved), for archival where the lineage must travel with the data.
pub fn seal(repo: &Path, version: &str, out_path: &Path, out: &mut dyn Write) -> Result<()> {
    let repo = Repository::open(repo)?;
    let manifest = repo.get_manifest(version)?;
    let payloads = gather_payloads(&repo, &manifest)?;
    pack(&manifest, &payloads, out_path)?;
    writeln!(out, "sealed {} → {}", short(version), out_path.display()).map_err(Error::from)?;
    writeln!(
        out,
        "  history preserved (supersedes/derivation edges intact)"
    )
    .map_err(Error::from)?;
    Ok(())
}

/// `tessera publish REPO VERSION OUT [--anonymous]` (= `git archive`) — export a version to a
/// **history-free** standalone `.tsra`: drops the `supersedes` version chain (keeping scientific
/// `derived_from` provenance + metadata + data), so the artifact stands alone for DOI / handover. By
/// default it carries a single `snapshot_of` audit edge pinning the source version's `manifest_hash`
/// (one pointer back to the full-history version in the repository); `--anonymous` drops even that.
/// `id` stays the stable lineage handle (model A); only the seal is fresh.
pub fn publish(
    repo: &Path,
    version: &str,
    out_path: &Path,
    anonymous: bool,
    out: &mut dyn Write,
) -> Result<()> {
    let repo = Repository::open(repo)?;
    let src = repo.get_manifest(version)?;
    // from_manifest keeps id/blocks/metadata/derivation edges and drops the parent's supersedes chain.
    let mut b = ProductBuilder::from_manifest(&src);
    if !anonymous {
        b.add_source(Source::new("snapshot_of", version).with_content_hash(version));
    }
    let manifest = b.seal()?;
    let payloads = gather_payloads(&repo, &src)?;
    pack(&manifest, &payloads, out_path)?;

    writeln!(out, "published {} → {}", short(version), out_path.display()).map_err(Error::from)?;
    if anonymous {
        writeln!(out, "  history-free + anonymous (no back-pointer)").map_err(Error::from)?;
    } else {
        writeln!(out, "  history-free, snapshot_of {}", short(version)).map_err(Error::from)?;
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

    /// A sealed `.tsra` carrying one named table block (a distinct block to compose via --add-block).
    fn write_block_tsra(path: &Path, block: &str, vals: &[u32]) {
        let spec = TableSpec {
            columns: vec![Column {
                name: "v".into(),
                dtype: "u4".into(),
                codec: None,
            }],
            rows: u64::try_from(vals.len()).unwrap(),
            row_index: None,
        };
        let data = vec![("v".into(), ColumnData::U32(vals.to_vec()))];
        let (br, payload) = table_block(block, &spec, &data).unwrap();
        let mut b = PB::new("recon", "DP06-roi", "roi", "2024-01-01T00:00:00Z");
        b.add_block_ref(br);
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
        commit(
            &repo,
            &lineage,
            &["tracer=FLT".into()],
            &[],
            &[],
            &mut Vec::new(),
        )
        .unwrap();
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
        let err = commit(
            &repo,
            "blake3:nope",
            &["a=b".into()],
            &[],
            &[],
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(format!("{err}").contains("no lineage"));
    }

    #[test]
    fn diff_shows_metadata_change_and_lineage_verdict() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let tsra = dir.path().join("p.tsra");
        write_tsra(&tsra);
        init(&repo, &mut Vec::new()).unwrap();
        let mut ibuf = Vec::new();
        import(&repo, &tsra, &mut ibuf).unwrap();
        let lineage = String::from_utf8(ibuf)
            .unwrap()
            .lines()
            .find_map(|l| l.strip_prefix("imported lineage "))
            .unwrap()
            .to_string();
        commit(
            &repo,
            &lineage,
            &["tracer=FLT".into()],
            &[],
            &[],
            &mut Vec::new(),
        )
        .unwrap();

        let r = Repository::open(&repo).unwrap();
        let tip = r.read_ref(&lineage).unwrap().unwrap();

        // one-arg diff: parent → tip
        let mut buf = Vec::new();
        diff(&repo, &tip, None, &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(
            s.contains("NEW supersedes OLD"),
            "lineage verdict missing: {s}"
        );
        assert!(
            s.contains("blocks: 0 changed"),
            "block change unexpected: {s}"
        );
        assert!(s.contains("metadata: 1 changed"), "metadata count: {s}");
        assert!(s.contains("~ tracer"));
        assert!(s.contains("FDG") && s.contains("FLT"));
    }

    #[test]
    fn publish_is_history_free_seal_preserves_history() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let tsra = dir.path().join("p.tsra");
        write_tsra(&tsra);
        init(&repo, &mut Vec::new()).unwrap();
        let mut ibuf = Vec::new();
        import(&repo, &tsra, &mut ibuf).unwrap();
        let lineage = String::from_utf8(ibuf)
            .unwrap()
            .lines()
            .find_map(|l| l.strip_prefix("imported lineage "))
            .unwrap()
            .to_string();
        commit(
            &repo,
            &lineage,
            &["tracer=FLT".into()],
            &[],
            &[],
            &mut Vec::new(),
        )
        .unwrap();
        let r = Repository::open(&repo).unwrap();
        let tip = r.read_ref(&lineage).unwrap().unwrap();

        // publish: history-free, with the snapshot_of breadcrumb; data travels + verifies.
        let pub_out = dir.path().join("published.tsra");
        publish(&repo, &tip, &pub_out, false, &mut Vec::new()).unwrap();
        let mut pr = Reader::open(&pub_out).unwrap();
        let pm = pr.manifest().clone();
        assert!(
            !pm.sources.iter().any(|s| s.role == "supersedes"),
            "published is history-free"
        );
        assert!(
            pm.sources
                .iter()
                .any(|s| s.role == "snapshot_of" && s.content_hash.as_deref() == Some(tip.as_str())),
            "snapshot_of breadcrumb present"
        );
        assert_eq!(pm.id, lineage, "id stays the stable lineage handle");
        for n in pr.block_names() {
            pr.read_block(&n).unwrap(); // digest-verified — the data travelled
        }

        // publish --anonymous drops even the breadcrumb.
        let anon_out = dir.path().join("anon.tsra");
        publish(&repo, &tip, &anon_out, true, &mut Vec::new()).unwrap();
        let am = Reader::open(&anon_out).unwrap().manifest().clone();
        assert!(
            !am.sources.iter().any(|s| s.role == "snapshot_of"),
            "anonymous drops the back-pointer"
        );

        // seal: the exact version, history preserved.
        let seal_out = dir.path().join("sealed.tsra");
        seal(&repo, &tip, &seal_out, &mut Vec::new()).unwrap();
        let sm = Reader::open(&seal_out).unwrap().manifest().clone();
        assert_eq!(
            sm.manifest_hash.as_deref(),
            Some(tip.as_str()),
            "seal exports the exact version byte-for-byte"
        );
        assert!(
            sm.sources.iter().any(|s| s.role == "supersedes"),
            "seal preserves history"
        );
    }

    #[test]
    fn commit_add_block_composes_and_remove_drops() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let p1 = dir.path().join("p1.tsra");
        let p2 = dir.path().join("p2.tsra");
        write_tsra(&p1); // "listmode" with an "events" block
        write_block_tsra(&p2, "roi", &[9, 8]); // a distinct "roi" block to attach
        init(&repo, &mut Vec::new()).unwrap();
        let mut ibuf = Vec::new();
        import(&repo, &p1, &mut ibuf).unwrap();
        let lineage = String::from_utf8(ibuf)
            .unwrap()
            .lines()
            .find_map(|l| l.strip_prefix("imported lineage "))
            .unwrap()
            .to_string();
        let before = count_objects(&repo);

        // --add-block roi=<p2>:roi — attach the already-encoded block (file i/o, no re-encode).
        let add = format!("roi={}:roi", p2.to_str().unwrap());
        commit(&repo, &lineage, &[], &[add], &[], &mut Vec::new()).unwrap();
        assert_eq!(
            count_objects(&repo),
            before + 2,
            "the roi block object + the new manifest"
        );

        let r = Repository::open(&repo).unwrap();
        let tip = r.read_ref(&lineage).unwrap().unwrap();
        let m = r.get_manifest(&tip).unwrap();
        assert!(m.blocks.iter().any(|b| b.name == "events"));
        let roi = m.blocks.iter().find(|b| b.name == "roi").unwrap();
        r.read_object(roi.digest.as_deref().unwrap()).unwrap(); // roi data travelled + verifies

        // --remove-block events — manifest edit only; no new data object.
        let objs = count_objects(&repo);
        commit(
            &repo,
            &lineage,
            &[],
            &[],
            &["events".into()],
            &mut Vec::new(),
        )
        .unwrap();
        assert_eq!(
            count_objects(&repo),
            objs + 1,
            "remove writes only the new manifest"
        );
        let tip2 = r.read_ref(&lineage).unwrap().unwrap();
        let m2 = r.get_manifest(&tip2).unwrap();
        assert!(
            !m2.blocks.iter().any(|b| b.name == "events"),
            "events dropped"
        );
        assert!(m2.blocks.iter().any(|b| b.name == "roi"), "roi remains");
    }
}
