//! Executable book transcripts. Valid fixtures are sealed through public APIs.
//! The missing-token refusal models a received object that a reader can verify but a writer refuses.
use std::path::Path;
use tessera_core::collection::{CollectionBuilder, ProductHandle, Role};
use tessera_core::provenance::Source;
use tessera_core::{Collection, ProductBuilder};

fn collection(
    parent: Option<&Collection>,
    fork: Option<&str>,
    members: &[tessera_core::Manifest],
) -> Collection {
    let mut b = CollectionBuilder::new("current", "repository walkthrough", "2024-01-01T00:00:00Z");
    if let Some(parent) = parent {
        let mut inputs = parent.id_inputs.clone();
        let role = if let Some(token) = fork {
            inputs.insert("fork".into(), token.into());
            "forked_from"
        } else {
            "supersedes"
        };
        b.with_id_inputs(inputs);
        if fork.is_none() {
            // Preserve origins before the new supersedes edge deliberately: history must group
            // supersedes first and visit the shared origin only once, independent of source order.
            for source in &parent.sources {
                if source.role != "supersedes" {
                    b.add_source(source.clone());
                }
            }
        }
        b.add_source(
            Source::new(role, &parent.id).with_content_hash(parent.manifest_hash.as_ref().unwrap()),
        );
    }
    for member in members {
        b.add_product(&ProductHandle::of(member).unwrap(), Role::Derived, vec![]);
    }
    b.seal().unwrap()
}

fn write(dir: &Path, name: &str, c: &Collection) {
    let sub = dir.join(name);
    std::fs::create_dir(&sub).unwrap();
    tessera_io::stored::write_collection(&sub.join("collection.json"), c).unwrap();
}

