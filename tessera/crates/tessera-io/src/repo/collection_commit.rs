use super::*;

/// A sealed member supplied to an atomic collection commit. Payloads belong to their product's
/// block names. Supply nested collections and products in any order; shared objects deduplicate.
/// All supplied values are validated before any object is stored.
pub enum NewMember<'a> {
    /// A sealed product and any block payloads not already stored.
    Product {
        /// The member's sealed manifest.
        manifest: &'a Manifest,
        /// Encoded block bytes, matched to the manifest by block name.
        payloads: &'a [BlockPayload],
    },
    /// A sealed sub-collection; its members must be stored or supplied in the same call.
    Collection(&'a Collection),
}

/// A labelled predecessor in the version DAG, pinned by seal, not by member content hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEdge {
    /// `supersedes` or `forked_from`.
    pub role: String,
    /// The predecessor's manifest seal/address.
    pub target: String,
}

/// A distinct version reached by [`Repository::history`]. Members and derivation edges are
/// excluded: this is the version graph for callers computing ancestry, not a GC reachability walk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    /// This version's manifest seal/address.
    pub address: String,
    /// Product or collection, inferred from and checked against the sealed bytes.
    pub kind: ObjectKind,
    /// The version's stable lineage identity.
    pub id: String,
    /// Predecessors in walk order: supersedes first, then forked_from, preserving source order
    /// within each role.
    pub predecessors: Vec<HistoryEdge>,
}

impl Version {
    fn id(&self) -> &str {
        match self {
            Self::Product(m) => &m.id,
            Self::Collection(c) => &c.id,
        }
    }

    fn sources(&self) -> &[tessera_core::provenance::Source] {
        match self {
            Self::Product(m) => &m.sources,
            Self::Collection(c) => &c.sources,
        }
    }
}

