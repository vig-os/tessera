use super::*;
use tessera_core::collection::{CollectionHandle, CollectionMember, Role};
use tessera_core::{Collection, CollectionBuilder};

fn base() -> Collection {
    CollectionBuilder::new("current", "test", "2024-01-01T00:00:00Z")
        .seal()
        .unwrap()
}

fn next(parent: &Collection, rev: usize) -> Collection {
    let mut c = parent.clone();
    c.name = format!("revision {rev}");
    c.sources = vec![Source::new(SUPERSEDES, &parent.id)
        .with_content_hash(parent.manifest_hash.as_ref().unwrap())];
    c.manifest_hash = Some(c.compute_manifest_hash().unwrap());
    c
}

#[test]
fn commit_collection_advances_by_cas_and_reads_back() {
    let dir = tempfile::tempdir().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    let a = base();
    let ah = repo.commit_collection(L, None, &a, &[]).unwrap();
    let b = next(&a, 1);
    let bh = repo.commit_collection(L, Some(&ah), &b, &[]).unwrap();
    assert_eq!(repo.get_collection(&bh).unwrap().name, b.name);
    assert_eq!(
        repo.log(L).unwrap(),
        vec![
            LogEntry {
                manifest_hash: ah.clone(),
                parent: None
            },
            LogEntry {
                manifest_hash: bh.clone(),
                parent: Some(ah.clone())
            },
        ]
    );
    let before = objects(dir.path());
    assert!(matches!(
        repo.commit_collection(L, Some(&ah), &next(&a, 2), &[]),
        Err(Error::RefMoved { .. })
    ));
    assert_eq!(objects(dir.path()), before);
    assert_eq!(repo.read_ref(L).unwrap(), Some(bh));
}

#[test]
fn commit_collection_refuses_a_version_with_another_id() {
    let dir = tempfile::tempdir().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    let a = base();
    let ah = repo.commit_collection(L, None, &a, &[]).unwrap();
    let mut b = CollectionBuilder::new("another", "test", "2024-01-01T00:00:00Z")
        .seal()
        .unwrap();
    b.sources = next(&a, 1).sources;
    b.manifest_hash = Some(b.compute_manifest_hash().unwrap());
    let before = objects(dir.path());
    assert!(matches!(
        repo.commit_collection(L, Some(&ah), &b, &[]),
        Err(Error::LineageIdMismatch { .. })
    ));
    assert_eq!(objects(dir.path()), before);
}

#[test]
fn commit_collection_refuses_a_wrong_supersedes_edge() {
    let dir = tempfile::tempdir().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    let a = base();
    let ah = repo.commit_collection(L, None, &a, &[]).unwrap();
    for sources in [
        vec![],
        vec![Source::new(SUPERSEDES, &a.id).with_content_hash(tessera_core::hash::digest(b"wrong"))],
        vec![next(&a, 1).sources[0].clone(); 2],
    ] {
        let mut b = next(&a, 1);
        b.sources = sources;
        b.manifest_hash = Some(b.compute_manifest_hash().unwrap());
        let before = objects(dir.path());
        assert!(matches!(
            repo.commit_collection(L, Some(&ah), &b, &[]),
            Err(Error::SupersedesMismatch { .. })
        ));
        assert_eq!(objects(dir.path()), before);
    }
}

#[test]
fn member_closure_refuses_missing_torn_and_wrong_kind_members() {
    let dir = tempfile::tempdir().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    let (m, payloads) = sealed_on(0, None);
    let mh = m.manifest_hash.as_ref().unwrap();
    let (new_member, new_payloads) = version(99, None, &[9, 9]);
    let mut b = CollectionBuilder::new("current", "test", "2024-01-01T00:00:00Z");
    b.add_member(&m.id, mh, Role::Raw, vec![]);
    b.add_member(
        &new_member.id,
        new_member.manifest_hash.as_ref().unwrap(),
        Role::Raw,
        vec![],
    );
    let c = b.seal().unwrap();
    let check = |collection: &Collection, depth: usize| {
        let before = objects(dir.path());
        let err = repo
            .commit_collection(
                L,
                None,
                collection,
                &[NewMember::Product {
                    manifest: &new_member,
                    payloads: &new_payloads,
                }],
            )
            .unwrap_err();
        let Error::MissingMember { path, .. } = err else {
            panic!("{err}")
        };
        assert_eq!(path.matches("members[0]").count(), depth, "{path}");
        assert_eq!(objects(dir.path()), before);
        assert_eq!(repo.read_ref(L).unwrap(), None);
    };
    check(&c, 1);
    repo.put_collection(&c).unwrap();
    let mut parent = CollectionBuilder::new("parent", "test", "2024-01-01T00:00:00Z");
    parent.add_subcollection(&CollectionHandle::of(&c).unwrap(), Role::Raw, vec![]);
    check(&parent.seal().unwrap(), 2);
    repo.put_manifest(&m).unwrap();
    check(&c, 1); // member block is absent
    repo.write_object(m.blocks[0].digest.as_ref().unwrap(), &payloads[0].bytes)
        .unwrap();
    std::fs::write(repo.object_path(mh).unwrap(), b"torn").unwrap();
    check(&c, 1);
    repo.put_manifest(&m).unwrap();
    let mut wrong = c.clone();
    wrong.members[0].kind = MemberKind::Collection;
    wrong.content_hash = Some(wrong.recompute_content_hash());
    wrong.manifest_hash = Some(wrong.compute_manifest_hash().unwrap());
    check(&wrong, 1);
}

