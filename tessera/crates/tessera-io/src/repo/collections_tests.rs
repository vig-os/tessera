use super::*;
use tessera_core::collection::{CollectionHandle, CollectionMember, Role};
use tessera_core::{Collection, CollectionBuilder};

fn collection(name: &str, members: Vec<CollectionMember>) -> Collection {
    let mut b = CollectionBuilder::new(name, "repository test", "2024-01-01T00:00:00Z");
    for m in members {
        b.add_member(&m.reference, &m.manifest_hash, m.role, m.derived_from);
    }
    b.seal().unwrap()
}

fn subcollection(name: &str, child: &Collection) -> Collection {
    let mut b = CollectionBuilder::new(name, "repository test", "2024-01-01T00:00:00Z");
    b.add_subcollection(&CollectionHandle::of(child).unwrap(), Role::Raw, Vec::new());
    b.seal().unwrap()
}

fn store_collection(repo: &Repository, c: &Collection) -> String {
    let (bytes, seal) = c.to_stored().unwrap();
    repo.write_object(&seal, &bytes).unwrap();
    seal
}

fn root(repo: &Repository, kind: &str, address: &str) {
    std::fs::write(
        repo.ref_path("current").unwrap(),
        format!("{kind} {address}\n"),
    )
    .unwrap();
}

fn nested(repo: &Repository) -> (String, String, String) {
    let (m, payloads) = sealed_on(0, None);
    let mh = repo.commit("product", None, &m, &payloads).unwrap();
    repo.forget("product").unwrap();
    let leaf = collection("leaf", vec![CollectionMember::new(&m.id, &mh, Role::Raw)]);
    store_collection(repo, &leaf);
    let middle = subcollection("middle", &leaf);
    store_collection(repo, &middle);
    let mut b = CollectionBuilder::new("top", "d", "2024-01-01T00:00:00Z");
    b.add_subcollection(
        &CollectionHandle::of(&middle).unwrap(),
        Role::Raw,
        Vec::new(),
    );
    b.add_subcollection(&CollectionHandle::of(&leaf).unwrap(), Role::Raw, Vec::new()); // shared member
    let tip = store_collection(repo, &b.seal().unwrap());
    root(repo, "collection", &tip);
    (tip, mh, m.blocks[0].digest.clone().unwrap())
}

#[test]
fn gc_marks_through_nested_collections() {
    let dir = tempfile::tempdir().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    let (tip, mh, block) = nested(&repo);
    let junk = tessera_core::hash::digest(b"unreachable");
    repo.write_object(&junk, b"unreachable").unwrap();
    let report = repo.gc().unwrap();
    assert_eq!((report.kept, report.reclaimed), (5, 1));
    assert_eq!(repo.read_ref("current").unwrap(), Some(tip));
    repo.get_manifest(&mh).unwrap();
    repo.read_object(&block).unwrap();
}

#[test]
fn gc_aborts_on_a_missing_nested_member_and_names_its_path() {
    let dir = tempfile::tempdir().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    let (tip, mh, _) = nested(&repo);
    std::fs::remove_file(repo.object_path(&mh).unwrap()).unwrap();
    let before = objects(dir.path());
    let err = repo.gc().unwrap_err().to_string();
    assert!(err.contains(&tip) && err.contains(&mh), "{err}");
    assert!(err.contains("members[") && err.contains("current"), "{err}");
    assert_eq!(objects(dir.path()), before);
}

#[test]
fn a_product_ref_pointing_at_a_collection_is_refused() {
    mismatched_ref("product", true);
}

#[test]
fn a_collection_ref_pointing_at_a_product_is_refused() {
    mismatched_ref("collection", false);
}

fn mismatched_ref(record: &str, collection_target: bool) {
    {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let address = if collection_target {
            store_collection(&repo, &collection("empty", vec![]))
        } else {
            repo.put_manifest(&sealed_on(0, None).0).unwrap()
        };
        root(&repo, record, &address);
        let err = repo.read_ref("current").unwrap_err();
        assert!(
            matches!(&err, Error::RefKindMismatch { path, record: expected, object }
            if *path == repo.ref_path("current").unwrap() && expected != object),
            "{err}"
        );
        let before = objects(dir.path());
        assert!(matches!(repo.gc(), Err(Error::GcAborted { source, .. })
            if matches!(source.as_ref(), Error::RefKindMismatch { .. })));
        assert_eq!(objects(dir.path()), before);
    }
}

