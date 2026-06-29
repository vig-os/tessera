//! Content-addressed **repository** — the copy-on-write versioning substrate (ADR-0036).
//!
//! A repository is a directory (or, later, an object-store prefix) laid out as:
//!
//! ```text
//! objects/<alg>/<aa>/<rest-of-hex>   write-once content-addressed objects (blocks AND manifests)
//! refs/<lineage>                     the latest version's manifest_hash for a lineage (mutable, CAS)
//! log/<lineage>.jsonl                rebuildable cache: one line per commit (NOT canonical)
//! ```
//!
//! Objects are addressed by the **blake3 digest we already compute** (`BlockRef.digest`,
//! `manifest_hash`) — no second hash is introduced. Identical bytes share one path, so a new version
//! that only changes metadata re-stores exactly one object (its manifest): the copy is proportional to
//! the *delta*, not the dataset. The version DAG is the `supersedes`/`derived_from` edges already inside
//! each manifest (ADR-0022); `refs/<lineage>` is the only mutable state, and the `log` is a pure cache
//! that can be rebuilt by walking the DAG from the ref.
//!
//! This module is the store + `commit`/`log` primitives; higher-level verbs (`diff`/`publish`/`seal`)
//! and the `evolve` delta builder layer on top.

use std::collections::BTreeSet;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tessera_core::{Error, Manifest, Result};

use crate::BlockPayload;

/// A content-addressed repository rooted at a directory.
pub struct Repository {
    root: PathBuf,
}

/// One entry in a lineage's `log` cache — a committed version and the tip it superseded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEntry {
    /// The committed version's `manifest_hash`.
    pub manifest_hash: String,
    /// The lineage tip this commit advanced from (`None` for the first commit).
    pub parent: Option<String>,
}

impl Repository {
    /// Create a repository under `root` (idempotent — existing dirs are fine).
    pub fn init(root: &Path) -> Result<Repository> {
        for d in ["objects", "refs", "log"] {
            std::fs::create_dir_all(root.join(d))?;
        }
        Ok(Repository {
            root: root.to_path_buf(),
        })
    }

    /// Open an existing repository (errors if `root` has no `objects/`).
    pub fn open(root: &Path) -> Result<Repository> {
        if !root.join("objects").is_dir() {
            return Err(Error::Invalid(format!(
                "{} is not a tessera repository (no objects/ — run `tessera init`)",
                root.display()
            )));
        }
        Ok(Repository {
            root: root.to_path_buf(),
        })
    }

