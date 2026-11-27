//! ADR-0067 step 3: object discrimination and collection member differences.

use proptest::prelude::*;
use tessera_core::collection::{diff, CollectionMember, MemberKind};
use tessera_core::{Collection, CollectionBuilder, Error, ObjectKind, ProductBuilder, Role};

const TS: &str = "2026-11-26T00:00:00Z";

fn member(reference: &str, seal: &str) -> CollectionMember {
    CollectionMember {
        reference: reference.into(),
        manifest_hash: seal.into(),
        kind: MemberKind::Product,
        role: Role::Raw,
        derived_from: vec![],
    }
}

fn collection(members: Vec<CollectionMember>) -> Collection {
    let mut c = Collection::new("c", "d", TS);
    c.members = members;
    c
}

#[test]
fn object_kind_discriminates_a_zero_block_manifest() {
    let product = ProductBuilder::new("recon", "p", "d", TS).seal().unwrap();
    let (bytes, _) = product.to_stored().unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(value.get("product").is_some());
    assert!(value.get("members").is_none());
    assert_eq!(ObjectKind::of(&bytes).unwrap(), ObjectKind::Product);
    let c = CollectionBuilder::new("c", "d", TS).seal().unwrap();
    let (bytes, _) = c.to_stored().unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["members"], serde_json::json!([]));
    assert!(value.get("product").is_none());
    assert_eq!(ObjectKind::of(&bytes).unwrap(), ObjectKind::Collection);
    for bytes in [
        br#"{"tessera_version":"0.0.0","product":{},"members":[]}"#.as_slice(),
        br#"{"tessera_version":"0.0.0"}"#,
        br#"{"product":{}}"#,
        br#"{"members":[]}"#,
        br#"[]"#,
        br#"null"#,
        br#"42"#,
        br#"{"nested":{"tessera_version":"0.0.0","product":{}}}"#,
    ] {
        assert!(matches!(
            ObjectKind::of(bytes),
            Err(Error::AmbiguousObjectKind)
        ));
    }
    assert!(matches!(ObjectKind::of(b"{"), Err(Error::Serde(_))));
    // Classification tests key presence, not field values or seal validity.
    assert_eq!(
        ObjectKind::of(br#"{"tessera_version":null,"product":null}"#).unwrap(),
        ObjectKind::Product
    );
}

#[test]
fn member_diff_reports_added_removed_repinned_and_order() {
    let old = collection(vec![
        member("z", "0"),
        member("b", "1"),
        member("a", "2"),
        member("x", "3"),
    ]);
    let mut changed_kind = member("a", "2");
    changed_kind.kind = MemberKind::Collection;
    let new = collection(vec![
        member("y", "4"),
        changed_kind,
        member("b", "5"),
        member("w", "6"),
    ]);
    let d = diff(&old, &new).unwrap();
    assert_eq!(
        d.added,
        vec![new.members[0].clone(), new.members[3].clone()]
    );
    assert_eq!(
        d.removed,
        vec![old.members[0].clone(), old.members[3].clone()]
    );
    assert_eq!(d.repinned.len(), 2);
    assert_eq!(d.repinned[0].reference, "a");
    assert_eq!(d.repinned[0].old_manifest_hash, "2");
    assert_eq!(d.repinned[0].new_manifest_hash, "2");
    assert_eq!(d.repinned[1].reference, "b");
    assert_eq!(d.repinned[1].old_manifest_hash, "1");
    assert_eq!(d.repinned[1].new_manifest_hash, "5");
    assert!(d.order_changed);
    assert_eq!(diff(&old, &new).unwrap(), d);
    let unchanged = diff(&old, &old).unwrap();
    assert!(
        unchanged.added.is_empty() && unchanged.removed.is_empty() && unchanged.repinned.is_empty()
    );
    assert!(!unchanged.order_changed);
    let removed = diff(&old, &collection(vec![])).unwrap();
    assert_eq!(removed.removed, old.members);
    assert!(!removed.order_changed);
    let added = diff(&collection(vec![]), &new).unwrap();
    assert_eq!(added.added, new.members);
    assert!(!added.order_changed);
    let stable = collection(vec![member("y", "4"), member("b", "5"), member("a", "2")]);
    assert!(!diff(&old, &stable).unwrap().order_changed);
}