#[test]
fn a_bare_block_recorded_as_product_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    let address = tessera_core::hash::digest(b"block");
    repo.write_object(&address, b"block").unwrap();
    assert!(repo.set_ref("current", None, &address).is_err());
    assert_eq!(repo.read_ref("current").unwrap(), None);
    root(&repo, "product", &address);
    assert!(repo.read_ref("current").is_err());
    let before = objects(dir.path());
    assert!(repo.gc().is_err());
    assert_eq!(objects(dir.path()), before);
}

#[test]
fn gc_marks_through_collection_members() {
    let dir = tempfile::tempdir().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    let mut members = Vec::new();
    let mut hashes = Vec::new();
    for rev in 0..3 {
        let (m, payloads) = version(rev, None, &[rev as u32]);
        let seal = repo
            .commit(&format!("member-{rev}"), None, &m, &payloads)
            .unwrap();
        repo.forget(&format!("member-{rev}")).unwrap();
        members.push(CollectionMember::new(&m.id, &seal, Role::Raw));
        hashes.push((seal, m.blocks[0].digest.clone().unwrap()));
    }
    let tip = store_collection(&repo, &collection("current", members));
    root(&repo, "collection", &tip);
    assert_eq!(repo.gc().unwrap().kept, 7);
    for (manifest, block) in hashes {
        repo.get_manifest(&manifest).unwrap();
        repo.read_object(&block).unwrap();
    }
}

#[test]
fn gc_aborts_on_a_wrong_kind_nested_member_and_names_its_path() {
    let dir = tempfile::tempdir().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    let child = collection("child", vec![]);
    let child_hash = store_collection(&repo, &child);
    // A readable collection, falsely declared as a product member of the leaf.
    let leaf = collection(
        "leaf",
        vec![CollectionMember::new("wrong", &child_hash, Role::Raw)],
    );
    store_collection(&repo, &leaf);
    let middle = subcollection("middle", &leaf);
    store_collection(&repo, &middle);
    let tip = store_collection(&repo, &subcollection("top", &middle));
    root(&repo, "collection", &tip);
    let before = objects(dir.path());
    let err = repo.gc().unwrap_err();
    let Error::GcAborted { what, source } = &err else {
        panic!("{err}")
    };
    assert!(what.contains(&tip) && what.contains(&child_hash), "{err}");
    assert_eq!(what.matches("members[0]").count(), 3, "{err}");
    assert!(what.contains("current"), "{err}");
    assert!(
        matches!(source.as_ref(), Error::MemberKindMismatch { .. }),
        "{err}"
    );
    assert_eq!(objects(dir.path()), before);
}

#[test]
fn collection_storage_round_trips_and_replaces_torn_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    let c = collection("empty", vec![]);
    let seal = repo.put_collection(&c).unwrap();
    assert_eq!(repo.get_collection(&seal).unwrap(), c);
    assert_eq!(repo.read_object(&seal).unwrap(), c.to_stored().unwrap().0);
    repo.set_ref("current", None, &seal).unwrap();
    assert_eq!(repo.read_ref("current").unwrap(), Some(seal.clone()));
    assert_eq!(repo.gc().unwrap().kept, 1);
    assert!(matches!(
        repo.get_manifest(&seal),
        Err(Error::RefKindMismatch { .. })
    ));
    let product = repo.put_manifest(&sealed_on(0, None).0).unwrap();
    assert!(matches!(
        repo.get_collection(&product),
        Err(Error::RefKindMismatch { .. })
    ));
    std::fs::write(repo.object_path(&seal).unwrap(), b"torn").unwrap();
    assert!(repo.get_collection(&seal).is_err());
    repo.put_collection(&c).unwrap();
    assert_eq!(repo.get_collection(&seal).unwrap(), c);
    let mut edited = c;
    edited.description.push_str("edited");
    let before = objects(dir.path());
    assert!(repo.put_collection(&edited).is_err());
    assert!(repo
        .put_collection(&Collection::new("unsealed", "d", "2024-01-01T00:00:00Z"))
        .is_err());
    assert_eq!(objects(dir.path()), before);
}

#[test]
fn gc_aborts_on_a_torn_nested_member_without_sweeping() {
    let dir = tempfile::tempdir().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    let (tip, mh, _) = nested(&repo);
    std::fs::write(repo.object_path(&mh).unwrap(), b"torn").unwrap();
    let before = objects(dir.path());
    let err = repo.gc().unwrap_err().to_string();
    assert!(
        err.contains(&tip) && err.contains(&mh) && err.contains("members["),
        "{err}"
    );
    assert_eq!(objects(dir.path()), before);
}