#[test]
fn repository_walkthroughs() {
    let tmp = tempfile::tempdir().unwrap();
    let mut b = ProductBuilder::new("generic", "base", "base", "2024-01-01T00:00:00Z");
    b.with_producer(tessera_core::Producer::new("repository-fixture", "1"));
    let (block, base_payload) =
        tessera_io::blob::blob_block("data", "base.txt", Some("text/plain"), b"base".to_vec())
            .unwrap();
    b.add_block_ref(block);
    let base = b.seal().unwrap();
    let mut b = ProductBuilder::new("generic", "delta", "delta", "2024-01-01T00:00:00Z");
    b.with_producer(tessera_core::Producer::new("repository-fixture", "1"));
    let (block, delta_payload) =
        tessera_io::blob::blob_block("data", "delta.txt", Some("text/plain"), b"delta".to_vec())
            .unwrap();
    b.add_block_ref(block);
    let delta = b.seal().unwrap();
    let main = collection(None, None, std::slice::from_ref(&base));
    let fork = collection(Some(&main), Some("node-a:1"), std::slice::from_ref(&base));
    let next = collection(Some(&fork), None, &[base.clone(), delta.clone()]);
    let merged = collection(Some(&main), None, &[base.clone(), delta.clone()]);
    // Same id as the product, so the existing product lineage is the intended kind refusal.
    let mut b = CollectionBuilder::new("wrong-kind", "", "2024-01-01T00:00:00Z");
    b.with_id_inputs(base.id_inputs.clone());
    let wrong = b.seal().unwrap();
    assert_eq!(wrong.id, base.id);
    let vars = [
        ("@MAIN@", main.id.clone()),
        ("@FORK@", fork.id.clone()),
        ("@BASE@", base.id.clone()),
        ("@DELTA@", delta.id.clone()),
        ("@M@", main.manifest_hash.clone().unwrap()),
        ("@F@", fork.manifest_hash.clone().unwrap()),
        ("@N@", next.manifest_hash.clone().unwrap()),
        ("@MERGED@", merged.manifest_hash.clone().unwrap()),
        ("@B@", base.manifest_hash.clone().unwrap()),
        ("@D@", delta.manifest_hash.clone().unwrap()),
        ("@ABSENT@", tessera_core::hash::digest(b"not stored")),
        ("@BLOCK@", base.blocks[0].digest.clone().unwrap()),
    ];
    // Register both before running: a lifecycle failure must not prevent the refusal proof.
    let cases = trycmd::TestCases::new();
    cases.register_bin(
        "tessera",
        std::path::PathBuf::from(env!("CARGO_BIN_EXE_tessera")),
    );
    for name in ["lifecycle", "refusals"] {
        let input = tmp.path().join(format!("{name}.in"));
        std::fs::create_dir(&input).unwrap();
        std::fs::create_dir(input.join("unmarked")).unwrap();
        let damaged = input.join("damaged");
        let store = tessera_io::Repository::init(&damaged).unwrap();
        let delta_address = store
            .commit(
                &delta.id,
                None,
                &delta,
                std::slice::from_ref(&delta_payload),
            )
            .unwrap();
        std::fs::write(
            damaged.join("refs/pins/missing"),
            format!("product {}\n", tessera_core::hash::digest(b"not stored")),
        )
        .unwrap();
        std::fs::write(
            damaged.join("refs/pins/torn"),
            format!("product {delta_address}\n"),
        )
        .unwrap();
        // Isolate listing's one damaged pin: read_dir's error order is unspecified.
        let kind_only = input.join("kind-only");
        let store = tessera_io::Repository::init(&kind_only).unwrap();
        let address = store
            .commit(&base.id, None, &base, std::slice::from_ref(&base_payload))
            .unwrap();
        std::fs::write(
            kind_only.join("refs/pins/kind"),
            format!("collection {address}\n"),
        )
        .unwrap();
        let object = |root: &Path, seal: &str| {
            let hex = seal.strip_prefix("blake3:").unwrap();
            root.join("objects/blake3").join(&hex[..2]).join(&hex[2..])
        };
        std::fs::write(object(&damaged, &delta_address), b"{").unwrap();
        let missing_block = input.join("missing-block");
        let store = tessera_io::Repository::init(&missing_block).unwrap();
        store
            .commit(&base.id, None, &base, std::slice::from_ref(&base_payload))
            .unwrap();
        std::fs::remove_file(object(
            &missing_block,
            base.blocks[0].digest.as_ref().unwrap(),
        ))
        .unwrap();

        tessera_io::pack(
            &base,
            std::slice::from_ref(&base_payload),
            &input.join("base.tsra"),
        )
        .unwrap();
        tessera_io::pack(
            &delta,
            std::slice::from_ref(&delta_payload),
            &input.join("delta.tsra"),
        )
        .unwrap();
        for (name, c) in [
            ("main", &main),
            ("fork", &fork),
            ("next", &next),
            ("merged", &merged),
            ("wrong", &wrong),
        ] {
            write(&input, name, c);
        }
        // A reader can verify the seal of an externally written fork with missing token;
        // the writer door must still refuse it. Rehashing here models those received bytes.
        let (bytes, _) = fork.to_stored().unwrap();
        let mut v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        v["id_inputs"].as_object_mut().unwrap().remove("fork");
        v["id"] = main.id.clone().into();
        let bytes = tessera_core::canonical::to_bytes(&v).unwrap();
        let bad = input.join("no-token");
        std::fs::create_dir(&bad).unwrap();
        std::fs::write(bad.join("collection.json"), &bytes).unwrap();
        std::fs::write(
            bad.join("collection.seal"),
            tessera_core::hash::digest(&bytes),
        )
        .unwrap();
        // Whitespace-only tokens have the same typed refusal as an absent token.
        v["id_inputs"]["fork"] = " \t".into();
        let mut blank = fork.clone();
        blank.id_inputs.insert("fork".into(), " \t".into());
        v["id"] = blank.recompute_id().unwrap().into();
        let bytes = tessera_core::canonical::to_bytes(&v).unwrap();
        let bad = input.join("blank-token");
        std::fs::create_dir(&bad).unwrap();
        std::fs::write(bad.join("collection.json"), &bytes).unwrap();
        std::fs::write(
            bad.join("collection.seal"),
            tessera_core::hash::digest(&bytes),
        )
        .unwrap();
        let mut text =
            std::fs::read_to_string(format!("tests/cmd/repositories/{name}.trycmd")).unwrap();
        for (key, value) in &vars {
            text = text.replace(key, value);
        }
        let case = tmp.path().join(format!("{name}.trycmd"));
        std::fs::write(&case, text).unwrap();
        cases.case(case);
    }
}

