//! Layout-2 acceptance tests (ADR-0067 step 1).
use std::path::Path;
use tessera_core::{Error, ProductBuilder};
use tessera_io::repo::Repository;

fn typed_tip(root: &Path, name: &str) -> (Repository, String) {
    let repo = Repository::init(root).unwrap();
    let m = ProductBuilder::new("listmode", "DP", "d", "2024-01-01T00:00:00Z")
        .seal()
        .unwrap();
    let address = repo.put_manifest(&m).unwrap();
    repo.set_ref(name, None, &address).unwrap();
    assert_eq!(
        std::fs::read_to_string(root.join("refs/lineages").join(name)).unwrap(),
        format!("product {address}\n")
    );
    (repo, address)
}

#[test]
fn layout1_is_refused_naming_found_and_required() {
    let dir = tempfile::tempdir().unwrap();
    Repository::init(dir.path()).unwrap();
    std::fs::write(dir.path().join("layout"), b"tessera-repository 1\n").unwrap();
    for result in [Repository::open(dir.path()), Repository::init(dir.path())] {
        assert!(
            matches!(result, Err(Error::UnsupportedRepositoryLayout { found, required, .. })
            if found.contains("tessera-repository 1") && required.contains("tessera-repository 2"))
        );
    }
}

#[test]
fn typed_product_record_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let (repo, address) = typed_tip(dir.path(), "x");
    assert_eq!(repo.read_ref("x").unwrap(), Some(address));
}

#[test]
fn malformed_records_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (repo, address) = typed_tip(dir.path(), "x");
    for record in [
        address.clone(),
        format!("product {address}"),
        format!("product {address} \n"),
        format!("product {}\n", address.to_uppercase()),
        format!("product  {address}\n"),
        format!("product {address}\n\n"),
    ] {
        std::fs::write(dir.path().join("refs/lineages/x"), &record).unwrap();
        assert!(
            matches!(repo.read_ref("x"), Err(Error::CorruptRef { .. })),
            "{record:?}"
        );
        assert!(matches!(repo.gc(), Err(Error::GcAborted { .. })));
        assert!(repo.has_object(&address));
    }
}

#[test]
fn unknown_record_kind_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (repo, address) = typed_tip(dir.path(), "x");
    for kind in ["unknown", "Collection"] {
        std::fs::write(
            dir.path().join("refs/lineages/x"),
            format!("{kind} {address}\n"),
        )
        .unwrap();
        assert!(matches!(repo.read_ref("x"), Err(Error::CorruptRef { .. })));
        assert!(matches!(repo.gc(), Err(Error::GcAborted { .. })));
        assert!(repo.has_object(&address));
    }
}

#[test]
fn lineage_and_pin_namespaces_never_collide() {
    let dir = tempfile::tempdir().unwrap();
    let (repo, address) = typed_tip(dir.path(), "x");
    let pin = dir.path().join("refs/pins/x");
    std::fs::write(&pin, format!("product {address}\n")).unwrap();
    assert!(repo.forget("x").unwrap());
    assert_eq!(repo.read_ref("x").unwrap(), None);
    assert_eq!(
        std::fs::read_to_string(pin).unwrap(),
        format!("product {address}\n")
    );
}
