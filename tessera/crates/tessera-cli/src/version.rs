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
/// A lineage whose tip is another version is refused ([`Error::RefMoved`]): importing over it would
/// replace its tip with a version whose history does not reach the old one. Use `commit` to change
/// it, or `forget` it first. Importing the version that is already the tip changes no ref and
/// succeeds; its objects are written again where one is missing or torn (#946).
pub fn import(repo: &Path, tsra: &Path, out: &mut dyn Write) -> Result<()> {
    let repo = Repository::open(repo)?;
    let mut r = Reader::open(tsra)?;
    let manifest = r.manifest().clone();
    let mut payloads = Vec::with_capacity(manifest.blocks.len());
    for b in &manifest.blocks {
        let bytes = r.read_block(&b.name)?;
        payloads.push(BlockPayload::new(b.name.clone(), bytes));
    }
    let mh = match repo.commit(&manifest.id, None, &manifest, &payloads) {
        Err(Error::RefMoved {
            found: Some(tip), ..
        }) if manifest.manifest_hash.as_deref() == Some(tip.as_str()) => {
            for p in &payloads {
                let digest = manifest
                    .blocks
                    .iter()
                    .find(|b| b.name == p.name)
                    .and_then(|b| b.digest.as_deref())
                    .ok_or_else(|| Error::MissingDigest(p.name.to_string()))?;
                repo.write_object(digest, &p.bytes)?;
            }
            repo.put_manifest(&manifest)?
        }
        other => other?,
    };
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
///   **primary**, whatever its class in `SRC`. A derived block is never copied: when `SRC` carried
///   an index for `BLK`, `commit` rebuilds `N.cidx` from the copied bytes and lands that derived
///   ([`rebuild_indexes`]).
///
/// Encoding *raw* data into a new block (array→zarr/pcodec, table→Vortex) is `ingest`/`pack`'s job —
/// the "extra handling" tier, deliberately kept out of the versioning layer. The one decode here is
/// the index rebuild, which runs the writers' own index functions over the copied block.
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
    // from and bound to it by name only. A derived `.cidx` left behind would be inherited by the next
    // `--add-block …:<name>`, and `tessera stats` would serve the OLD block's statistics for the NEW
    // one, labelled exact (#347 review); pyramid levels left behind without level 0 would be the
    // product's only blocks, real data under the empty set's `content_hash` (#720 review). A primary
    // block of one of those names is data of its own: the cascade never deletes supplied data. A
    // chunk index of a removed block that the cascade doesn't take (a primary one, or one under
    // another name, which only a hand-built product holds) would dangle, a defect every reader
    // refuses (`Error::DanglingChunkIndex`), so the remove is refused, naming the index to remove too.
    let mut removed_derived = 0;
    for name in removes {
        let gone = b.remove_block_with_derived(name).ok_or_else(|| {
            Error::Invalid(format!("--remove-block: no block '{name}' in this version"))
        })?;
        removed_derived += gone.len() - 1;
    }
    refuse_orphaned_indexes(&parent.blocks, removes)?;

    // Adds — copy an already-encoded block from another .tsra (file i/o, no re-encode). A name the
    // version already holds (or that two adds share) is refused by the seal below, with
    // `Error::DuplicateBlockName`, before anything is stored: rename it, or --remove-block it first.
    let added = adds
        .iter()
        .map(|spec| load_block(spec))
        .collect::<Result<Vec<_>>>()?;
    let rebuilt = rebuild_indexes(&added)?;
    let added_derived = rebuilt.len();
    let mut payloads = Vec::new();
    for (block_ref, payload) in added
        .into_iter()
        .map(|a| (a.block_ref, a.payload))
        .chain(rebuilt)
    {
        b.add_block_ref(block_ref);
        payloads.push(payload);
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
    // commit stores the new block payloads content-addressed (dedup if already present) + the
    // manifest, and advances the ref only if it is still at `tip`, the version this one was built on
    // (ADR-0036 §2): a commit that landed since is never overwritten, the new version is refused.
    let mh = repo.commit(lineage, Some(&tip), &new_manifest, &payloads)?;

    writeln!(out, "committed {lineage}").map_err(Error::from)?;
    writeln!(out, "  version    {mh}").map_err(Error::from)?;
    writeln!(out, "  supersedes {tip}").map_err(Error::from)?;
    if !payloads.is_empty() || !removes.is_empty() {
        writeln!(
            out,
            "  blocks     +{} -{}",
            block_count(payloads.len() - added_derived, added_derived),
            block_count(removes.len(), removed_derived),
        )
        .map_err(Error::from)?;
    }
    Ok(())
}

/// `N`, or `N (+D derived)` when `D` derived blocks went along: the commit summary's block counts.
fn block_count(named: usize, derived: usize) -> String {
    if derived == 0 {
        named.to_string()
    } else {
        format!("{named} (+{derived} derived)")
    }
}

/// A block `--add-block` copies.
struct AddedBlock {
    /// The ref the new version gets: the source's kind, digest and spec, under the new name, and
    /// primary.
    block_ref: tessera_core::block::BlockRef,
    payload: BlockPayload,
    /// The source carried an index for this block (a block named [`cidx_name`] of its source name),
    /// so the copy gets one too: rebuilt, never copied ([`rebuild_indexes`]).
    ///
    /// [`cidx_name`]: tessera_core::block::cidx_name
    source_indexed: bool,
}

/// Load an already-encoded block from another sealed `.tsra` for `--add-block`. `spec` is
/// `[NEWNAME=]SOURCE.tsra:SRCBLOCK`; the block's bytes and its kind, digest and spec are copied
/// verbatim — no re-encoding. SOURCE is verified first exactly as `tessera verify` does it (its
/// container layout and every payload), so a product `verify` rejects is never read from.
///
/// The copy is **primary**, whatever its class in the source (ADR-0028 §4, #720). A derived block is
/// derived *of the product that computed it*: a pyramid level is a downsample of that product's
/// level 0. In another product it is a function of nothing there, so it is data the user supplies,
/// and a block a user supplies is primary: it is part of the new version's `content_hash`. There is
/// no option to land a copy derived.
///
/// A **chunk index** is refused, whatever it is called and whatever its class
/// ([`BlockRef::is_chunk_index`]): its statistics are a claim about a block that only a name binds
/// it to, so it is rebuilt from the data and never copied. Copying the data block from a source that
/// indexes it gets the index ([`rebuild_indexes`]).
///
/// [`BlockRef::is_chunk_index`]: tessera_core::block::BlockRef::is_chunk_index
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
    if src_ref.is_chunk_index() {
        let data = src_ref
            .spec
            .get("indexes")
            .and_then(|v| v.as_str())
            .map_or_else(|| "its data block".to_string(), |d| format!("'{d}'"));
        return Err(Error::Invalid(format!(
            "--add-block: '{srcblock}' in {path} is a chunk index. Chunk indexes are rebuilt from \
             the data and never copied: copy {data} from a source that indexes it (commit rebuilds \
             its index), or re-write the product"
        )));
    }
    // The same strictness as `tessera verify`, before a byte is taken from the source.
    r.verify_payloads(path)?;
    let bytes = r.read_block(srcblock)?; // digest-verified on read
    let index_name = tessera_core::block::cidx_name(srcblock);
    let source_indexed = r.manifest().blocks.iter().any(|b| b.name == index_name);
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
        source_indexed,
    })
}

