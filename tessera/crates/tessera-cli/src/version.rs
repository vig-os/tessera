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
    for b in &manifest.blocks {
        let bytes = r.read_block(&b.name)?;
        payloads.push(BlockPayload::new(b.name.clone(), bytes));
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
/// - `--remove-block N` — drop a `BlockRef` and its derived family (manifest edit; the objects stay
///   for other versions): its `N.cidx` sidecar and, when `N` is pyramid level 0 (`BASE/0`), the
///   derived levels `BASE/1…` and their sidecars ([`tessera_core::block::derived_family`]), so no
///   accelerator outlives the data it was computed from (ADR-0028 §4).
/// - `--add-block [N=]SRC.tsra:BLK` — copy an already-encoded + digested block from another `.tsra`
///   (store the object if absent + add a `BlockRef`). Composition, **not** encoding. The copy lands
///   **primary**, whatever its class in `SRC`, with one exception: see [`pair_travelling_sidecars`].
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
    let mut b = ProductBuilder::from_manifest(&parent)?;

    // Removes first — manifest edit only; the data objects are left for other versions / gc. A
    // block's derived family goes with it: an accelerator is a function of the block it was computed
    // from and bound to it by name only. A `.cidx` left behind would be inherited by the next
    // `--add-block …:<name>`, and `tessera stats` would serve the OLD block's statistics for the NEW
    // one, labelled exact (#347 review); pyramid levels left behind without level 0 would be the
    // product's only blocks, real data under the empty set's `content_hash` (#720 review).
    for name in removes {
        if b.remove_block_with_derived(name).is_none() {
            return Err(Error::Invalid(format!(
                "--remove-block: no block '{name}' in this version"
            )));
        }
    }

    // Adds — copy an already-encoded block from another .tsra (file i/o, no re-encode). A name the
    // version already holds (or that two adds share) is refused by the seal below, with
    // `Error::DuplicateBlockName`, before anything is stored: rename it, or --remove-block it first.
    let mut added = adds
        .iter()
        .map(|spec| load_block(spec))
        .collect::<Result<Vec<_>>>()?;
    pair_travelling_sidecars(&mut added);
    let mut payloads = Vec::new();
    for a in added {
        b.add_block_ref(a.block_ref);
        payloads.push(a.payload);
    }

    // Metadata sets. Each value is checked against the encoding and range its field declares in the
    // schema the new version carries (the parent's embedded one, else the registry's; #895), naming
    // the flag. The seal checks them again, as `metadata.<k>`.
    let schema = tessera_core::schema::declared_schema(&parent)?;
    for kv in sets {
        let (k, v) = kv
            .split_once('=')
            .ok_or_else(|| Error::Invalid(format!("--set expects key=value, got '{kv}'")))?;
        let flag = format!("--set {k}");
        let value = tessera_core::canonical::parse_user_value(&flag, v)?;
        if let Some(schema) = &schema {
            schema.check_value(k, &value, &flag)?;
        }
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

/// A block `--add-block` copies, with where it came from.
struct AddedBlock {
    /// The ref the new version gets: the source's kind, digest and spec, under the new name, and
    /// primary until [`pair_travelling_sidecars`] says otherwise.
    block_ref: tessera_core::block::BlockRef,
    payload: BlockPayload,
    /// The source `.tsra`, canonicalised, so two spellings of one file compare equal.
    source: std::path::PathBuf,
    /// The block's ref in the source product.
    source_ref: tessera_core::block::BlockRef,
}

/// Load an already-encoded block from another sealed `.tsra` for `--add-block`. `spec` is
/// `[NEWNAME=]SOURCE.tsra:SRCBLOCK`; the block's bytes (digest-verified on read) and its kind, digest
/// and spec are copied verbatim — no re-encoding.
///
/// The copy is **primary**, whatever its class in the source (ADR-0028 §4, #720). A derived block is
/// derived *of the product that computed it*: a pyramid level is a downsample of that product's
/// level 0, a `.cidx` an index of that product's block. In another product it is a function of
/// nothing there, so it is data the user supplies, and a block a user supplies is primary: it is part
/// of the new version's `content_hash`. There is no option to land a copy derived.
fn load_block(spec: &str) -> Result<AddedBlock> {
    let (newname, locator) = match spec.split_once('=') {
        Some((n, rest)) => (Some(n), rest),
        None => (None, spec),
    };
    let (path, srcblock) = locator.rsplit_once(':').ok_or_else(|| {
        Error::Invalid(format!(
            "--add-block expects [NAME=]SOURCE.tsra:BLOCK, got '{spec}'"
        ))
    })?;
    let newname = tessera_core::block::BlockName::new(newname.unwrap_or(srcblock))?;
    let mut r = Reader::open(Path::new(path))?;
    let src_ref = r
        .manifest()
        .blocks
        .iter()
        .find(|b| b.name == srcblock)
        .ok_or_else(|| Error::Invalid(format!("no block '{srcblock}' in {path}")))?
        .clone();
    let bytes = r.read_block(srcblock)?; // digest-verified on read
    let block_ref = tessera_core::block::BlockRef {
        name: newname.clone(),
        kind: src_ref.kind,
        digest: src_ref.digest.clone(),
        class: tessera_core::block::BlockClass::Primary,
        spec: src_ref.spec.clone(),
    };
    Ok(AddedBlock {
        block_ref,
        payload: BlockPayload::new(newname, bytes),
        source: std::fs::canonicalize(path)?,
        source_ref: src_ref,
    })
}

/// The one exception to "a copy lands primary": a chunk-index sidecar that travels **with the block
/// it indexes**, in the same `commit`, keeps its derived class. Exactly when all of these hold:
///
/// - in the source it is a derived `chunk_index` block whose spec says it `indexes` the block `D`;
/// - the same commit adds `D` from the same source file (the same file, canonicalised);
/// - neither is renamed: `D` lands as `D`, and the sidecar as [`cidx_name`]`(D)`, its name in the
///   source.
///
/// Then the new version holds byte-for-byte the pair the source sealed (each copy is digest-verified
/// on read), under the names that bind them (a sidecar is bound to its block by name, and its spec's
/// `indexes` still names the right block). So the sidecar is still the function of `D` the source's
/// writer computed: dropping and rebuilding it gives the same bytes, and leaving it out of
/// `content_hash` hides no data. Any other copied sidecar (alone, renamed, or with a `D` from another
/// file) indexes nothing it can be checked against here and stays primary.
///
/// [`cidx_name`]: tessera_core::block::cidx_name
fn pair_travelling_sidecars(added: &mut [AddedBlock]) {
    use tessera_core::block::{cidx_name, BlockClass, BlockKind};
    let travelling: Vec<usize> = (0..added.len())
        .filter(|&i| {
            let a = &added[i];
            let src = &a.source_ref;
            let Some(indexes) = src.spec.get("indexes").and_then(|v| v.as_str()) else {
                return false;
            };
            src.kind == BlockKind::ChunkIndex
                && src.class == BlockClass::Derived
                && src.name == cidx_name(indexes)
                && a.block_ref.name == src.name
                && added.iter().any(|d| {
                    d.source == a.source
                        && d.source_ref.name == indexes
                        && d.block_ref.name == indexes
                })
        })
        .collect();
    for i in travelling {
        added[i].block_ref.class = BlockClass::Derived;
    }
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

/// `tessera forget REPO LINEAGE` — delete a lineage's ref + log. Its now-unreachable objects are
/// reclaimed by a subsequent `gc` (shared blocks stay).
pub fn forget(repo: &Path, lineage: &str, out: &mut dyn Write) -> Result<()> {
    let repo = Repository::open(repo)?;
    if repo.forget(lineage)? {
        writeln!(out, "forgot lineage {lineage}").map_err(Error::from)?;
        writeln!(out, "  run `tessera gc` to reclaim now-unreachable objects")
            .map_err(Error::from)?;
    } else {
        writeln!(out, "no lineage '{lineage}' to forget").map_err(Error::from)?;
    }
    Ok(())
}

/// `tessera gc REPO` — reclaim objects unreachable from any ref.
pub fn gc(repo: &Path, out: &mut dyn Write) -> Result<()> {
    let repo = Repository::open(repo)?;
    let r = repo.gc()?;
    writeln!(
        out,
        "gc: scanned {} · kept {} · reclaimed {} ({} bytes)",
        r.scanned, r.kept, r.reclaimed, r.bytes_reclaimed
    )
    .map_err(Error::from)?;
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
    let mut b = ProductBuilder::from_manifest(&src)?;
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
    use crate::test_support::{assert_stored_pair, assert_tsra_stored_pair};
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
                ..Default::default()
            }],
            rows: 3,
            row_index: None,
            partition: None,
            edition: None,
            codec_profile: None,
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
                ..Default::default()
            }],
            rows: u64::try_from(vals.len()).unwrap(),
            row_index: None,
            partition: None,
            edition: None,
            codec_profile: None,
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

    /// `--set k=<integer beyond ±(2^53−1)>` is refused with the fix (SPEC §1, #886): the value is
    /// neither sealed approximately nor silently turned into a string, and no version is stored. The
    /// error names the key as typed and quotes the typed text (`…993.0`, not the double it parses to).
    #[test]
    fn commit_set_refuses_an_integer_a_seal_cannot_pin() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let tsra = dir.path().join("p.tsra");
        write_tsra(&tsra);
        init(&repo, &mut Vec::new()).unwrap();
        let mut buf = Vec::new();
        import(&repo, &tsra, &mut buf).unwrap();
        let out = String::from_utf8(buf).unwrap();
        let lineage = out
            .lines()
            .find_map(|l| l.strip_prefix("imported lineage "))
            .unwrap()
            .to_string();
        let before = count_objects(&repo);
        let set = |kv: &str| commit(&repo, &lineage, &[kv.into()], &[], &[], &mut Vec::new());
        let err = set("daq_start_ns=1730000000000000000").unwrap_err();
        assert_eq!(
            err.to_string(),
            "--set daq_start_ns = 1730000000000000000 is outside the range a sealed JSON number \
             can hold (±(2^53−1), I-JSON RFC 7493 §2.2): a sealed JSON number is an IEEE-754 \
             double (RFC 8785 §3.2.2.3), which can't hold this integer exactly; write it as a \
             string"
        );
        assert!(matches!(
            set("daq_start_ns=9007199254740993.0"),
            Err(Error::InexactInteger { path, value, .. })
                if path == "--set daq_start_ns" && value == "9007199254740993.0"
        ));
        assert_eq!(
            count_objects(&repo),
            before,
            "a refused commit stores nothing"
        );
        // The advice works: as a JSON string the value is sealed exactly.
        set(r#"daq_start_ns="1730000000000000000""#).unwrap();
        let r = Repository::open(&repo).unwrap();
        let m = r
            .get_manifest(&r.read_ref(&lineage).unwrap().unwrap())
            .unwrap();
        assert_eq!(
            m.metadata.get("daq_start_ns"),
            Some(&serde_json::json!("1730000000000000000"))
        );
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
        // `commit` stores the new version's manifest object as its stored bytes, which hash to the
        // object's address (ADR-0066: the address is the seal).
        let (alg, hex) = tip.split_once(':').unwrap();
        let object = repo
            .join("objects")
            .join(alg)
            .join(&hex[..2])
            .join(&hex[2..]);
        assert_stored_pair(
            "commit objects/<tip>",
            &std::fs::read(object).unwrap(),
            tip.as_bytes(),
        );

        // publish: history-free, with the snapshot_of breadcrumb; data travels + verifies.
        let pub_out = dir.path().join("published.tsra");
        publish(&repo, &tip, &pub_out, false, &mut Vec::new()).unwrap();
        let mut pr = Reader::open(&pub_out).unwrap();
        assert_tsra_stored_pair("publish", &pub_out);
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
        let ar = Reader::open(&anon_out).unwrap();
        assert_tsra_stored_pair("publish --anonymous", &anon_out);
        let am = ar.manifest().clone();
        assert!(
            !am.sources.iter().any(|s| s.role == "snapshot_of"),
            "anonymous drops the back-pointer"
        );

        // seal: the exact version, history preserved.
        let seal_out = dir.path().join("sealed.tsra");
        seal(&repo, &tip, &seal_out, &mut Vec::new()).unwrap();
        let sr = Reader::open(&seal_out).unwrap();
        assert_tsra_stored_pair("seal", &seal_out);
        let sm = sr.manifest().clone();
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

    /// Removing a block must take its chunk-index sidecar with it (#347 review).
    ///
    /// The `.cidx` is bound to its data block only by NAME. Leaving `volume.cidx` behind when
    /// `volume` is removed means the very next `--add-block <other>:volume` silently inherits it,
    /// and `tessera stats` would then serve the OLD volume's min/max/mean for the NEW block and
    /// label them `exact: true`. This is the exact command sequence from the review:
    ///
    /// ```text
    /// tessera commit --remove-block volume --add-block other.tsra:volume
    /// ```
    #[test]
    fn removing_a_block_also_removes_its_chunk_index_sidecar() {
        use tessera_core::block::array::ArraySpec;
        use tessera_io::array::ArrayData;

        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let p1 = dir.path().join("p1.tsra");
        let p2 = dir.path().join("p2.tsra");

        // p1: an indexed integer volume -> carries BOTH `volume` and `volume.cidx`.
        let mut spec = ArraySpec::new(vec![4], "int16");
        spec.chunks = vec![2];
        let ((br, pl), side) = tessera_io::array::array_block_with_index(
            "volume",
            &spec,
            &ArrayData::I16(vec![1, 2, 3, 4]),
        )
        .unwrap();
        let (sref, spl) = side.expect("integer array gets a sidecar");
        let mut b = PB::new("recon", "DP-idx", "v", "2024-01-01T00:00:00Z");
        b.add_block_ref(br);
        b.add_block_ref(sref);
        pack(&b.seal().unwrap(), &[pl, spl], &p1).unwrap();

        // p2: a DIFFERENT volume, same block name, no index.
        let (br2, pl2) = tessera_io::array::array_block(
            "volume",
            &spec,
            &ArrayData::I16(vec![100, 200, 300, 400]),
        )
        .unwrap();
        let mut b2 = PB::new("recon", "DP-other", "v", "2024-01-01T00:00:00Z");
        b2.add_block_ref(br2);
        pack(&b2.seal().unwrap(), &[pl2], &p2).unwrap();

        init(&repo, &mut Vec::new()).unwrap();
        let mut ibuf = Vec::new();
        import(&repo, &p1, &mut ibuf).unwrap();
        let lineage = String::from_utf8(ibuf)
            .unwrap()
            .lines()
            .find_map(|l| l.strip_prefix("imported lineage "))
            .unwrap()
            .to_string();

        let r = Repository::open(&repo).unwrap();
        let m0 = r
            .get_manifest(&r.read_ref(&lineage).unwrap().unwrap())
            .unwrap();
        assert!(
            m0.blocks.iter().any(|b| b.name == "volume.cidx"),
            "precondition: the imported version carries the sidecar"
        );

        // Swap the volume for a different one, in one commit.
        let add = format!("volume={}:volume", p2.to_str().unwrap());
        commit(
            &repo,
            &lineage,
            &[],
            &[add],
            &["volume".into()],
            &mut Vec::new(),
        )
        .unwrap();

        let m1 = r
            .get_manifest(&r.read_ref(&lineage).unwrap().unwrap())
            .unwrap();
        assert!(
            m1.blocks.iter().any(|b| b.name == "volume"),
            "the new volume is attached"
        );
        assert!(
            !m1.blocks.iter().any(|b| b.name == "volume.cidx"),
            "the STALE sidecar must not survive its block — it would describe the old data"
        );
    }

    /// Import `tsra` into a fresh repository at `repo`; its lineage.
    fn imported(repo: &Path, tsra: &Path) -> String {
        init(repo, &mut Vec::new()).unwrap();
        let mut out = Vec::new();
        import(repo, tsra, &mut out).unwrap();
        String::from_utf8(out)
            .unwrap()
            .lines()
            .find_map(|l| l.strip_prefix("imported lineage "))
            .unwrap()
            .to_string()
    }

    /// The tip manifest of `lineage`.
    fn tip(repo: &Path, lineage: &str) -> tessera_core::Manifest {
        let r = Repository::open(repo).unwrap();
        r.get_manifest(&r.read_ref(lineage).unwrap().unwrap())
            .unwrap()
    }

    /// An int16 `recon` volume of `shape` named `name`, sealed at `path`.
    fn write_volume(path: &Path, name: &str, offset: i16, shape: [u64; 3]) {
        use tessera_core::block::array::ArraySpec;
        use tessera_io::array::ArrayData;
        let spec = ArraySpec::new(shape.to_vec(), "int16");
        let n = shape.iter().product::<u64>() as usize;
        let data = ArrayData::I16((0..n).map(|k| (k % 1000) as i16 + offset).collect());
        let (br, pl) = tessera_io::array::array_block(name, &spec, &data).unwrap();
        let mut b = PB::new("recon", "DP-vol", "v", "2024-01-01T00:00:00Z");
        b.add_block_ref(br);
        pack(&b.seal().unwrap(), &[pl], path).unwrap();
    }

    /// #720 review: a derived block is derived of the product that computed it. Copied into another
    /// product with `--add-block` it is data the user supplies, so it lands primary and moves
    /// `content_hash` (a pyramid level kept derived would add data under an unchanged identity).
    #[test]
    fn add_block_lands_a_copied_derived_block_primary() {
        let dir = tempfile::tempdir().unwrap();
        let (src, pyr, repo) = (
            dir.path().join("src.tsra"),
            dir.path().join("pyr.tsra"),
            dir.path().join("repo"),
        );
        write_volume(&src, "volume", 0, [8, 8, 8]);
        crate::nav::build_pyramid(&src, "volume", None, &pyr).unwrap();
        let lvl1 = Reader::open(&pyr).unwrap().manifest().blocks[1].clone();
        assert!(
            !lvl1.is_primary(),
            "precondition: the source level is derived"
        );

        let lineage = imported(&repo, &src);
        let before = tip(&repo, &lineage);
        let add = format!("ct_overview={}:volume/1", pyr.to_str().unwrap());
        commit(&repo, &lineage, &[], &[add], &[], &mut Vec::new()).unwrap();
        let after = tip(&repo, &lineage);
        let copied = after
            .blocks
            .iter()
            .find(|b| b.name == "ct_overview")
            .expect("the copied level is attached");
        assert!(copied.is_primary(), "a copied derived block lands primary");
        assert_eq!(copied.digest, lvl1.digest, "its bytes are the source's");
        assert_ne!(
            after.content_hash, before.content_hash,
            "new data moves content_hash"
        );
    }

    /// The one exception: a `.cidx` that travels with the block it indexes, from the same file and
    /// under the same names, stays derived (it is still that block's index). Alone, renamed, or with
    /// a block of that name from another file, it lands primary.
    #[test]
    fn a_copied_sidecar_stays_derived_only_beside_the_block_it_indexes() {
        use tessera_core::block::array::ArraySpec;
        use tessera_core::block::BlockClass;
        use tessera_io::array::ArrayData;

        let dir = tempfile::tempdir().unwrap();
        let (src, other, base) = (
            dir.path().join("src.tsra"),
            dir.path().join("other.tsra"),
            dir.path().join("base.tsra"),
        );
        let mut spec = ArraySpec::new(vec![4], "int16");
        spec.chunks = vec![2];
        let ((br, pl), side) = tessera_io::array::array_block_with_index(
            "volume",
            &spec,
            &ArrayData::I16(vec![1, 2, 3, 4]),
        )
        .unwrap();
        let (sref, spl) = side.expect("integer array gets a sidecar");
        let mut b = PB::new("recon", "DP-idx", "v", "2024-01-01T00:00:00Z");
        b.add_block_ref(br);
        b.add_block_ref(sref);
        pack(&b.seal().unwrap(), &[pl, spl], &src).unwrap();
        write_volume(&other, "volume", 7, [8, 8, 8]);
        write_volume(&base, "base", 0, [8, 8, 8]);
        let (s, o) = (src.to_str().unwrap(), other.to_str().unwrap());

        // (adds, the class the `.cidx` copy must land with)
        let cases: [(Vec<String>, &str, BlockClass); 4] = [
            (
                vec![format!("{s}:volume"), format!("{s}:volume.cidx")],
                "volume.cidx",
                BlockClass::Derived,
            ),
            (
                vec![format!("{s}:volume.cidx")],
                "volume.cidx",
                BlockClass::Primary,
            ),
            (
                vec![format!("v2={s}:volume"), format!("v2.cidx={s}:volume.cidx")],
                "v2.cidx",
                BlockClass::Primary,
            ),
            (
                vec![format!("{o}:volume"), format!("{s}:volume.cidx")],
                "volume.cidx",
                BlockClass::Primary,
            ),
        ];
        for (i, (adds, cidx, want)) in cases.into_iter().enumerate() {
            let repo = dir.path().join(format!("repo{i}"));
            let lineage = imported(&repo, &base);
            commit(&repo, &lineage, &[], &adds, &[], &mut Vec::new()).unwrap();
            let m = tip(&repo, &lineage);
            let got = m.blocks.iter().find(|b| b.name == cidx).unwrap().class;
            assert_eq!(got, want, "{adds:?}");
            // Primary blocks only: `base` and the added data block, plus the sidecar if primary.
            let primary: Vec<String> = m
                .blocks
                .iter()
                .filter(|b| b.is_primary())
                .map(|b| b.digest.clone().unwrap())
                .collect();
            assert_eq!(
                m.content_hash.as_deref(),
                Some(tessera_core::hash::merkle_root(&primary).as_str())
            );
        }
    }

    /// #720 review: removing pyramid level 0 takes its derived levels with it, so the result is
    /// never downsamples alone under the empty set's `content_hash`; removing a derived level leaves
    /// level 0 and the other levels.
    #[test]
    fn removing_level_0_of_a_pyramid_removes_its_derived_levels() {
        let dir = tempfile::tempdir().unwrap();
        let (src, pyr) = (dir.path().join("src.tsra"), dir.path().join("pyr.tsra"));
        // 256×4×4 halves to 128×2×2 and 64×1×1: levels 0, 1 and 2.
        write_volume(&src, "volume", 0, [256, 4, 4]);
        crate::nav::build_pyramid(&src, "volume", None, &pyr).unwrap();
        let names_of = |m: &tessera_core::Manifest| -> Vec<String> {
            m.blocks.iter().map(|b| b.name.to_string()).collect()
        };
        let levels = names_of(Reader::open(&pyr).unwrap().manifest());
        assert_eq!(levels, ["volume/0", "volume/1", "volume/2"], "precondition");

        // A derived level goes alone.
        let repo = dir.path().join("repo-level");
        let lineage = imported(&repo, &pyr);
        commit(
            &repo,
            &lineage,
            &[],
            &[],
            &["volume/1".into()],
            &mut Vec::new(),
        )
        .unwrap();
        assert_eq!(names_of(&tip(&repo, &lineage)), ["volume/0", "volume/2"]);

        // Level 0 takes every derived level with it.
        let repo = dir.path().join("repo-base");
        let lineage = imported(&repo, &pyr);
        let base = tessera_core::block::pyramid_level_name("volume", 0);
        let add = format!("{}:volume", src.to_str().unwrap());
        commit(&repo, &lineage, &[], &[add], &[base], &mut Vec::new()).unwrap();
        let m = tip(&repo, &lineage);
        assert_eq!(names_of(&m), ["volume"]);
        assert_eq!(
            m.content_hash.as_deref(),
            Some(tessera_core::hash::merkle_root(&[m.blocks[0].digest.clone().unwrap()]).as_str())
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

    /// #734: `--add-block` of a name the version already holds is refused by the seal's uniqueness
    /// check, with the typed error, and leaves the repository as it was: no object, same tip.
    #[test]
    fn commit_add_block_refuses_a_name_already_taken() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let p1 = dir.path().join("p1.tsra");
        let p2 = dir.path().join("p2.tsra");
        write_tsra(&p1); // carries block "events"
        write_block_tsra(&p2, "events", &[9, 8]); // a different block, also named "events"
        init(&repo, &mut Vec::new()).unwrap();
        let mut ibuf = Vec::new();
        import(&repo, &p1, &mut ibuf).unwrap();
        let lineage = String::from_utf8(ibuf)
            .unwrap()
            .lines()
            .find_map(|l| l.strip_prefix("imported lineage "))
            .unwrap()
            .to_string();
        let r = Repository::open(&repo).unwrap();
        let tip = r.read_ref(&lineage).unwrap();
        let before = count_objects(&repo);

        let add = format!("{}:events", p2.to_str().unwrap());
        let e = commit(&repo, &lineage, &[], &[add], &[], &mut Vec::new()).unwrap_err();
        assert!(
            matches!(&e, Error::DuplicateBlockName { name } if name == "events"),
            "{e}"
        );
        assert_eq!(
            count_objects(&repo),
            before,
            "a refused commit stores nothing"
        );
        assert_eq!(r.read_ref(&lineage).unwrap(), tip, "the tip is unchanged");
    }
}