#[test]
fn commit_collection_stores_new_members_in_the_same_critical_section() {
    let dir = tempfile::tempdir().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    let (m, payloads) = sealed_on(0, None);
    let mut b = CollectionBuilder::new("leaf", "test", "2024-01-01T00:00:00Z");
    b.add_member(&m.id, m.manifest_hash.as_ref().unwrap(), Role::Raw, vec![]);
    let leaf = b.seal().unwrap();
    let mut b = CollectionBuilder::new("root", "test", "2024-01-01T00:00:00Z");
    b.add_subcollection(&CollectionHandle::of(&leaf).unwrap(), Role::Raw, vec![]);
    let c = b.seal().unwrap();
    let root = dir.path().to_path_buf();
    let _watch = seam::watch(move |_| {
        let lock = File::open(root.join(LOCK)).unwrap();
        assert!(
            lock.try_lock().is_err(),
            "writer fence must cover every durable step"
        );
    });
    let recording = seam::record_fs();
    let tip = repo
        .commit_collection(
            L,
            None,
            &c,
            &[
                NewMember::Collection(&leaf),
                NewMember::Product {
                    manifest: &m,
                    payloads: &payloads,
                },
            ],
        )
        .unwrap();
    let events = recording.events();
    assert!(!events.contains(&seam::Event::GcWalk));
    let destinations: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            seam::Event::Rename { to, .. } => Some(to.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        destinations,
        vec![
            repo.object_path(m.blocks[0].digest.as_ref().unwrap())
                .unwrap(),
            repo.object_path(m.manifest_hash.as_ref().unwrap()).unwrap(),
            repo.object_path(leaf.manifest_hash.as_ref().unwrap())
                .unwrap(),
            repo.object_path(&tip).unwrap(),
            repo.ref_path(L).unwrap(),
        ]
    );
    drop(recording);
    drop(_watch);
    assert_eq!(repo.gc().unwrap().kept, 4);
    assert_eq!(repo.read_ref(L).unwrap(), Some(tip));
}

#[test]
fn history_from_a_fork_reaches_the_main_version_it_forked_from() {
    let dir = tempfile::tempdir().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    let a = base();
    let ah = repo.put_collection(&a).unwrap();
    let b = next(&a, 1);
    let bh = repo.put_collection(&b).unwrap();
    let mut fork = base();
    fork.id_inputs.insert("fork".into(), "node:1".into());
    fork.id = fork.recompute_id().unwrap();
    fork.sources = vec![Source::new("forked_from", &a.id).with_content_hash(&ah)];
    fork.manifest_hash = Some(fork.compute_manifest_hash().unwrap());
    let fh = repo.put_collection(&fork).unwrap();
    let mut merged = next(&b, 2);
    merged.sources.insert(
        0,
        Source::new("forked_from", &fork.id).with_content_hash(&fh),
    );
    merged.id_inputs.insert("fork".into(), "merge:1".into());
    merged.id = merged.recompute_id().unwrap();
    merged.manifest_hash = Some(merged.compute_manifest_hash().unwrap());
    let mh = repo.put_collection(&merged).unwrap();
    let h = repo.history(&mh).unwrap();
    assert_eq!(
        h.iter().map(|r| r.address.as_str()).collect::<Vec<_>>(),
        [&mh, &bh, &ah, &fh]
    );
    assert_eq!(h[0].predecessors[0].role, SUPERSEDES);
    assert_eq!(h[3].predecessors[0].target, ah);
    assert_eq!(h[0].kind, ObjectKind::Collection);
    assert_eq!(h[0].id, merged.id);
    std::fs::remove_file(repo.object_path(&ah).unwrap()).unwrap();
    let err = repo.history(&mh).unwrap_err();
    assert!(
        matches!(&err, Error::HistoryAborted { what, .. } if what.contains(&ah) && what.contains("supersedes"))
    );
}