/// Refuse a `--remove-block` that would leave a chunk index without its block: a block of the
/// parent that is a chunk index ([`tessera_core::block::BlockRef::is_chunk_index`]), indexes one of
/// `removes`, and is not itself removed (by the derived cascade or its own `--remove-block`). The
/// cascade takes only derived blocks, so this is a primary look-alike or an index under another
/// name; the error names it and the flag that removes it.
fn refuse_orphaned_indexes(
    parent: &[tessera_core::block::BlockRef],
    removes: &[String],
) -> Result<()> {
    let gone: Vec<&str> = removes
        .iter()
        .map(String::as_str)
        .chain(
            removes
                .iter()
                .flat_map(|name| tessera_core::block::derived_family(name, parent))
                .map(|n| n.as_str()),
        )
        .collect();
    for index in parent
        .iter()
        .filter(|b| b.is_chunk_index() && !gone.contains(&b.name.as_str()))
    {
        let Some(data) = index.spec.get("indexes").and_then(|v| v.as_str()) else {
            continue;
        };
        if removes.iter().any(|r| r == data) {
            return Err(Error::Invalid(format!(
                "--remove-block {data}: '{index}' is a chunk index of '{data}' that the cascade \
                 does not remove (it takes only the derived '{data}.cidx'; supplied data is never \
                 deleted implicitly), and a chunk index without its block is refused everywhere \
                 (#720): remove it too, with --remove-block {index}",
                index = index.name
            )));
        }
    }
    Ok(())
}