    /// Map a `blake3:<hex>` address to its sharded on-disk path (`objects/blake3/<aa>/<rest>`).
    fn object_path(&self, address: &str) -> Result<PathBuf> {
        let (alg, hex) = address
            .split_once(':')
            .ok_or_else(|| Error::Invalid(format!("not a digest: '{address}'")))?;
        if hex.len() < 4 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::Invalid(format!("malformed digest '{address}'")));
        }
        Ok(self
            .root
            .join("objects")
            .join(alg)
            .join(&hex[..2])
            .join(&hex[2..]))
    }

    /// Write bytes at `address` if absent (content-addressed → idempotent). No digest check here;
    /// callers that hold raw bytes use [`write_object`](Self::write_object) for the verified path.
    fn write_raw(&self, address: &str, bytes: &[u8]) -> Result<()> {
        let p = self.object_path(address)?;
        if p.exists() {
            return Ok(()); // already present — content-addressed objects are immutable
        }
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&p, bytes)?;
        Ok(())
    }

    fn read_raw(&self, address: &str) -> Result<Vec<u8>> {
        Ok(std::fs::read(self.object_path(address)?)?)
    }

    /// `true` if an object with this address is stored.
    pub fn has_object(&self, address: &str) -> bool {
        self.object_path(address)
            .map(|p| p.exists())
            .unwrap_or(false)
    }

    /// Store a **block** object, verifying its bytes hash to `address` (rejects a digest mismatch
    /// before anything touches disk). Idempotent: a present object is left as-is.
    pub fn write_object(&self, address: &str, bytes: &[u8]) -> Result<()> {
        let actual = tessera_core::hash::digest(bytes);
        if actual != address {
            return Err(Error::Integrity {
                what: "object",
                expected: address.into(),
                actual,
            });
        }
        self.write_raw(address, bytes)
    }

    /// Read a **block** object, verifying its bytes still hash to `address` (catches on-disk
    /// corruption / tampering on access).
    pub fn read_object(&self, address: &str) -> Result<Vec<u8>> {
        let bytes = self.read_raw(address)?;
        let actual = tessera_core::hash::digest(&bytes);
        if actual != address {
            return Err(Error::Integrity {
                what: "object",
                expected: address.into(),
                actual,
            });
        }
        Ok(bytes)
    }

    /// Store a sealed manifest as an object **addressed by its `manifest_hash`** (the seal, not a
    /// byte-digest — so the address attests the whole product). Returns the `manifest_hash`.
    pub fn put_manifest(&self, m: &Manifest) -> Result<String> {
        let mh = m
            .manifest_hash
            .clone()
            .ok_or_else(|| Error::Invalid("put_manifest: manifest is not sealed".into()))?;
        self.write_raw(&mh, m.to_json()?.as_bytes())?;
        Ok(mh)
    }

    /// Read a manifest object back, verifying its seal **and** that the seal equals the requested
    /// address (an object filed under the wrong name is an integrity error).
    pub fn get_manifest(&self, manifest_hash: &str) -> Result<Manifest> {
        let bytes = self.read_raw(manifest_hash)?;
        let s = std::str::from_utf8(&bytes)
            .map_err(|e| Error::Invalid(format!("manifest {manifest_hash}: not UTF-8: {e}")))?;
        let m = Manifest::from_json_verified(s)?; // recomputes + checks all three hashes
        match &m.manifest_hash {
            Some(mh) if mh == manifest_hash => Ok(m),
            other => Err(Error::Integrity {
                what: "manifest_address",
                expected: manifest_hash.into(),
                actual: other.clone().unwrap_or_default(),
            }),
        }
    }

    fn ref_path(&self, lineage: &str) -> PathBuf {
        self.root
            .join("refs")
            .join(lineage.replace([':', '/', '\\'], "_"))
    }

    /// The current tip (`manifest_hash`) of a lineage, or `None` if it has no commits yet.
    pub fn read_ref(&self, lineage: &str) -> Result<Option<String>> {
        match std::fs::read_to_string(self.ref_path(lineage)) {
            Ok(s) => Ok(Some(s.trim().to_string())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Advance a lineage's ref from `expected` to `new` (compare-and-set). A tip that moved out from
    /// under us (`current != expected`) is rejected, so a concurrent commit is never silently lost.
    pub fn set_ref(&self, lineage: &str, expected: Option<&str>, new: &str) -> Result<()> {
        let current = self.read_ref(lineage)?;
        if current.as_deref() != expected {
            return Err(Error::Invalid(format!(
                "ref '{lineage}' moved: expected {expected:?}, found {current:?} (concurrent commit?)"
            )));
        }
        let p = self.ref_path(lineage);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&p, new)?;
        Ok(())
    }

    fn log_path(&self, lineage: &str) -> PathBuf {
        self.root
            .join("log")
            .join(format!("{}.jsonl", lineage.replace([':', '/', '\\'], "_")))
    }

    fn append_log(&self, lineage: &str, manifest_hash: &str, parent: Option<&str>) -> Result<()> {
        let p = self.log_path(lineage);
        if let Some(parent_dir) = p.parent() {
            std::fs::create_dir_all(parent_dir)?;
        }
        let entry = LogEntry {
            manifest_hash: manifest_hash.to_string(),
            parent: parent.map(str::to_string),
        };
        let mut line = serde_json::to_string(&entry)?;
        line.push('\n');
        let mut f = OpenOptions::new().create(true).append(true).open(&p)?;
        f.write_all(line.as_bytes())?;
        Ok(())
    }

    /// Read a lineage's `log` cache, oldest commit first (empty if the lineage has no commits).
    pub fn log(&self, lineage: &str) -> Result<Vec<LogEntry>> {
        match std::fs::read_to_string(self.log_path(lineage)) {
            Ok(s) => s
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(|l| serde_json::from_str(l).map_err(Error::from))
                .collect(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e.into()),
        }
    }

    /// Commit a sealed product as a new version on `lineage`: store any supplied block payloads
    /// content-addressed (reusing — never rewriting — blocks already present), confirm every block
    /// the manifest references is in the store, store the manifest object, advance the ref (CAS on
    /// the prior tip), and append the `log` cache. Returns the new `manifest_hash`.
    ///
    /// A metadata-only version supplies no new payloads: every block is already stored from the
    /// parent, so only the manifest object is written.
    pub fn commit(
        &self,
        lineage: &str,
        manifest: &Manifest,
        payloads: &[BlockPayload],
    ) -> Result<String> {
        let mh = manifest
            .manifest_hash
            .clone()
            .ok_or_else(|| Error::Invalid("commit: manifest is not sealed".into()))?;

        // 1. Store supplied payloads, content-addressed (skip any already present = the dedup win).
        for p in payloads {
            let br = manifest
                .blocks
                .iter()
                .find(|b| b.name == p.name)
                .ok_or_else(|| {
                    Error::Invalid(format!(
                        "commit: payload '{}' has no matching block in the manifest",
                        p.name
                    ))
                })?;
            let digest = br.digest.as_deref().ok_or_else(|| {
                Error::Invalid(format!("commit: block '{}' has no digest", br.name))
            })?;
            if !self.has_object(digest) {
                self.write_object(digest, &p.bytes)?;
            }
        }

        // 2. Every referenced block must now exist (supplied just now, or shared from a prior version).
        for b in &manifest.blocks {
            let digest = b.digest.as_deref().ok_or_else(|| {
                Error::Invalid(format!("commit: block '{}' has no digest", b.name))
            })?;
            if !self.has_object(digest) {
                return Err(Error::Invalid(format!(
                    "commit: block '{}' ({digest}) is neither supplied nor already in the store",
                    b.name
                )));
            }
        }

        // 3. Store the manifest object, advance the ref (CAS), append the log cache.
        self.put_manifest(manifest)?;
        let parent = self.read_ref(lineage)?;
        self.set_ref(lineage, parent.as_deref(), &mh)?;
        self.append_log(lineage, &mh, parent.as_deref())?;
        Ok(mh)
    }

    /// Delete a lineage's ref + `log` cache. Returns `true` if the lineage existed. The version
    /// objects are left in place — they may be shared with other lineages; run [`gc`](Self::gc) to
    /// reclaim whatever is now unreachable.
    pub fn forget(&self, lineage: &str) -> Result<bool> {
        let mut existed = false;
        match std::fs::remove_file(self.ref_path(lineage)) {
            Ok(()) => existed = true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        let lp = self.log_path(lineage);
        if lp.exists() {
            std::fs::remove_file(&lp)?;
        }
        Ok(existed)
    }

    /// The current tip of every lineage (`refs/*` contents).
    fn all_ref_tips(&self) -> Result<Vec<String>> {
        let mut tips = Vec::new();
        match std::fs::read_dir(self.root.join("refs")) {
            Ok(rd) => {
                for e in rd {
                    let e = e?;
                    if e.path().is_file() {
                        let s = std::fs::read_to_string(e.path())?;
                        let t = s.trim();
                        if !t.is_empty() {
                            tips.push(t.to_string());
                        }
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        Ok(tips)
    }

    /// Every stored object as `(address, path, size)` by walking `objects/<alg>/<aa>/<rest>`.
    fn all_objects(&self) -> Result<Vec<(String, PathBuf, u64)>> {
        let mut out = Vec::new();
        let objects = self.root.join("objects");
        let algs = match std::fs::read_dir(&objects) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(e.into()),
        };
        for alg_e in algs {
            let alg_e = alg_e?;
            if !alg_e.path().is_dir() {
                continue;
            }
            let alg = alg_e.file_name().to_string_lossy().into_owned();
            for aa_e in std::fs::read_dir(alg_e.path())? {
                let aa_e = aa_e?;
                if !aa_e.path().is_dir() {
                    continue;
                }
                let aa = aa_e.file_name().to_string_lossy().into_owned();
                for f_e in std::fs::read_dir(aa_e.path())? {
                    let f_e = f_e?;
                    if !f_e.path().is_file() {
                        continue;
                    }
                    let rest = f_e.file_name().to_string_lossy().into_owned();
                    out.push((
                        format!("{alg}:{aa}{rest}"),
                        f_e.path(),
                        f_e.metadata()?.len(),
                    ));
                }
            }
        }
        Ok(out)
    }

    /// Reclaim objects unreachable from any ref. Reachability: walk the `supersedes`/derivation edges
    /// (whose `content_hash` names an in-repo manifest) from every ref tip, marking each manifest +
    /// the blocks it references; anything not marked is deleted. Returns the sweep counts.
    pub fn gc(&self) -> Result<GcReport> {
        let mut reachable: BTreeSet<String> = BTreeSet::new();
        let mut stack = self.all_ref_tips()?;
        while let Some(mh) = stack.pop() {
            if !reachable.insert(mh.clone()) {
                continue; // already visited
            }
            let m = match self.get_manifest(&mh) {
                Ok(m) => m,
                Err(_) => continue, // not a resolvable manifest object — leave it for the sweep
            };
            for b in &m.blocks {
                if let Some(d) = &b.digest {
                    reachable.insert(d.clone());
                }
            }
            for s in &m.sources {
                if let Some(ch) = &s.content_hash {
                    if self.has_object(ch) {
                        stack.push(ch.clone());
                    }
                }
            }
        }
        let mut report = GcReport::default();
        for (addr, path, size) in self.all_objects()? {
            report.scanned += 1;
            if reachable.contains(&addr) {
                report.kept += 1;
            } else {
                std::fs::remove_file(&path)?;
                report.reclaimed += 1;
                report.bytes_reclaimed += size;
            }
        }
        Ok(report)
    }
}

/// Counts from a [`Repository::gc`] sweep.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GcReport {
    /// Objects examined.
    pub scanned: usize,
    /// Objects kept (reachable).
    pub kept: usize,
    /// Objects deleted (unreachable).
    pub reclaimed: usize,
    /// Bytes freed by the reclaimed objects.
    pub bytes_reclaimed: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::table_block;
    use crate::ColumnData;
    use tessera_core::block::table::{Column, TableSpec};
    use tessera_core::ProductBuilder;

    /// A sealed 1-block product whose block bytes are identical across calls (so versions share it),
    /// with a `rev` metadata field that differs — the metadata-only-edit fixture.
    fn sealed_with(rev: i64) -> (Manifest, Vec<BlockPayload>) {
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
        let mut b = ProductBuilder::new("listmode", "DP", "d", "2024-01-01T00:00:00Z");
        b.add_block_ref(block_ref);
        b.with_field("rev", serde_json::json!(rev));
        let m = b.seal().unwrap();
        (m, vec![payload])
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
    fn commit_dedups_shared_blocks_and_advances_ref() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();

        let (m1, p1) = sealed_with(1);
        let mh1 = repo.commit("study", &m1, &p1).unwrap();
        assert_eq!(count_objects(dir.path()), 2, "v1 = 1 block + 1 manifest");

        let (m2, p2) = sealed_with(2);
        assert_eq!(
            m1.blocks[0].digest, m2.blocks[0].digest,
            "block bytes identical → same digest"
        );
        let mh2 = repo.commit("study", &m2, &p2).unwrap();
        assert_eq!(
            count_objects(dir.path()),
            3,
            "v2 shares the block → only the manifest is new"
        );
        assert_ne!(mh1, mh2, "metadata changed → different seal");

        assert_eq!(repo.read_ref("study").unwrap(), Some(mh2.clone()));
        assert_eq!(
            repo.get_manifest(&mh1).unwrap().metadata.get("rev"),
            Some(&serde_json::json!(1))
        );
        assert_eq!(
            repo.get_manifest(&mh2).unwrap().metadata.get("rev"),
            Some(&serde_json::json!(2))
        );

        let log = repo.log("study").unwrap();
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].manifest_hash, mh1);
        assert_eq!(log[1].manifest_hash, mh2);
        assert_eq!(log[1].parent.as_deref(), Some(mh1.as_str()));
    }

    #[test]
    fn metadata_only_commit_writes_exactly_one_object() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let (m1, p1) = sealed_with(1);
        repo.commit("study", &m1, &p1).unwrap();
        let before = count_objects(dir.path());
        // Second version supplies NO payloads — the block is already stored; only the manifest is new.
        let (m2, _) = sealed_with(2);
        repo.commit("study", &m2, &[]).unwrap();
        assert_eq!(count_objects(dir.path()), before + 1);
    }

    #[test]
    fn commit_rejects_a_block_neither_supplied_nor_stored() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let (m, _payloads) = sealed_with(1);
        let err = repo.commit("study", &m, &[]).unwrap_err();
        assert!(format!("{err}").contains("neither supplied nor already in the store"));
    }

    #[test]
    fn set_ref_rejects_a_moved_tip() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        repo.set_ref("s", None, "blake3:aa").unwrap();
        // expecting None but the tip is now blake3:aa → CAS fails
        let err = repo.set_ref("s", None, "blake3:bb").unwrap_err();
        assert!(format!("{err}").contains("moved"));
        repo.set_ref("s", Some("blake3:aa"), "blake3:bb").unwrap();
        assert_eq!(repo.read_ref("s").unwrap(), Some("blake3:bb".to_string()));
    }

    #[test]
    fn forget_then_gc_reclaims_exclusive_keeps_shared() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let (m1, p1) = sealed_with(1);
        let (m2, p2) = sealed_with(2); // different metadata, SAME block digest
        repo.commit("a", &m1, &p1).unwrap();
        repo.commit("b", &m2, &p2).unwrap();
        assert_eq!(count_objects(dir.path()), 3, "1 shared block + 2 manifests");

        // nothing to collect while both refs live
        assert_eq!(repo.gc().unwrap().reclaimed, 0);

        // forget "a" → gc reclaims a's manifest, keeps the shared block (b still references it)
        assert!(repo.forget("a").unwrap());
        let rep = repo.gc().unwrap();
        assert_eq!(rep.reclaimed, 1, "a's now-unreachable manifest");
        assert_eq!(
            count_objects(dir.path()),
            2,
            "shared block + b's manifest remain"
        );

        // b is intact + verifiable
        let tip_b = repo.read_ref("b").unwrap().unwrap();
        repo.get_manifest(&tip_b).unwrap();
        assert!(
            !repo.forget("nope").unwrap(),
            "forgetting an absent lineage is a no-op"
        );
    }

    #[test]
    fn read_object_detects_corruption() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let (m, p) = sealed_with(1);
        repo.commit("study", &m, &p).unwrap();
        let digest = m.blocks[0].digest.clone().unwrap();
        std::fs::write(repo.object_path(&digest).unwrap(), b"corrupted").unwrap();
        assert!(matches!(
            repo.read_object(&digest),
            Err(Error::Integrity { .. })
        ));
    }
}