impl Repository {
    /// Commit a sealed collection and new members atomically under the repository writer fence.
    /// `expected` is the caller's observed tip (`None` for the first version). CAS is checked
    /// first, then the lineage kind, exactly one supersedes edge to the expected tip, and stable
    /// lineage id. Errors are [`Error::RefMoved`], [`Error::LineageKindMismatch`],
    /// [`Error::SupersedesMismatch`] and [`Error::LineageIdMismatch`], respectively.
    ///
    /// Every member, recursively, must be readable as its declared kind from the store or
    /// `members`. Each product's blocks must exist or have supplied, digest-checked payloads.
    /// Missing, corrupt or wrong-kind members are [`Error::MissingMember`] with the member path
    /// and underlying cause. Unchanged blocks are checked for existence only, as in `commit`.
    /// Seals, version pins and required fork tokens are checked by the core writer validators.
    /// Validation finishes before any storage. Blocks, products, then collections in dependency
    /// order are stored durably before advancing the collection ref and appending the log cache.
    ///
    /// The fence prevents GC between storing a new member and making it reachable. Supplied
    /// values may be in any order; the validation walk uses a heap stack and visits each shared
    /// member once, checking every incoming kind claim. Edges are open for commit (D3.1);
    /// [`Self::history`] instead reports missing predecessors as errors. A failed log append can
    /// report an error after the ref has advanced, as for product commits.
    pub fn commit_collection(
        &self,
        lineage: &str,
        expected: Option<&str>,
        collection: &Collection,
        members: &[NewMember<'_>],
    ) -> Result<String> {
        lineage_file_name(lineage)?;
        let writer = self.lock()?;
        self.check_ref(lineage, expected)?;
        self.check_lineage_kind(lineage, ObjectKind::Collection)?;
        if let Some(parent) = expected {
            check_source_supersedes(lineage, parent, &collection.sources)?;
            let id = self.get_collection(parent)?.id;
            if collection.id != id {
                return Err(Error::LineageIdMismatch {
                    lineage: lineage.into(),
                    expected: id,
                    found: collection.id.clone(),
                });
            }
        }
        let (bytes, seal) = crate::stored::collection(collection)?;
        require_object_kind(Path::new(&seal), ObjectKind::Collection, &bytes)?;
        // Keep prepared bytes separate from the validated graph. No writes until closure succeeds.
        let mut objects = BTreeMap::new();
        let mut blocks = BTreeMap::new();
        for member in members {
            match member {
                NewMember::Collection(c) => {
                    let (bytes, seal) = crate::stored::collection(c)?;
                    require_object_kind(Path::new(&seal), ObjectKind::Collection, &bytes)?;
                    objects.insert(seal, bytes);
                }
                NewMember::Product { manifest, payloads } => {
                    let (seal, bytes) = stored_manifest(manifest)?;
                    for payload in *payloads {
                        let block = manifest
                            .blocks
                            .iter()
                            .find(|b| b.name == payload.name)
                            .ok_or_else(|| {
                                Error::Invalid(format!(
                                    "collection commit: payload '{}' has no matching block",
                                    payload.name
                                ))
                            })?;
                        let digest = block_digest(block)?;
                        check_digest(digest, &payload.bytes)?;
                        blocks.insert(digest.to_string(), payload.bytes.as_slice());
                    }
                    objects.insert(seal, bytes);
                }
            }
        }
        objects.insert(seal.clone(), bytes);
        let mut paths = WalkPaths::default();
        let root = paths.push(None, PathLabel::Version(seal.clone()));
        let mut stack: Vec<_> = objects
            .keys()
            .map(|a| (a.clone(), None, root, false))
            .collect();
        stack.retain(|(address, _, _, _)| address != &seal);
        stack.push((seal.clone(), None, root, false));
        let mut walked = BTreeMap::new();
        let mut ordered = Vec::new();
        while let Some((address, expect, via, exiting)) = stack.pop() {
            if exiting {
                if objects.contains_key(&address) {
                    ordered.push(address);
                }
                continue;
            }
            let fail = |source| Error::MissingMember {
                path: format!("{address} ({})", paths.render(via)),
                source: Box::new(source),
            };
            if let Some(kind) = walked.get(&address) {
                check_member_kind(expect, *kind, &address).map_err(fail)?;
                continue;
            }
            let version = if let Some(bytes) = objects.get(&address) {
                read_version(bytes, &address)
            } else {
                self.read_raw(&address)
                    .and_then(|bytes| read_version(&bytes, &address))
            }
            .map_err(fail)?;
            check_member_kind(expect, version.kind(), &address).map_err(fail)?;
            walked.insert(address.clone(), version.kind());
            stack.push((address.clone(), expect, via, true));
            match version {
                Version::Product(m) => {
                    for b in &m.blocks {
                        let digest = block_digest(b).map_err(fail)?;
                        if !blocks.contains_key(digest) && !self.has_object(digest) {
                            return Err(fail(Error::Invalid(format!(
                                "block '{}' ({digest}) is absent",
                                b.name
                            ))));
                        }
                    }
                }
                Version::Collection(c) => {
                    let children = c
                        .members
                        .iter()
                        .enumerate()
                        .map(|(index, m)| {
                            let kind = match m.kind {
                                MemberKind::Product => ObjectKind::Product,
                                MemberKind::Collection => ObjectKind::Collection,
                                _ => {
                                    return Err(fail(Error::Invalid(
                                        "unsupported member kind".into(),
                                    )))
                                }
                            };
                            Ok((index, m, kind))
                        })
                        .collect::<Result<Vec<_>>>()?;
                    for (index, m, kind) in children.into_iter().rev() {
                        let child = paths.push(
                            Some(via),
                            PathLabel::Member {
                                collection: address.clone(),
                                index,
                                reference: m.reference.clone(),
                            },
                        );
                        stack.push((m.manifest_hash.clone(), Some(kind), child, false));
                    }
                }
            }
        }
        for (address, bytes) in blocks {
            self.store(&writer, &address, bytes)?;
        }
        // Postorder guarantees nested dependencies precede their collections. Products have no
        // member dependencies and all block payloads are already durable.
        for kind in [ObjectKind::Product, ObjectKind::Collection] {
            for address in &ordered {
                if walked[address] == kind {
                    self.store(&writer, address, &objects[address])?;
                }
            }
        }
        self.advance_ref(&writer, lineage, expected, &seal)?;
        self.append_log(lineage, &seal, expected)?;
        Ok(seal)
    }

    /// Walk the version graph from any stored product or collection seal, including the start.
    /// Returns distinct records in depth-first preorder, following supersedes before forked_from
    /// (source order within a role). Members and other source roles are excluded. Shared ancestors
    /// appear once. Every read checks kind, canonical bytes and seal; a missing or unreadable
    /// predecessor is [`Error::HistoryAborted`] naming its address and the edge that reached it,
    /// never a silently truncated chain. This is stricter than GC's open external-edge rule.
    ///
    /// Uses an explicit heap stack, no depth cutoff, and parent-index provenance. Error paths
    /// summarize consecutive edges by hop count, bounding rendering for long chains. Readers do
    /// not take the writer fence: concurrent forget/GC can cause a typed read error.
    pub fn history(&self, address: &str) -> Result<Vec<HistoryEntry>> {
        self.layout()?;
        let mut paths = WalkPaths::default();
        let root = paths.push(None, PathLabel::Version(address.into()));
        let mut stack = vec![(address.to_string(), root)];
        let mut walked = BTreeSet::new();
        let mut entries = Vec::new();
        while let Some((address, via)) = stack.pop() {
            if !walked.insert(address.clone()) {
                continue;
            }
            let fail = |source| Error::HistoryAborted {
                what: format!("{address} ({})", paths.render(via)),
                source: Box::new(source),
            };
            let version = self
                .read_raw(&address)
                .and_then(|bytes| read_version(&bytes, &address))
                .map_err(fail)?;
            let mut predecessors = Vec::new();
            for role in [SUPERSEDES, "forked_from"] {
                for source in version.sources().iter().filter(|s| s.role == role) {
                    let target = source.content_hash.clone().ok_or_else(|| {
                        fail(Error::Invalid(format!(
                            "{role} edge of {address} has no predecessor seal"
                        )))
                    })?;
                    predecessors.push(HistoryEdge {
                        role: role.into(),
                        target,
                    });
                }
            }
            for edge in predecessors.iter().rev() {
                let child = paths.push(
                    Some(via),
                    PathLabel::Edge {
                        object: address.clone(),
                        role: edge.role.clone(),
                    },
                );
                stack.push((edge.target.clone(), child));
            }
            entries.push(HistoryEntry {
                address,
                kind: version.kind(),
                id: version.id().into(),
                predecessors,
            });
        }
        Ok(entries)
    }
}

fn check_member_kind(expect: Option<ObjectKind>, object: ObjectKind, path: &str) -> Result<()> {
    if let Some(expected) = expect {
        if expected != object {
            return Err(Error::MemberKindMismatch {
                path: path.into(),
                expected,
                object,
            });
        }
    }
    Ok(())
}

pub(super) fn check_source_supersedes(
    lineage: &str,
    parent: &str,
    sources: &[tessera_core::provenance::Source],
) -> Result<()> {
    let found: Vec<String> = sources
        .iter()
        .filter(|s| s.role == SUPERSEDES)
        .map(|s| s.content_hash.clone().unwrap_or_else(|| "(none)".into()))
        .collect();
    if found != [parent] {
        return Err(Error::SupersedesMismatch {
            lineage: lineage.into(),
            expected: parent.into(),
            found,
        });
    }
    Ok(())
}