/// The index of each copied block whose source carried one, **rebuilt from the copied bytes** and
/// landed derived (ADR-0028 §4, #720).
///
/// A derived block is computed by the writer that marks it derived; it is never copied. A source's
/// `volume.cidx` is bound to its `volume` by name only (its spec records `indexes: "volume"`, not
/// that block's digest), so its bytes prove nothing about the block they claim to describe: a crafted
/// product can carry an index whose min/max are not the data's, and `tessera stats` would serve
/// them as exact. So the source's index bytes are never read. The index is recomputed by
/// [`tessera_io::chunk_index::rebuild_chunk_index_block`], with the same functions the product
/// writers use, under the name the block lands with (`ct=SRC:volume` gets `ct.cidx`). It is byte for
/// byte the index a writer would emit for that block, so it may stay out of `content_hash`.
///
/// A block the writer would not index (a float array, a table without an integer `row_index`) gets
/// none. A rebuilt index's name may not also be claimed by an explicit `--add-block` (no index, which
/// [`load_block`] refuses, but a block copied under that name): that is refused, naming the clash.
fn rebuild_indexes(
    added: &[AddedBlock],
) -> Result<Vec<(tessera_core::block::BlockRef, BlockPayload)>> {
    let mut rebuilt = Vec::new();
    for a in added.iter().filter(|a| a.source_indexed) {
        let Some((index_ref, payload)) =
            tessera_io::chunk_index::rebuild_chunk_index_block(&a.block_ref, &a.payload.bytes)?
        else {
            continue;
        };
        if added.iter().any(|o| o.block_ref.name == index_ref.name) {
            return Err(Error::Invalid(format!(
                "--add-block: '{0}' is the name of the index commit rebuilds for '{1}' from its \
                 bytes; land the other copy under another name",
                index_ref.name, a.block_ref.name
            )));
        }
        rebuilt.push((index_ref, payload));
    }
    Ok(rebuilt)
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

    /// #946: `import` starts a lineage. Importing the tip's own version again is a no-op that
    /// repairs its objects; importing into a lineage whose tip is another version is refused with
    /// `RefMoved` (it replaced the tip with a version whose history skips the old one, which `gc`
    /// then reclaimed), and the tip stays.
    #[test]
    fn import_into_a_lineage_with_a_version_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let tsra = dir.path().join("p.tsra");
        write_tsra(&tsra);
        let lineage = imported(&repo, &tsra);
        // The same version again: no ref moves, and a torn block object is written again.
        let r = Repository::open(&repo).unwrap();
        let v0 = r.read_ref(&lineage).unwrap().unwrap();
        let block = r.get_manifest(&v0).unwrap().blocks[0]
            .digest
            .clone()
            .unwrap();
        let hex = block.strip_prefix("blake3:").unwrap();
        let object = repo.join("objects/blake3").join(&hex[..2]).join(&hex[2..]);
        std::fs::write(&object, b"torn").unwrap();
        import(&repo, &tsra, &mut Vec::new()).unwrap();
        assert_eq!(r.read_ref(&lineage).unwrap().as_deref(), Some(v0.as_str()));
        r.read_object(&block)
            .expect("re-import repairs the torn block");

        commit(
            &repo,
            &lineage,
            &["tracer=FLT".into()],
            &[],
            &[],
            &mut Vec::new(),
        )
        .unwrap();
        let tip = r.read_ref(&lineage).unwrap();

        let e = import(&repo, &tsra, &mut Vec::new()).unwrap_err();
        assert!(
            matches!(&e, Error::RefMoved { expected: None, found, .. } if *found == tip),
            "{e}"
        );
        assert_eq!(r.read_ref(&lineage).unwrap(), tip, "the tip is unchanged");
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

    /// #975: a repository `import` laid out before #975 (ref `refs/blake3_<hex>`, no layout marker)
    /// is refused by every verb, never read as a lineage without commits; the error names the
    /// layout it found and the one it needs, and says to import the source products again.
    #[test]
    fn a_pre_975_repository_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let tsra = dir.path().join("p.tsra");
        write_tsra(&tsra);
        let lineage = imported(&repo, &tsra);
        // The old layout: the ref and log under the old names, no marker.
        let new = tessera_io::lineage_file_name(&lineage).unwrap();
        let old = lineage.replace([':', '/', '\\'], "_");
        std::fs::rename(repo.join("refs").join(&new), repo.join("refs").join(&old)).unwrap();
        std::fs::rename(
            repo.join("log").join(format!("{new}.jsonl")),
            repo.join("log").join(format!("{old}.jsonl")),
        )
        .unwrap();
        std::fs::remove_file(repo.join("layout")).unwrap();

        let refused = |r: Result<()>| {
            let e = r.unwrap_err();
            let text = e.to_string();
            assert!(
                matches!(e, Error::UnsupportedRepositoryLayout { .. })
                    && text.contains("before #975")
                    && text.contains("tessera-repository 1")
                    && text.contains("import the source products again"),
                "{text}"
            );
        };
        refused(log(&repo, &lineage, &mut Vec::new()));
        refused(commit(
            &repo,
            &lineage,
            &["k=1".into()],
            &[],
            &[],
            &mut Vec::new(),
        ));
        refused(import(&repo, &tsra, &mut Vec::new()));
        refused(init(&repo, &mut Vec::new()));
        refused(forget(&repo, &lineage, &mut Vec::new()));
        refused(gc(&repo, &mut Vec::new()));
        assert!(repo.join("refs").join(&old).exists(), "nothing was removed");
        assert!(!repo.join("layout").exists(), "nothing was marked");
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

    /// `stats` of `block` in the sealed file at `path`, as JSON.
    fn stats_of(path: &Path, block: &str) -> serde_json::Value {
        let mut buf = Vec::new();
        crate::nav::stats(path, block, true, &mut buf).unwrap();
        serde_json::from_slice(&buf).unwrap()
    }

    /// Seal the tip of `lineage` to `path`.
    fn seal_tip(repo: &Path, lineage: &str, path: &Path) {
        let mh = tip(repo, lineage).manifest_hash.unwrap();
        seal(repo, &mh, path, &mut Vec::new()).unwrap();
    }

    /// `content_hash` is over the primary blocks only, in order.
    fn assert_primary_content_hash(m: &tessera_core::Manifest) {
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

    /// #720 rounds 3 and 4: a derived block is computed by the writer that marks it derived, never
    /// copied. Copying a block whose source carried an index rebuilds the index from the copied bytes
    /// (byte for byte the writer's own, under the name the block lands with) and lands it derived. A
    /// copy of the index itself is refused, under any name: alone, renamed
    /// (`other.cidx=SRC:volume.cidx`), or beside its block, and the lineage does not move.
    #[test]
    fn copying_a_block_rebuilds_its_index_and_never_copies_it() {
        use tessera_core::block::array::ArraySpec;
        use tessera_core::block::BlockClass;
        use tessera_io::array::ArrayData;

        let dir = tempfile::tempdir().unwrap();
        let (src, plain, base) = (
            dir.path().join("src.tsra"),
            dir.path().join("plain.tsra"),
            dir.path().join("base.tsra"),
        );
        // An honest indexed source: eight 4³ chunks over an 8³ int16 volume.
        let mut spec = ArraySpec::new(vec![8, 8, 8], "int16");
        spec.chunks = vec![4, 4, 4];
        let data = ArrayData::I16((0..512).map(|k| k as i16 - 256).collect());
        let ((br, pl), side) =
            tessera_io::array::array_block_with_index("volume", &spec, &data).unwrap();
        let (sref, spl) = side.expect("integer array gets a sidecar");
        let mut b = PB::new("recon", "DP-idx", "v", "2024-01-01T00:00:00Z");
        b.add_block_ref(br);
        b.add_block_ref(sref.clone());
        pack(&b.seal().unwrap(), &[pl, spl], &src).unwrap();
        write_volume(&plain, "volume", 7, [8, 8, 8]);
        write_volume(&base, "base", 0, [8, 8, 8]);
        let (s, o) = (src.to_str().unwrap(), plain.to_str().unwrap());

        // The index a writer builds fresh for this data, under a given name.
        let fresh = |name: &str| {
            let idx = tessera_io::array::array_chunk_index(&spec, &data).unwrap();
            tessera_io::chunk_index::chunk_index_block(name, &idx)
                .unwrap()
                .0
        };

        // (adds, then each `.cidx` the tip must hold: (name, class, the rebuilt ref it must equal))
        type Want = Vec<(&'static str, BlockClass, tessera_core::block::BlockRef)>;
        let cases: Vec<(Vec<String>, Want)> = vec![
            // The data block alone: its index is rebuilt, byte-identical to a fresh build.
            (
                vec![format!("{s}:volume")],
                vec![("volume.cidx", BlockClass::Derived, fresh("volume"))],
            ),
            // Renamed: the rebuilt index follows the name the block lands with.
            (
                vec![format!("ct={s}:volume")],
                vec![("ct.cidx", BlockClass::Derived, fresh("ct"))],
            ),
            // A source with no index: none is invented.
            (vec![format!("{o}:volume")], vec![]),
        ];
        // Each copy of the index is refused (round 3 landed these primary; round 4 refuses them).
        let refused = [
            // The `.cidx` alone.
            vec![format!("{s}:volume.cidx")],
            // Only the `.cidx` renamed.
            vec![format!("other.cidx={s}:volume.cidx")],
            // The block, and its index under another name.
            vec![format!("{s}:volume"), format!("other.cidx={s}:volume.cidx")],
        ];
        for (i, adds) in refused.iter().enumerate() {
            let repo = dir.path().join(format!("refused{i}"));
            let lineage = imported(&repo, &base);
            let before = tip(&repo, &lineage).manifest_hash;
            let err = commit(&repo, &lineage, &[], adds, &[], &mut Vec::new()).unwrap_err();
            assert!(err.to_string().contains("never copied"), "{adds:?}: {err}");
            assert_eq!(tip(&repo, &lineage).manifest_hash, before, "{adds:?}");
        }
        for (i, (adds, want)) in cases.into_iter().enumerate() {
            let repo = dir.path().join(format!("repo{i}"));
            let lineage = imported(&repo, &base);
            commit(&repo, &lineage, &[], &adds, &[], &mut Vec::new()).unwrap();
            let m = tip(&repo, &lineage);
            let got: Vec<_> = m
                .blocks
                .iter()
                .filter(|b| b.name.ends_with(".cidx"))
                .collect();
            assert_eq!(got.len(), want.len(), "{adds:?}: {got:?}");
            for (name, class, rebuilt) in &want {
                let b = got.iter().find(|b| b.name == *name).unwrap();
                assert_eq!(b.class, *class, "{adds:?}: {name}");
                assert_eq!(*b, rebuilt, "{adds:?}: {name} is the writer's own index");
            }
            assert_primary_content_hash(&m);
        }
    }

    /// A 16³ int16 `volume` whose values span -1024..=3071, beside a forged chunk index of it that
    /// claims min = max = 7, stored as `index_name` with `class`, sealed at `path`. The forgery has
    /// the right kind, spec and entry count (one 64³ chunk covers the volume), so only the data can
    /// tell it is false.
    fn write_lying(path: &Path, index_name: &str, class: tessera_core::block::BlockClass) {
        use tessera_core::block::array::ArraySpec;
        use tessera_io::array::ArrayData;
        let spec = ArraySpec::new(vec![16, 16, 16], "int16");
        let data = ArrayData::I16((0..4096).map(|k| k as i16 - 1024).collect());
        let (br, pl) = tessera_io::array::array_block("volume", &spec, &data).unwrap();
        let mut forged = tessera_core::chunk_index::ChunkIndex::new();
        forged.push(tessera_core::hash::digest(b"forged"), &[7; 4096]);
        let (mut fref, fpl) =
            tessera_io::chunk_index::chunk_index_block("volume", &forged).unwrap();
        fref.name = tessera_core::block::BlockName::new(index_name).unwrap();
        fref.class = class;
        let fpl = BlockPayload::new(fref.name.clone(), fpl.bytes);
        let mut b = PB::new("recon", "DP-lie", "v", "2024-01-01T00:00:00Z");
        b.add_block_ref(br);
        b.add_block_ref(fref);
        pack(&b.seal().unwrap(), &[pl, fpl], path).unwrap();
    }

    /// `stats` of `volume` at `path` reports the data's true range, -1024..=3071, by `method`.
    fn assert_true_range(path: &Path, method: &str) {
        let v = stats_of(path, "volume");
        assert_eq!(v["method"], method, "{v}");
        assert_eq!(v["raw"]["min"].as_f64(), Some(-1024.0), "{v}");
        assert_eq!(v["raw"]["max"].as_f64(), Some(3071.0), "{v}");
    }

    /// #720 round 3, the lying-index attack. A crafted int16 product carries a derived `volume.cidx`
    /// claiming min = max = 7 for data that spans -1024..=3071. It verifies, and `stats` believes it.
    /// Copying `volume` out of it must never carry that index along: the commit rebuilds the index
    /// from the data, so `stats` reports the real range, and an explicit copy of the lying index is
    /// refused.
    #[test]
    fn a_lying_index_never_travels_with_the_block_it_lies_about() {
        let dir = tempfile::tempdir().unwrap();
        let (lying, base) = (dir.path().join("lying.tsra"), dir.path().join("base.tsra"));
        write_lying(
            &lying,
            "volume.cidx",
            tessera_core::block::BlockClass::Derived,
        );
        write_volume(&base, "base", 0, [8, 8, 8]);

        // Precondition: the attack is real. The product verifies and `stats` serves the lie.
        Reader::open(&lying)
            .unwrap()
            .verify_payloads("lying")
            .unwrap();
        let v = stats_of(&lying, "volume");
        assert_eq!(v["method"], "chunk-index-aggregate", "precondition");
        assert_eq!(v["raw"]["min"].as_f64(), Some(7.0), "precondition");

        let l = lying.to_str().unwrap();
        // The attack: the block and its lying index together. Refused, the lineage unchanged.
        let repo = dir.path().join("repo-attack");
        let lineage = imported(&repo, &base);
        let before = tip(&repo, &lineage);
        let adds = [format!("{l}:volume"), format!("{l}:volume.cidx")];
        let err = commit(&repo, &lineage, &[], &adds, &[], &mut Vec::new()).unwrap_err();
        assert!(err.to_string().contains("never copied"), "{err}");
        assert_eq!(tip(&repo, &lineage).manifest_hash, before.manifest_hash);

        // The block alone: its index is rebuilt from the data, and `stats` reports the real range.
        let repo = dir.path().join("repo-copy");
        let lineage = imported(&repo, &base);
        commit(
            &repo,
            &lineage,
            &[],
            &[format!("{l}:volume")],
            &[],
            &mut Vec::new(),
        )
        .unwrap();
        let out = dir.path().join("out.tsra");
        seal_tip(&repo, &lineage, &out);
        // The rebuilt index is used, and it is true.
        assert_true_range(&out, "chunk-index-aggregate");
    }

    /// #720 round 4, the reviewer's attack: the data block from a source with NO index (so nothing
    /// is rebuilt) beside the lying index, in one commit or two. Every copy of the index is refused,
    /// whatever it is called and whatever its class in the source, and no version is written. Only
    /// the data block lands, and `stats` computes its true range from the data.
    #[test]
    fn a_chunk_index_is_never_copied_under_any_name_or_class() {
        use tessera_core::block::BlockClass;
        let dir = tempfile::tempdir().unwrap();
        let (plain, base) = (dir.path().join("plain.tsra"), dir.path().join("base.tsra"));
        // `plain` holds the same volume, unindexed: the lying product minus its index.
        {
            let lying = dir.path().join("lying-for-plain.tsra");
            write_lying(&lying, "volume.cidx", BlockClass::Derived);
            let mut r = Reader::open(&lying).unwrap();
            let vref = r.manifest().blocks[0].clone();
            let bytes = r.read_block("volume").unwrap();
            let mut b = PB::new("recon", "DP-plain", "v", "2024-01-01T00:00:00Z");
            b.add_block_ref(vref);
            pack(
                &b.seal().unwrap(),
                &[BlockPayload::new("volume".parse().unwrap(), bytes)],
                &plain,
            )
            .unwrap();
        }
        write_volume(&base, "base", 0, [8, 8, 8]);
        let p = plain.to_str().unwrap();
        // The forged index under `.cidx` and non-`.cidx` names, derived and primary.
        let mut sources = Vec::new();
        for (i, (name, class)) in [
            ("volume.cidx", BlockClass::Derived),
            ("volume.cidx", BlockClass::Primary),
            ("notes", BlockClass::Derived),
            ("notes", BlockClass::Primary),
        ]
        .into_iter()
        .enumerate()
        {
            let path = dir.path().join(format!("lying{i}.tsra"));
            write_lying(&path, name, class);
            sources.push(format!("{}:{name}", path.to_str().unwrap()));
        }
        let renamed = format!("volume.cidx={}", sources[2]);
        let attempts: Vec<Vec<String>> = sources
            .iter()
            .flat_map(|l| [vec![format!("{p}:volume"), l.clone()], vec![l.clone()]])
            .chain([vec![format!("{p}:volume"), renamed]])
            .collect();
        for (i, adds) in attempts.iter().enumerate() {
            let repo = dir.path().join(format!("repo{i}"));
            let lineage = imported(&repo, &base);
            let before = tip(&repo, &lineage).manifest_hash;
            let err = commit(&repo, &lineage, &[], adds, &[], &mut Vec::new()).unwrap_err();
            assert!(err.to_string().contains("never copied"), "{adds:?}: {err}");
            assert_eq!(tip(&repo, &lineage).manifest_hash, before, "{adds:?}");
        }

        // As two commits: the index first is refused; the data block then lands alone.
        let repo = dir.path().join("repo-two");
        let lineage = imported(&repo, &base);
        let before = tip(&repo, &lineage).manifest_hash;
        assert!(commit(
            &repo,
            &lineage,
            &[],
            &[sources[0].clone()],
            &[],
            &mut Vec::new()
        )
        .is_err());
        assert_eq!(tip(&repo, &lineage).manifest_hash, before);
        commit(
            &repo,
            &lineage,
            &[],
            &[format!("{p}:volume")],
            &[],
            &mut Vec::new(),
        )
        .unwrap();
        let m = tip(&repo, &lineage);
        let names: Vec<&str> = m.blocks.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, ["base", "volume"], "only the data block landed");
        let out = dir.path().join("out.tsra");
        seal_tip(&repo, &lineage, &out);
        assert_true_range(&out, "full-decode-exact-integer");
    }

    /// #720 round 4: a product written with a primary chunk index (no commit involved, the writer
    /// builds it directly) verifies, but `stats` never takes aggregates from it: a primary block is
    /// data someone supplied, not the writer's index, so `stats` computes from the data, as it does
    /// for an unindexed block. The same forgery derived is served, which is what makes it the test.
    #[test]
    fn stats_never_trusts_a_primary_chunk_index() {
        use tessera_core::block::BlockClass;
        let dir = tempfile::tempdir().unwrap();
        let (primary, derived) = (dir.path().join("p.tsra"), dir.path().join("d.tsra"));
        write_lying(&primary, "volume.cidx", BlockClass::Primary);
        write_lying(&derived, "volume.cidx", BlockClass::Derived);
        Reader::open(&primary)
            .unwrap()
            .verify_payloads("primary")
            .unwrap();
        assert_eq!(
            stats_of(&derived, "volume")["raw"]["min"].as_f64(),
            Some(7.0),
            "precondition: a derived forgery is served"
        );
        assert_true_range(&primary, "full-decode-exact-integer");
    }

    /// #720 round 4 nit: `--remove-block volume` cascades only to DERIVED blocks. A primary
    /// `volume.cidx` (only a hand-built product carries one) is data of its own, never deleted
    /// implicitly; round 5: left behind it would be an index without its block, so the remove is
    /// refused with a hint, and removing both by name works.
    #[test]
    fn removal_never_cascades_to_a_primary_index() {
        use tessera_core::block::BlockClass;
        let dir = tempfile::tempdir().unwrap();
        let (src, repo) = (dir.path().join("p.tsra"), dir.path().join("repo"));
        write_lying(&src, "volume.cidx", BlockClass::Primary);
        let lineage = imported(&repo, &src);
        let before = tip(&repo, &lineage).manifest_hash;
        // #720 round 5: the primary `volume.cidx` stays, so removing `volume` alone would leave an
        // index without its block. Refused, naming the index and the flag that removes it.
        let err = commit(
            &repo,
            &lineage,
            &[],
            &[],
            &["volume".into()],
            &mut Vec::new(),
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("'volume.cidx' is a chunk index of 'volume'")
                && err.contains("--remove-block volume.cidx"),
            "{err}"
        );
        assert_eq!(tip(&repo, &lineage).manifest_hash, before);
        // Removing both, each by name, is two primary removals.
        let mut out = Vec::new();
        let both = ["volume".to_string(), "volume.cidx".to_string()];
        commit(&repo, &lineage, &[], &[], &both, &mut out).unwrap();
        let out = String::from_utf8(out).unwrap();
        assert!(out.contains("  blocks     +0 -2\n"), "{out}");
        assert!(tip(&repo, &lineage).blocks.is_empty());
    }

    /// The forged chunk index of #720 round 5 for a block `ct` (min = max = 7, one chunk), derived,
    /// in a product beside `base`. `with_ct` keeps the 16³ int16 block `ct` the index names; without
    /// it the block list is resealed by hand, as a hostile producer would (every writer refuses a
    /// dangling index), into the canonical archive tessera's writer emits. Returns the manifest.
    fn write_orphan(path: &Path, with_ct: bool) -> tessera_core::Manifest {
        use std::io::Write as _;
        use tessera_core::block::array::ArraySpec;
        use tessera_io::array::ArrayData;
        let spec = ArraySpec::new(vec![16, 16, 16], "int16");
        let data = ArrayData::I16((0..4096).map(|k| k as i16 - 1024).collect());
        let (base, base_pl) = tessera_io::array::array_block("base", &spec, &data).unwrap();
        let (ct, ct_pl) = tessera_io::array::array_block("ct", &spec, &data).unwrap();
        let mut forged = tessera_core::chunk_index::ChunkIndex::new();
        forged.push(tessera_core::hash::digest(b"forged"), &[7; 4096]);
        let (cidx, cidx_pl) = tessera_io::chunk_index::chunk_index_block("ct", &forged).unwrap();
        let mut b = PB::new("recon", "DP-orphan", "v", "2024-01-01T00:00:00Z");
        b.add_block_ref(base);
        b.add_block_ref(ct);
        b.add_block_ref(cidx.derived());
        let mut m = b.seal().unwrap();
        if with_ct {
            pack(&m, &[base_pl, ct_pl, cidx_pl], path).unwrap();
            return m;
        }
        m.blocks.retain(|b| b.name != "ct");
        m.content_hash = Some(m.recompute_content_hash().unwrap());
        let json = m.canonical_bytes().unwrap();
        let seal = tessera_core::hash::digest(&json);
        m.manifest_hash = Some(seal.clone());
        let o = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored)
            .last_modified_time(zip::DateTime::default())
            .large_file(true)
            .system(zip::System::Unix);
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, bytes) in [
            (
                "mimetype".to_string(),
                tessera_io::MIMETYPE.as_bytes().to_vec(),
            ),
            ("manifest.seal".into(), seal.into_bytes()),
            ("manifest.json".into(), json),
            ("blocks/base".into(), base_pl.bytes),
            ("blocks/ct.cidx".into(), cidx_pl.bytes),
        ] {
            w.start_file(name, o).unwrap();
            w.write_all(&bytes).unwrap();
        }
        std::fs::write(path, w.finish().unwrap().into_inner()).unwrap();
        m
    }

    /// The refusal of a dangling index, matched by its message (#720 round 5).
    fn is_dangling(e: &Error) -> bool {
        e.to_string()
            .contains("chunk index 'ct.cidx' indexes 'ct', which is not another block")
    }

    /// #720 round 5, the reviewer's attack: `orphan.tsra` holds `base` and a derived `ct.cidx` that
    /// lies (min = max = 7) about a block `ct` the product doesn't hold. Imported, then given an
    /// honest unindexed `ct`, the version inherited the lie: no copy, no rebuild, and `stats ct`
    /// served it as exact. Now the sealed orphan is refused at open, so `verify` and `import`
    /// refuse it and no lineage is written.
    #[test]
    fn an_orphan_chunk_index_is_refused_at_open_and_import() {
        let dir = tempfile::tempdir().unwrap();
        let (orphan, repo) = (dir.path().join("orphan.tsra"), dir.path().join("repo"));
        let m = write_orphan(&orphan, false);
        let e = Reader::open(&orphan)
            .err()
            .expect("open refuses the orphan index");
        assert!(is_dangling(&e), "{e}");
        let e = tessera_io::verify_payloads_parallel(&orphan, "orphan", 2).unwrap_err();
        assert!(is_dangling(&e), "{e}");

        init(&repo, &mut Vec::new()).unwrap();
        let e = import(&repo, &orphan, &mut Vec::new()).unwrap_err();
        assert!(is_dangling(&e), "{e}");
        let r = Repository::open(&repo).unwrap();
        assert_eq!(r.read_ref(&m.id).unwrap(), None, "no version is written");
    }

    /// #720 round 5, the attack's commit half, for a repository that already holds an orphan
    /// version (planted as raw objects, as a repository written before the refusal would hold
    /// one): `commit --add-block ct=plain16.tsra:volume` is refused, because the parent no longer
    /// reads, and the tip is unchanged.
    #[test]
    fn a_commit_on_an_orphan_index_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let (honest, orphan, plain, repo) = (
            dir.path().join("honest.tsra"),
            dir.path().join("orphan.tsra"),
            dir.path().join("plain16.tsra"),
            dir.path().join("repo"),
        );
        write_orphan(&honest, true);
        let m = write_orphan(&orphan, false);
        write_volume(&plain, "volume", -1024, [16, 16, 16]);
        // The honest product stores every block object; then the orphan manifest becomes the tip.
        let lineage = imported(&repo, &honest);
        assert_eq!(lineage, m.id, "precondition: one lineage");
        let seal = m.manifest_hash.clone().unwrap();
        let hex = seal.strip_prefix("blake3:").unwrap();
        let object = repo.join("objects/blake3").join(&hex[..2]).join(&hex[2..]);
        std::fs::create_dir_all(object.parent().unwrap()).unwrap();
        std::fs::write(&object, m.canonical_bytes().unwrap()).unwrap();
        let ref_file = repo
            .join("refs")
            .join(tessera_io::lineage_file_name(&lineage).unwrap());
        std::fs::write(ref_file, format!("{seal}\n")).unwrap();

        let add = format!("ct={}:volume", plain.to_str().unwrap());
        let e = commit(&repo, &lineage, &[], &[add], &[], &mut Vec::new()).unwrap_err();
        assert!(is_dangling(&e), "{e}");
        let r = Repository::open(&repo).unwrap();
        assert_eq!(
            r.read_ref(&lineage).unwrap().as_deref(),
            Some(seal.as_str())
        );
    }

    /// #720 round 4 nit: `--add-block` reads only from a source `tessera verify` accepts. A source
    /// with bytes after its end records opens (its seal holds) but does not verify, and the copy is
    /// refused with `verify`'s own error before anything is written.
    #[test]
    fn add_block_refuses_a_source_verify_rejects() {
        let dir = tempfile::tempdir().unwrap();
        let (src, base) = (dir.path().join("src.tsra"), dir.path().join("base.tsra"));
        write_volume(&src, "volume", 0, [8, 8, 8]);
        write_volume(&base, "base", 0, [8, 8, 8]);
        let mut bytes = std::fs::read(&src).unwrap();
        bytes.extend_from_slice(b"trailing");
        std::fs::write(&src, bytes).unwrap();
        let mut r = Reader::open(&src).expect("precondition: the seal still opens");
        let want = r.verify_payloads("x").unwrap_err();
        assert!(matches!(want, Error::Container(_)), "precondition: {want}");

        let repo = dir.path().join("repo");
        let lineage = imported(&repo, &base);
        let before = tip(&repo, &lineage).manifest_hash;
        let add = format!("{}:volume", src.to_str().unwrap());
        let err = commit(&repo, &lineage, &[], &[add], &[], &mut Vec::new()).unwrap_err();
        assert!(matches!(err, Error::Container(_)), "{err}");
        assert_eq!(tip(&repo, &lineage).manifest_hash, before);
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
