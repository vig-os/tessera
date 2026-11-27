use super::*;

fn target(kind: ObjectKind, address: &str) -> PinTarget {
    PinTarget { kind, address: address.into() }
}

#[test]
fn a_pin_keeps_a_version_alive_until_removed() {
    let dir = tempfile::tempdir().unwrap();
    let (repo, hash) = with_first_version(dir.path());
    let block = repo.get_manifest(&hash).unwrap().blocks[0].digest.clone().unwrap();
    repo.set_pin("release", None, &target(ObjectKind::Product, &hash)).unwrap();
    repo.forget(L).unwrap();
    assert_eq!(repo.gc().unwrap().reclaimed, 0);
    repo.get_manifest(&hash).unwrap();
    repo.read_object(&block).unwrap();
    assert!(repo.remove_pin("release", Some(&hash)).unwrap());
    assert_eq!(repo.gc().unwrap().reclaimed, 2);
    assert!(!repo.has_object(&hash) && !repo.has_object(&block));
}

#[test]
fn a_collection_pin_marks_nested_members_until_removed() {
    let dir = tempfile::tempdir().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    let (m, payloads) = sealed_on(0, None);
    let hash = repo.commit(L, None, &m, &payloads).unwrap();
    let mut b = tessera_core::CollectionBuilder::new("leaf", "d", "2024-01-01T00:00:00Z");
    b.add_member(&m.id, &hash, tessera_core::collection::Role::Raw, vec![]);
    let leaf = b.seal().unwrap();
    repo.put_collection(&leaf).unwrap();
    let mut b = tessera_core::CollectionBuilder::new("top", "d", "2024-01-01T00:00:00Z");
    b.add_subcollection(&tessera_core::collection::CollectionHandle::of(&leaf).unwrap(),
        tessera_core::collection::Role::Raw, vec![]);
    let top = repo.put_collection(&b.seal().unwrap()).unwrap();
    repo.set_pin("base", None, &target(ObjectKind::Collection, &top)).unwrap();
    repo.forget(L).unwrap();
    assert_eq!(repo.gc().unwrap().kept, 4);
    repo.get_collection(&top).unwrap();
    repo.get_manifest(&hash).unwrap();
    repo.read_object(m.blocks[0].digest.as_deref().unwrap()).unwrap();
    repo.remove_pin("base", Some(&top)).unwrap();
    assert_eq!(repo.gc().unwrap().reclaimed, 4);
}

#[test]
fn set_pin_refuses_an_unstored_address_and_a_wrong_kind() {
    let dir = tempfile::tempdir().unwrap();
    let (repo, hash) = with_first_version(dir.path());
    let absent = tessera_core::hash::digest(b"absent");
    assert!(repo.set_pin("x", None, &target(ObjectKind::Product, &absent)).is_err());
    assert!(matches!(repo.set_pin("x", None, &target(ObjectKind::Collection, &hash)),
        Err(Error::RefKindMismatch { record: ObjectKind::Collection, object: ObjectKind::Product, .. })));
    assert!(repo.pins().unwrap().is_empty());
    let block = repo.get_manifest(&hash).unwrap().blocks[0].digest.clone().unwrap();
    assert!(repo.set_pin("x", None, &target(ObjectKind::Product, &block)).is_err());
    assert!(repo.set_pin("x", None, &target(ObjectKind::Product, "garbage")).is_err());
}

#[test]
fn a_pin_and_a_lineage_with_one_name_are_independent() {
    let dir = tempfile::tempdir().unwrap();
    let (repo, hash) = with_first_version(dir.path());
    let (m, payloads) = sealed_on(7, None);
    let pin = target(ObjectKind::Product, &hash);
    repo.set_pin(&m.id, None, &pin).unwrap();
    let other = repo.commit(&m.id, None, &m, &payloads).unwrap();
    assert_eq!(repo.read_pin(&m.id).unwrap(), Some(pin.clone()));
    repo.forget(&m.id).unwrap();
    assert_eq!(repo.read_pin(&m.id).unwrap(), Some(pin.clone()));
    // Import publishes the product by committing to its id, the same repository primitive.
    repo.commit(&m.id, None, &m, &payloads).unwrap();
    repo.remove_pin(&m.id, Some(&hash)).unwrap();
    assert_eq!(repo.read_ref(&m.id).unwrap(), Some(other));
}