/// Exercise CLI rendering beyond the lifecycle's addition/removal and supply nested closure in-call.
#[test]
fn diff_repins_order_and_atomic_nested_members() {
    use std::process::Command;
    use tessera_core::collection::{CollectionHandle, Role};
    let dir = tempfile::tempdir().unwrap();
    let a = ProductBuilder::new("generic", "a", "", "2024-01-01T00:00:00Z")
        .seal()
        .unwrap();
    let b = ProductBuilder::new("generic", "b", "", "2024-01-01T00:00:00Z")
        .seal()
        .unwrap();
    let mut builder = ProductBuilder::from_manifest(&a).unwrap();
    builder.with_field("note", serde_json::json!("edited"));
    let a2 = builder.seal().unwrap();
    let old = collection(None, None, &[a.clone(), b.clone()]);
    let new = collection(Some(&old), None, &[b.clone(), a2.clone()]);
    let repo_path = dir.path().join("repo");
    let repo = tessera_io::Repository::init(&repo_path).unwrap();
    let old_seal = repo
        .commit_collection(
            &old.id,
            None,
            &old,
            &[
                tessera_io::NewMember::Product {
                    manifest: &a,
                    payloads: &[],
                },
                tessera_io::NewMember::Product {
                    manifest: &b,
                    payloads: &[],
                },
            ],
        )
        .unwrap();
    let new_seal = repo
        .commit_collection(
            &new.id,
            Some(&old_seal),
            &new,
            &[tessera_io::NewMember::Product {
                manifest: &a2,
                payloads: &[],
            }],
        )
        .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_tessera"))
        .args(["collection", "diff"])
        .arg(&repo_path)
        .args([&old_seal, &new_seal])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!(
            "~ {} {} -> {}\norder_changed: true\n",
            a.id,
            a.manifest_hash.as_ref().unwrap(),
            a2.manifest_hash.as_ref().unwrap()
        )
    );

    let nested = dir.path().join("nested");
    tessera_io::Repository::init(&nested).unwrap();
    let mut builder = CollectionBuilder::new("outer", "", "2024-01-01T00:00:00Z");
    builder.with_collection_schema("project").add_subcollection(
        &CollectionHandle::of(&old).unwrap(),
        Role::Derived,
        vec![],
    );
    let outer = builder.seal().unwrap();
    write(dir.path(), "outer", &outer);
    write(dir.path(), "child", &old);
    tessera_io::pack(&a, &[], &dir.path().join("a.tsra")).unwrap();
    tessera_io::pack(&b, &[], &dir.path().join("b.tsra")).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_tessera"))
        .args(["collection", "commit"])
        .arg(&nested)
        .arg(dir.path().join("outer/collection.json"))
        .arg("--subcollection")
        .arg(dir.path().join("child/collection.json"))
        .arg("--member")
        .arg(dir.path().join("a.tsra"))
        .arg("--member")
        .arg(dir.path().join("b.tsra"))
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let repo = tessera_io::Repository::open(&nested).unwrap();
    assert_eq!(repo.read_ref(&outer.id).unwrap(), outer.manifest_hash);
    assert_eq!(repo.gc().unwrap().kept, 4);
    assert_eq!(
        repo.get_manifest(a.manifest_hash.as_ref().unwrap())
            .unwrap()
            .id,
        a.id
    );
    assert_eq!(
        repo.get_collection(old.manifest_hash.as_ref().unwrap())
            .unwrap()
            .id,
        old.id
    );
}

/// Pin names are a JSON string followed by kind and seal, even with embedded line breaks.
#[test]
fn pin_listing_quotes_names() {
    let dir = tempfile::tempdir().unwrap();
    let repo = tessera_io::Repository::init(dir.path()).unwrap();
    let product = ProductBuilder::new("generic", "base", "", "2024-01-01T00:00:00Z")
        .seal()
        .unwrap();
    let address = repo.commit(&product.id, None, &product, &[]).unwrap();
    let names = ["a\nsecond-line", "a b", "a\t\r\u{0001}\"\\é"];
    for name in names {
        repo.set_pin(
            name,
            None,
            &tessera_io::repo::PinTarget {
                kind: tessera_core::ObjectKind::Product,
                address: address.clone(),
            },
        )
        .unwrap();
    }
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_tessera"))
        .args(["pin", "ls"])
        .arg(dir.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let mut names = names.to_vec();
    names.sort();
    let expected: String = names
        .iter()
        .map(|name| {
            format!(
                "{} product {address}\n",
                serde_json::to_string(name).unwrap()
            )
        })
        .collect();
    assert_eq!(String::from_utf8(output.stdout).unwrap(), expected);
}