#[test]
fn member_diff_refuses_a_duplicate_reference() {
    let empty = collection(vec![]);
    for second in [member("a", "1"), member("a", "2")] {
        let duplicate = collection(vec![member("a", "1"), second]);
        for (old, new) in [
            (&duplicate, &empty),
            (&empty, &duplicate),
            (&duplicate, &duplicate),
        ] {
            match diff(old, new) {
                Err(Error::DuplicateMemberReference { reference }) => assert_eq!(reference, "a"),
                other => panic!("expected duplicate error, got {other:?}"),
            }
        }
    }
}

proptest! {
    #[test]
    fn member_diff_applied_to_old_gives_new(
        old_values in prop::collection::btree_map(0u8..32, (any::<u8>(), any::<bool>()), 0..20),
        new_values in prop::collection::btree_map(0u8..32, (any::<u8>(), any::<bool>()), 0..20),
        reverse_old in any::<bool>(), reverse_new in any::<bool>(),
    ) {
        let make = |values: std::collections::BTreeMap<u8, (u8, bool)>, reverse| {
            let mut members: Vec<_> = values.into_iter().map(|(r, (h, kind))| {
                let mut m = member(&r.to_string(), &h.to_string());
                if kind { m.kind = MemberKind::Collection; }
                m
            }).collect();
            if reverse { members.reverse(); }
            collection(members)
        };
        let old = make(old_values, reverse_old);
        let new = make(new_values, reverse_new);
        let d = diff(&old, &new).unwrap();
        let mut applied: std::collections::BTreeMap<_, _> = old.members.iter()
            .map(|m| (m.reference.clone(), m.clone())).collect();
        for m in &d.removed { prop_assert!(applied.remove(&m.reference).is_some()); }
        for m in &d.added { prop_assert!(applied.insert(m.reference.clone(), m.clone()).is_none()); }
        for pin in &d.repinned {
            let before = applied.get(&pin.reference).unwrap();
            prop_assert_eq!(&before.manifest_hash, &pin.old_manifest_hash);
            let after = new.members.iter().find(|m| m.reference == pin.reference).unwrap();
            prop_assert_eq!(&after.manifest_hash, &pin.new_manifest_hash);
            prop_assert!(before.manifest_hash != after.manifest_hash || before.kind != after.kind);
            applied.insert(pin.reference.clone(), after.clone());
        }
        let expected: std::collections::BTreeMap<_, _> = new.members.iter()
            .map(|m| (m.reference.clone(), m.clone())).collect();
        prop_assert_eq!(&applied, &expected);
        let old_shared: Vec<_> = old.members.iter().filter(|m| new.members.iter()
            .any(|n| n.reference == m.reference)).map(|m| &m.reference).collect();
        let new_shared: Vec<_> = new.members.iter().filter(|m| old.members.iter()
            .any(|n| n.reference == m.reference)).map(|m| &m.reference).collect();
        prop_assert_eq!(d.order_changed, old_shared != new_shared);
        // The new order is the ordering witness: the diff reports a boolean, not a permutation.
        let reconstructed: Vec<_> = new.members.iter().map(|m| applied[&m.reference].clone()).collect();
        prop_assert_eq!(reconstructed, new.members);
    }
}

#[test]
fn object_kind_refuses_duplicate_keys_and_invalid_utf8() {
    for bytes in [
        br#"{"tessera_version":"0.0.0","product":{},"product":null}"#.as_slice(),
        br#"{"tessera_version":"0.0.0","members":[],"nested":{"a":0,"a":1}}"#,
        b"\xff",
    ] {
        assert!(matches!(ObjectKind::of(bytes), Err(Error::Serde(_))));
    }
}

#[test]
fn member_diff_compares_pins_independently_of_metadata() {
    let old = collection(vec![member("a", "1"), member("b", "2")]);
    let mut new = old.clone();
    new.members[0].role = Role::Derived;
    new.members[0].derived_from = vec!["parent".into()];
    new.description = "updated metadata".into();
    new.sources
        .push(tessera_core::Source::new("supersedes", "old"));
    assert_eq!(diff(&old, &new).unwrap(), Default::default());
    new.members.reverse();
    let reordered = diff(&old, &new).unwrap();
    assert!(reordered.order_changed);
    assert!(reordered.added.is_empty());
    assert!(reordered.removed.is_empty());
    assert!(reordered.repinned.is_empty());
}