#[test]
fn pin_cas_and_listing_use_independent_encoded_names() {
    let dir = tempfile::tempdir().unwrap();
    let (repo, hash) = with_first_version(dir.path());
    let pin = target(ObjectKind::Product, &hash);
    for name in ["a:b", "a_b", "A_b", "a.b", "日本語"] {
        repo.set_pin(name, None, &pin).unwrap();
        assert_eq!(repo.read_pin(name).unwrap(), Some(pin.clone()));
        assert!(matches!(repo.set_pin(name, None, &pin), Err(Error::RefMoved { .. })));
        assert!(matches!(repo.remove_pin(name, None), Err(Error::RefMoved { .. })));
    }
    let listed = repo.pins().unwrap();
    assert_eq!(listed.len(), 5);
    assert_eq!(listed["日本語"], pin);
    assert_eq!(std::fs::read_to_string(dir.path().join("refs/pins/a%3ab")).unwrap(),
        format!("product {hash}\n"));
    assert!(!repo.remove_pin("absent", None).unwrap());
    for name in [String::new(), "x".repeat(250)] {
        assert!(matches!(repo.set_pin(&name, None, &pin), Err(Error::InvalidRefName { namespace, .. }) if namespace == "pins"));
        assert!(repo.read_pin(&name).is_err());
        assert!(repo.remove_pin(&name, None).is_err());
    }
    // Explicit CAS replacement can change kind; pins have no lineage identity.
    let c = tessera_core::CollectionBuilder::new("empty", "d", "2024-01-01T00:00:00Z").seal().unwrap();
    let ch = repo.put_collection(&c).unwrap();
    let replacement = target(ObjectKind::Collection, &ch);
    repo.set_pin("a:b", Some(&hash), &replacement).unwrap();
    assert_eq!(repo.read_pin("a:b").unwrap(), Some(replacement));
}

#[test]
fn removing_a_pin_preserves_other_roots() {
    let dir = tempfile::tempdir().unwrap();
    let (repo, hash) = with_first_version(dir.path());
    let pin = target(ObjectKind::Product, &hash);
    repo.set_pin("one", None, &pin).unwrap();
    repo.set_pin("two", None, &pin).unwrap();
    repo.remove_pin("one", Some(&hash)).unwrap();
    repo.gc().unwrap();
    repo.get_manifest(&hash).unwrap();
    repo.forget(L).unwrap();
    repo.gc().unwrap();
    repo.get_manifest(&hash).unwrap();
    repo.remove_pin("two", Some(&hash)).unwrap();
    assert_eq!(repo.gc().unwrap().reclaimed, 2);
}

#[test]
fn concurrent_set_pin_and_gc_never_publish_a_dangling_pin() {
    use std::sync::{Arc, Barrier};
    let dir = tempfile::tempdir().unwrap();
    let (repo, hash) = with_first_version(dir.path());
    repo.forget(L).unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let gate = barrier.clone();
    let root = dir.path().to_path_buf();
    let address = hash.clone();
    let setter = std::thread::spawn(move || {
        let repo = Repository::open(&root).unwrap();
        gate.wait();
        repo.set_pin("race", None, &target(ObjectKind::Product, &address))
    });
    barrier.wait();
    repo.gc().unwrap();
    match setter.join().unwrap() {
        Ok(()) => {
            assert_eq!(repo.read_pin("race").unwrap(), Some(target(ObjectKind::Product, &hash)));
            repo.get_manifest(&hash).unwrap();
        }
        Err(Error::Invalid(_)) => {
            assert_eq!(repo.read_pin("race").unwrap(), None);
            assert!(!repo.has_object(&hash));
        }
        Err(e) => panic!("{e}"),
    }
}

#[test]
fn pin_reads_and_listing_refuse_corrupt_records_and_wrong_kinds() {
    let dir = tempfile::tempdir().unwrap();
    let (repo, hash) = with_first_version(dir.path());
    repo.set_pin("x", None, &target(ObjectKind::Product, &hash)).unwrap();
    let path = dir.path().join("refs/pins/x");
    std::fs::write(&path, format!("collection {hash}\n")).unwrap();
    assert!(matches!(repo.read_pin("x"), Err(Error::RefKindMismatch { .. })));
    assert!(repo.pins().is_err());
    assert!(repo.gc().is_err());
    std::fs::write(&path, format!("product {hash} \n")).unwrap();
    assert!(matches!(repo.read_pin("x"), Err(Error::CorruptRef { .. })));
    assert!(repo.pins().is_err());
}

#[test]
fn pin_mutations_are_fenced_and_directory_synced() {
    let dir = tempfile::tempdir().unwrap();
    let (repo, hash) = with_first_version(dir.path());
    let lock = dir.path().join(LOCK);
    let watching = seam::watch(move |event| {
        assert!(matches!(File::open(&lock).unwrap().try_lock(),
            Err(std::fs::TryLockError::WouldBlock)), "unfenced pin step: {event:?}");
    });
    let record = seam::record_fs();
    repo.set_pin("x", None, &target(ObjectKind::Product, &hash)).unwrap();
    repo.remove_pin("x", Some(&hash)).unwrap();
    let events = record.events();
    let path = dir.path().join("refs/pins/x");
    let ns = dir.path().join("refs/pins");
    let rename = events.iter().position(|e| matches!(e, seam::Event::Rename { to, .. } if *to == path)).unwrap();
    let remove = events.iter().position(|e| *e == seam::Event::RemoveFile(path.clone())).unwrap();
    assert!(events[rename + 1..remove].contains(&seam::Event::SyncDir(ns.clone())));
    assert!(events[remove + 1..].contains(&seam::Event::SyncDir(ns)));
    drop(watching);
    File::open(dir.path().join(LOCK)).unwrap().try_lock().unwrap();
}