#[test]
fn gc_checks_every_shared_members_kind_claim() {
    use tessera_core::collection::MemberKind;
    for wrong_first in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let c = collection("empty", vec![]);
        let seal = repo.put_collection(&c).unwrap();
        // Use a product claim and a collection claim for the SAME address. The stack
        // visits either claim first; an already walked target must still be checked.
        let mut parent = collection("claims", vec![]);
        let wrong = CollectionMember::new("wrong", &seal, Role::Raw);
        let right =
            CollectionMember::new("right", &seal, Role::Raw).with_kind(MemberKind::Collection);
        parent.members = if wrong_first {
            vec![right, wrong]
        } else {
            vec![wrong, right]
        };
        parent.content_hash = Some(parent.recompute_content_hash());
        parent.manifest_hash = Some(parent.compute_manifest_hash().unwrap());
        let tip = repo.put_collection(&parent).unwrap();
        repo.set_ref("current", None, &tip).unwrap();
        let before = objects(dir.path());
        let err = repo.gc().unwrap_err();
        assert!(
            matches!(&err, Error::GcAborted { source, .. }
            if matches!(source.as_ref(), Error::MemberKindMismatch { .. })),
            "{err}"
        );
        assert!(err.to_string().contains("current"));
        assert_eq!(objects(dir.path()), before);
    }
}

#[test]
fn gc_follows_collection_edges_and_leaves_missing_edges_open() {
    use tessera_core::provenance::Source;
    let dir = tempfile::tempdir().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    let (_, product, block) = nested(&repo);
    let old = repo.read_ref("current").unwrap().unwrap();
    let source_bytes = b"stored source file";
    let source = tessera_core::hash::digest(source_bytes);
    repo.write_object(&source, source_bytes).unwrap();
    let mut b = CollectionBuilder::new("edges", "d", "2024-01-01T00:00:00Z");
    b.add_source(Source::new("derived_from", "old").with_content_hash(&old));
    b.add_source(Source::new("ingested_from", "source").with_content_hash(&source));
    b.add_source(
        Source::new("derived_from", "external")
            .with_content_hash(tessera_core::hash::digest(b"held elsewhere")),
    );
    let tip = repo.put_collection(&b.seal().unwrap()).unwrap();
    repo.set_ref("current", Some(&old), &tip).unwrap();
    assert_eq!(repo.gc().unwrap().kept, 7);
    repo.get_manifest(&product).unwrap();
    repo.read_object(&block).unwrap();
    repo.read_object(&source).unwrap();
}

#[test]
fn gc_walks_deep_collections_without_a_depth_cutoff() {
    let dir = tempfile::tempdir().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    let mut c = collection("bottom", vec![]);
    repo.put_collection(&c).unwrap();
    for index in 0..128 {
        c = subcollection(&format!("level-{index}"), &c);
        repo.put_collection(&c).unwrap();
    }
    let tip = c.manifest_hash.unwrap();
    repo.set_ref("current", None, &tip).unwrap();
    assert_eq!(repo.gc().unwrap().kept, 129);
}

#[test]
fn collection_pins_are_gc_roots_and_checked_against_their_objects() {
    let dir = tempfile::tempdir().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    let (tip, product, _) = nested(&repo);
    let pin = dir.path().join("refs/pins/pinned");
    std::fs::write(&pin, format!("collection {tip}\n")).unwrap();
    repo.forget("current").unwrap();
    assert_eq!(repo.gc().unwrap().kept, 5);
    std::fs::write(&pin, format!("product {tip}\n")).unwrap();
    let before = objects(dir.path());
    assert!(matches!(repo.gc(), Err(Error::GcAborted { source, .. })
        if matches!(source.as_ref(), Error::RefKindMismatch { path, .. } if *path == pin)));
    assert_eq!(objects(dir.path()), before);
    std::fs::write(&pin, format!("collection {product}\n")).unwrap();
    assert!(matches!(repo.gc(), Err(Error::GcAborted { source, .. })
        if matches!(source.as_ref(), Error::RefKindMismatch { .. })));
    assert_eq!(objects(dir.path()), before);
}

#[test]
fn every_collection_writer_refuses_an_incompatible_layout() {
    let dir = tempfile::tempdir().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    let c = collection("c", vec![]);
    let seal = repo.put_collection(&c).unwrap();
    std::fs::write(dir.path().join("layout"), b"tessera-repository 3\n").unwrap();
    let before = objects(dir.path());
    assert!(matches!(
        repo.put_collection(&c),
        Err(Error::UnsupportedRepositoryLayout { .. })
    ));
    assert!(matches!(
        repo.get_collection(&seal),
        Err(Error::UnsupportedRepositoryLayout { .. })
    ));
    assert_eq!(objects(dir.path()), before);
}