#[test]
fn two_lineages_share_a_base_until_both_are_forgotten() {
    let dir = tempfile::tempdir().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    let (m, payloads) = sealed_on(0, None);
    let mut c = base();
    c.members = vec![CollectionMember::new(
        &m.id,
        m.manifest_hash.as_ref().unwrap(),
        Role::Raw,
    )];
    c.content_hash = Some(c.recompute_content_hash());
    c.manifest_hash = Some(c.compute_manifest_hash().unwrap());
    repo.commit_collection(
        "one",
        None,
        &c,
        &[NewMember::Product {
            manifest: &m,
            payloads: &payloads,
        }],
    )
    .unwrap();
    let mut fork = c.clone();
    fork.id_inputs.insert("fork".into(), "workspace:1".into());
    fork.id = fork.recompute_id().unwrap();
    fork.sources = vec![
        Source::new("forked_from", &c.id).with_content_hash(c.manifest_hash.as_ref().unwrap())
    ];
    fork.manifest_hash = Some(fork.compute_manifest_hash().unwrap());
    assert_ne!(fork.id, c.id);
    repo.commit_collection("two", None, &fork, &[]).unwrap();
    assert_eq!(repo.log("two").unwrap()[0].parent, None);
    repo.forget("one").unwrap();
    assert_eq!(repo.gc().unwrap().reclaimed, 0);
    repo.get_manifest(m.manifest_hash.as_ref().unwrap())
        .unwrap();
    repo.forget("two").unwrap();
    assert_eq!(repo.gc().unwrap().reclaimed, 4);
}

#[test]
fn concurrent_commit_collection_loses_no_commit() {
    const N: usize = 8;
    let dir = tempfile::tempdir().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    let c = base();
    let first = repo.commit_collection(L, None, &c, &[]).unwrap();
    let revs: Vec<String> = (1..=N).map(|i| i.to_string()).collect();
    let mut children: Vec<Child> = revs
        .iter()
        .map(|rev| spawn_child(&["collection-writer", dir.path().to_str().unwrap(), rev]))
        .collect();
    let outcomes: Vec<String> = release(&mut children).into_iter().flatten().collect();
    let oks: Vec<&str> = outcomes
        .iter()
        .filter_map(|s| s.strip_prefix("OK "))
        .collect();
    assert_eq!(oks.len(), 1, "{outcomes:?}");
    assert_eq!(outcomes.iter().filter(|s| *s == "MOVED").count(), N - 1);
    let tip = repo.read_ref(L).unwrap().unwrap();
    let h = repo.history(&tip).unwrap();
    assert_eq!(
        h.iter().map(|r| r.address.as_str()).collect::<Vec<_>>(),
        [oks[0], &first]
    );
}

pub(super) fn racing_version(parent: &Collection, rev: usize) -> Collection {
    next(parent, rev)
}

#[test]
fn collection_commit_checks_cas_then_kind_before_version_edges() {
    let dir = tempfile::tempdir().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    let (m, payloads) = sealed_on(0, None);
    let tip = repo.commit(L, None, &m, &payloads).unwrap();
    let before = objects(dir.path());
    // Missing supersedes and unrelated id must not mask either earlier check.
    assert!(matches!(
        repo.commit_collection(L, None, &base(), &[]),
        Err(Error::RefMoved { .. })
    ));
    assert!(matches!(
        repo.commit_collection(L, Some(&tip), &base(), &[]),
        Err(Error::LineageKindMismatch { .. })
    ));
    assert_eq!(objects(dir.path()), before);
    assert_eq!(repo.read_ref(L).unwrap(), Some(tip));
}

#[test]
fn history_reports_a_torn_predecessor_with_bounded_provenance() {
    let dir = tempfile::tempdir().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    let (product, _) = sealed_on(0, None);
    let first = repo.put_manifest(&product).unwrap();
    let mut tip = first.clone();
    for rev in 1..=128 {
        let (m, _) = sealed_on(rev, Some(&tip));
        tip = repo.put_manifest(&m).unwrap();
    }
    assert_eq!(repo.history(&tip).unwrap().len(), 129);
    std::fs::write(repo.object_path(&first).unwrap(), b"torn").unwrap();
    let err = repo.history(&tip).unwrap_err();
    let Error::HistoryAborted { what, .. } = err else {
        panic!("{err}")
    };
    assert!(what.contains(&first) && what.contains("supersedes"));
    assert!(what.contains("128 source edges"), "{what}");
    assert!(what.len() < 600, "{what}");
}
