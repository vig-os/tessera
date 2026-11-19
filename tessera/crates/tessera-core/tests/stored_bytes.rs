//! The ADR-0066 stored-bytes API over every tracked product: `Manifest::to_stored` /
//! `from_stored`, `Collection::to_stored` / `from_stored`, and `canonical::check_canonical`.
//!
//! Two claims, both on real products:
//!
//! - **The seal keeps its value** (ADR-0066 rule 2). For all 13 tracked `.tsra` (12 distinct
//!   manifests: the two `study.tsra` fixtures are byte-identical), `blake3(to_stored().0)` equals
//!   the seal recorded in the product, and for the 8 corpus files also the `manifest_hash` golden
//!   in `corpus/corpus.json`. So the A′ seal equals today's seal for every existing product.
//! - **One seal, one byte string** (rules 1 and 4). The spike's respelling attacks
//!   (docs/spikes/897-seal-encoding/src/attacks.rs) replayed on each product's stored bytes: T4–T7
//!   under the old seal and S7/S8 sealed by the attacker are refused as non-canonical; S5b (an exact
//!   double beyond 2^53, its own JCS form) is refused by I-JSON; an edit under the old seal is a
//!   seal mismatch; an in-band `manifest_hash` key is refused.
//!
//! tessera-core has no zip dependency, so `manifest.json` is read straight from the STORED zip64
//! local headers (SPEC §6 requires STORED members with sizes in the header).

use std::path::Path;

use serde_json::{json, Value};
use tessera_core::collection::MemberKind;
use tessera_core::hash::digest;
use tessera_core::{canonical, Collection, CollectionMember, Error, Manifest, Role};

/// Every tracked `.tsra`, relative to the workspace (`git ls-files '*.tsra'`): 8 corpus files and
/// 5 test fixtures. A product added to the tree must be added here.
const PRODUCTS: [&str; 13] = [
    "corpus/files/blob_opaque.tsra",
    "corpus/files/empty_no_blocks.tsra",
    "corpus/files/listmode_events.tsra",
    "corpus/files/multiblock_study.tsra",
    "corpus/files/recon_float32_mumap.tsra",
    "corpus/files/recon_int16.tsra",
    "corpus/files/segmentation_uint8.tsra",
    "corpus/files/spectrum_lifetime.tsra",
    "crates/tessera-cli/tests/cmd/logical_tables.in/scan.tsra",
    "crates/tessera-cli/tests/cmd/persona_signing.in/study.tsra",
    "crates/tessera-cli/tests/cmd/versioning.in/study.tsra",
    "crates/tessera-io/tests/fixtures/legacy_flat_events.tsra",
    "crates/tessera-py/tests/fixtures/partitioned_pairs.tsra",
];

fn workspace(rel: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn le16(b: &[u8], at: usize) -> usize {
    u16::from_le_bytes([b[at], b[at + 1]]) as usize
}

fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

fn le64(b: &[u8], at: usize) -> usize {
    usize::try_from(u64::from_le_bytes(b[at..at + 8].try_into().unwrap())).unwrap()
}

/// The bytes of the STORED member `name`, found by walking the local file headers from the start.
/// Sizes come from the header, or from its zip64 extra field (`0x0001`: uncompressed, then
/// compressed size) when the header holds `0xFFFFFFFF`.
fn stored_member(zip: &[u8], name: &str) -> Vec<u8> {
    let mut at = 0;
    while le32(zip, at) == 0x0403_4b50 {
        let (flags, method) = (le16(zip, at + 6), le16(zip, at + 8));
        assert_eq!(flags & 0x8, 0, "a data descriptor: sizes not in the header");
        assert_eq!(method, 0, "every .tsra member is STORED (SPEC §6)");
        let (name_len, extra_len) = (le16(zip, at + 26), le16(zip, at + 28));
        let member = &zip[at + 30..at + 30 + name_len];
        let extra = &zip[at + 30 + name_len..at + 30 + name_len + extra_len];
        let mut size = le32(zip, at + 18) as usize;
        if size == 0xFFFF_FFFF {
            let mut e = 0;
            while le16(extra, e) != 0x0001 {
                e += 4 + le16(extra, e + 2);
            }
            // Uncompressed, then compressed; equal for STORED.
            size = le64(extra, e + 12);
        }
        let data = at + 30 + name_len + extra_len;
        if member == name.as_bytes() {
            return zip[data..data + size].to_vec();
        }
        at = data + size;
    }
    panic!("no STORED member {name}");
}

/// One tracked product: its `manifest.json` as stored today and the in-band seal it records.
struct Product {
    path: &'static str,
    text: String,
    seal: String,
}

fn products() -> Vec<Product> {
    PRODUCTS
        .iter()
        .map(|&path| {
            let zip = std::fs::read(workspace(path)).unwrap_or_else(|e| panic!("{path}: {e}"));
            let text = String::from_utf8(stored_member(&zip, "manifest.json")).unwrap();
            let seal = Manifest::from_json_verified(&text)
                .unwrap_or_else(|e| panic!("{path} verifies in-band: {e}"))
                .manifest_hash
                .unwrap();
            Product { path, text, seal }
        })
        .collect()
}

/// The `manifest_hash` goldens of `corpus/corpus.json`, by product name.
fn goldens() -> Vec<(String, String)> {
    let text = std::fs::read_to_string(workspace("corpus/corpus.json")).unwrap();
    let doc: Value = serde_json::from_str(&text).unwrap();
    doc.as_array()
        .unwrap()
        .iter()
        .map(|e| {
            let field = |k: &str| e[k].as_str().unwrap().to_owned();
            (field("name"), field("manifest_hash"))
        })
        .collect()
}

#[test]
fn the_stored_seal_of_every_tracked_product_is_its_recorded_seal() {
    let products = products();
    assert_eq!(products.len(), 13);
    let mut distinct: Vec<&str> = products.iter().map(|p| p.text.as_str()).collect();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        12,
        "the two study.tsra fixtures are one manifest"
    );

    for p in &products {
        let m = Manifest::from_json_verified(&p.text).unwrap();
        let (bytes, seal) = m.to_stored().unwrap_or_else(|e| panic!("{}: {e}", p.path));
        assert_eq!(digest(&bytes), seal, "{}", p.path);
        assert_eq!(seal, p.seal, "{}: the A′ seal is the recorded seal", p.path);
        // The stored bytes carry no seal key and are their own JCS form.
        let object = canonical::check_canonical(&bytes).unwrap();
        assert!(object.get("manifest_hash").is_none(), "{}", p.path);
        // Round trip: the stored form reads back as the same manifest, judged by its bytes. (Not by
        // struct `==`: JCS has one number type, so a view's `1.0` is stored as `1` and reads back as
        // an integer `Value`; the bytes and the seal are what the format pins.)
        let back = Manifest::from_stored(&bytes, &seal).unwrap();
        assert_eq!(back.manifest_hash.as_ref(), Some(&seal), "{}", p.path);
        assert_eq!(back.to_stored().unwrap(), (bytes, seal), "{}", p.path);
    }

    // The 8 corpus files against their committed goldens, each by name (no proxy count).
    let goldens = goldens();
    assert_eq!(goldens.len(), 8);
    for (name, golden) in &goldens {
        let path = format!("corpus/files/{name}.tsra");
        let p = products
            .iter()
            .find(|p| p.path == path)
            .unwrap_or_else(|| panic!("{path}"));
        let (_, seal) = Manifest::from_json_verified(&p.text)
            .unwrap()
            .to_stored()
            .unwrap();
        assert_eq!(&seal, golden, "{name}: corpus.json golden");
    }
}

// ------------------------------------------------------------------ the attack replay

/// The product's stored bytes and seal, as `to_stored` writes them.
fn stored(p: &Product) -> (String, String) {
    let (bytes, seal) = Manifest::from_json_verified(&p.text)
        .unwrap()
        .to_stored()
        .unwrap();
    (String::from_utf8(bytes).unwrap(), seal)
}

fn expect_non_canonical(path: &str, case: &str, bytes: &str, seal: &str) -> usize {
    match Manifest::from_stored(bytes.as_bytes(), seal) {
        Err(Error::NonCanonical { offset, .. }) => offset,
        other => panic!("{path} {case}: expected NonCanonical, got {other:?}"),
    }
}

/// The JCS bytes of `object` without any I-JSON check: what an attacker's own tool writes.
fn raw_jcs(object: &Value) -> String {
    String::from_utf8(serde_jcs::to_vec(object).unwrap()).unwrap()
}

/// The first number after `"shape":`, respelled as a float (`64` → `64.0`): T7/S7.
fn respell_shape(text: &str) -> Option<(String, usize)> {
    let k = text.find("\"shape\"")?;
    let start = k + text[k..].find(|c: char| c.is_ascii_digit())?;
    let end = start + text[start..].find(|c: char| !c.is_ascii_digit())?;
    Some((format!("{}.0{}", &text[..end], &text[end..]), end))
}

#[test]
fn every_respelling_of_a_stored_product_is_refused_as_non_canonical() {
    let mut shape_products = 0;
    for p in &products() {
        let (ap, seal) = stored(p);
        let object: Value = serde_json::from_str(&ap).unwrap();

        // T4: the top-level members in reverse order, old seal.
        let Value::Object(members) = &object else {
            unreachable!()
        };
        let reversed: Vec<String> = members
            .iter()
            .rev()
            .map(|(k, v)| format!("{}:{}", serde_json::to_string(k).unwrap(), raw_jcs(v)))
            .collect();
        let t4 = format!("{{{}}}", reversed.join(","));
        assert_eq!(serde_json::from_str::<Value>(&t4).unwrap(), object);
        expect_non_canonical(p.path, "T4", &t4, &seal);

        // T5: one space after the opening brace, old seal.
        let t5 = format!("{{ {}", &ap[1..]);
        assert_eq!(expect_non_canonical(p.path, "T5", &t5, &seal), 1);

        // T6: the first character of `tessera_version`'s value as a `\u` escape, old seal.
        let q = ap.find("\"tessera_version\":\"").unwrap() + "\"tessera_version\":\"".len();
        let c = ap[q..].chars().next().unwrap();
        let t6 = format!("{}\\u{:04x}{}", &ap[..q], c as u32, &ap[q + c.len_utf8()..]);
        assert_eq!(expect_non_canonical(p.path, "T6", &t6, &seal), q);

        // T7 (old seal) and S7 (sealed by the attacker): an array shape `64` written as `64.0`.
        if let Some((respelled, dot)) = respell_shape(&ap) {
            shape_products += 1;
            assert_eq!(expect_non_canonical(p.path, "T7", &respelled, &seal), dot);
            let s7_seal = digest(respelled.as_bytes());
            assert_eq!(
                expect_non_canonical(p.path, "S7", &respelled, &s7_seal),
                dot
            );
        }

        // S8: the pretty layout, sealed by the attacker.
        let s8 = serde_json::to_string_pretty(&object).unwrap();
        assert_eq!(
            expect_non_canonical(p.path, "S8", &s8, &digest(s8.as_bytes())),
            1
        );
    }
    // The spike's count of products with an array block (T7/S7 apply to those only).
    assert_eq!(shape_products, 7);
}

#[test]
fn s5b_an_exact_double_beyond_2_53_is_refused_by_i_json_not_the_byte_comparison() {
    for p in &products() {
        let (ap, _) = stored(p);
        let mut object: Value = serde_json::from_str(&ap).unwrap();
        let md = object
            .as_object_mut()
            .unwrap()
            .entry("metadata")
            .or_insert_with(|| json!({}));
        md["daq_start_ns"] = json!(1_730_000_000_123_456_800_u64);
        let s5b = raw_jcs(&object);
        // The byte comparison alone would pass it: the attacker's raw JCS is its own JCS form.
        let reparsed: Value = serde_json::from_str(&s5b).unwrap();
        assert_eq!(raw_jcs(&reparsed), s5b, "{}", p.path);
        match Manifest::from_stored(s5b.as_bytes(), &digest(s5b.as_bytes())) {
            Err(Error::InexactInteger { path, .. }) => {
                assert_eq!(path, "metadata.daq_start_ns", "{}", p.path)
            }
            other => panic!("{}: expected InexactInteger, got {other:?}", p.path),
        }
    }
}

#[test]
fn an_edit_under_the_old_seal_is_a_seal_mismatch_naming_both_values() {
    for p in &products() {
        let (ap, seal) = stored(p);
        let mut object: Value = serde_json::from_str(&ap).unwrap();
        let name = format!("{}-edited", object["name"].as_str().unwrap());
        object["name"] = json!(name);
        let edited = raw_jcs(&object);
        match Manifest::from_stored(edited.as_bytes(), &seal) {
            Err(Error::Integrity {
                what,
                expected,
                actual,
            }) => {
                assert_eq!(what, "manifest_hash");
                assert_eq!(expected, seal);
                assert_eq!(actual, digest(edited.as_bytes()));
            }
            other => panic!("{}: expected Integrity, got {other:?}", p.path),
        }
    }
}

#[test]
fn the_in_band_layout_is_refused_by_from_stored() {
    for p in &products() {
        // Today's object, as JCS with its `manifest_hash`, sealed over those very bytes.
        let object: Value = serde_json::from_str(&p.text).unwrap();
        let in_band = raw_jcs(&object);
        let e = Manifest::from_stored(in_band.as_bytes(), &digest(in_band.as_bytes())).unwrap_err();
        assert!(
            matches!(
                e,
                Error::InBandSeal {
                    document: "manifest"
                }
            ),
            "{}: {e:?}",
            p.path
        );
        // Today's stored text (#912: writers emit JCS) is exactly that in-band JCS form, so it is
        // canonical and refused for its `manifest_hash` key, even under its own recorded seal.
        assert_eq!(p.text, in_band, "{}: stored as JCS", p.path);
        let e = Manifest::from_stored(p.text.as_bytes(), &p.seal).unwrap_err();
        assert!(
            matches!(
                e,
                Error::InBandSeal {
                    document: "manifest"
                }
            ),
            "{}: {e:?}",
            p.path
        );
    }
}

#[test]
fn the_seal_is_compared_byte_for_byte() {
    let p = &products()[0];
    let (ap, seal) = stored(p);
    for other in [
        seal.to_uppercase(),
        seal.trim_start_matches("blake3:").to_owned(),
        String::new(),
    ] {
        let e = Manifest::from_stored(ap.as_bytes(), &other).unwrap_err();
        assert!(
            matches!(
                e,
                Error::Integrity {
                    what: "manifest_hash",
                    ..
                }
            ),
            "{other:?}: {e:?}"
        );
    }
}

// ------------------------------------------------------------------ to_stored refusals

#[test]
fn to_stored_refuses_what_from_stored_would_refuse() {
    let p = &products()[0];
    let m = Manifest::from_json_verified(&p.text).unwrap();

    // Without a content_hash, a manifest is not a sealed product.
    let mut unsealed = m.clone();
    unsealed.content_hash = None;
    unsealed.manifest_hash = None;
    assert!(matches!(unsealed.to_stored(), Err(Error::Invalid(_))));

    // A manifest_hash that is not the seal of the struct: changed after sealing.
    let mut edited = m.clone();
    edited.description.push_str(" (edited)");
    match edited.to_stored() {
        Err(Error::Integrity {
            what: "manifest_hash",
            expected,
            ..
        }) => assert_eq!(expected, p.seal),
        other => panic!("expected Integrity, got {other:?}"),
    }
    // With no manifest_hash, the seal is computed: an unset seal is not required.
    edited.manifest_hash = None;
    let (bytes, seal) = edited.to_stored().unwrap();
    assert_ne!(seal, p.seal);
    assert_eq!(
        Manifest::from_stored(&bytes, &seal).unwrap().description,
        edited.description
    );

    // Canonical bytes the view normalises (an explicit `"study": null`) verify as sealed, but the
    // view no longer has the sealed bytes, so it can't be re-emitted.
    let (ap, _) = stored(p);
    let mut object: Value = serde_json::from_str(&ap).unwrap();
    assert!(object.get("study").is_none(), "{}", p.path);
    object["study"] = Value::Null;
    let normalised = raw_jcs(&object);
    let read =
        Manifest::from_stored(normalised.as_bytes(), &digest(normalised.as_bytes())).unwrap();
    assert!(matches!(
        read.to_stored(),
        Err(Error::Integrity {
            what: "manifest_hash",
            ..
        })
    ));

    // A parsed manifest with newer-format fields can't be re-emitted from the view.
    let mut object: Value = serde_json::from_str(&ap).unwrap();
    object["actors"] = json!([]);
    let newer = raw_jcs(&object);
    let read = Manifest::from_stored(newer.as_bytes(), &digest(newer.as_bytes())).unwrap();
    assert_eq!(read.newer_fields(), ["actors"]);
    assert!(matches!(
        read.to_stored(),
        Err(Error::NewerFormatFields { .. })
    ));
}

// ------------------------------------------------------------------ collections

fn sealed_collection() -> Collection {
    let mut cb = tessera_core::CollectionBuilder::new("DP06", "a study", "2024-01-01T00:00:00Z");
    cb.add_member(
        "raw",
        format!("blake3:{}", "1".repeat(64)),
        Role::Raw,
        Vec::new(),
    );
    cb.add_member(
        "recon",
        format!("blake3:{}", "2".repeat(64)),
        Role::Derived,
        vec!["raw".into()],
    );
    cb.seal().unwrap()
}

#[test]
fn a_collection_round_trips_through_its_stored_form_and_keeps_its_seal() {
    let c = sealed_collection();
    let (bytes, seal) = c.to_stored().unwrap();
    // The in-band seal and the stored seal are one value (ADR-0066 rules 2 and 5).
    assert_eq!(Some(&seal), c.manifest_hash.as_ref());
    assert_eq!(digest(&bytes), seal);
    let object = canonical::check_canonical(&bytes).unwrap();
    assert!(object.get("manifest_hash").is_none());
    // The member pins keep their own `manifest_hash`: only the top-level key moves out.
    assert!(object["members"][0]["manifest_hash"].is_string());
    let back = Collection::from_stored(&bytes, &seal).unwrap();
    assert_eq!(back, c);
    assert_eq!(back.to_stored().unwrap(), (bytes, seal));
}

#[test]
fn a_collection_respelled_edited_or_in_band_is_refused() {
    let c = sealed_collection();
    let (bytes, seal) = c.to_stored().unwrap();
    let text = String::from_utf8(bytes).unwrap();
    let object: Value = serde_json::from_str(&text).unwrap();

    let pretty = serde_json::to_string_pretty(&object).unwrap();
    let e = Collection::from_stored(pretty.as_bytes(), &digest(pretty.as_bytes())).unwrap_err();
    assert!(matches!(e, Error::NonCanonical { offset: 1, .. }), "{e:?}");

    let mut edited = object.clone();
    edited["description"] = json!("another study");
    let edited = raw_jcs(&edited);
    let e = Collection::from_stored(edited.as_bytes(), &seal).unwrap_err();
    assert!(
        matches!(
            e,
            Error::Integrity {
                what: "manifest_hash",
                ..
            }
        ),
        "{e:?}"
    );

    let in_band = raw_jcs(&serde_json::to_value(&c).unwrap());
    let e = Collection::from_stored(in_band.as_bytes(), &digest(in_band.as_bytes())).unwrap_err();
    assert!(
        matches!(
            e,
            Error::InBandSeal {
                document: "collection"
            }
        ),
        "{e:?}"
    );

    // The typed layer runs too: a `dataset` holding a sub-collection, honestly hashed and sealed,
    // passes every hash and is refused by the level schema's member rule.
    let mut wrong = c.clone();
    wrong.collection_schema = "dataset".into();
    wrong.members[0] =
        CollectionMember::new("sub", format!("blake3:{}", "3".repeat(64)), Role::Raw)
            .with_kind(MemberKind::Collection);
    wrong.content_hash = Some(wrong.recompute_content_hash());
    let mut wrong = serde_json::to_value(&wrong).unwrap();
    wrong.as_object_mut().unwrap().remove("manifest_hash");
    let wrong = raw_jcs(&wrong);
    let e = Collection::from_stored(wrong.as_bytes(), &digest(wrong.as_bytes())).unwrap_err();
    assert!(matches!(e, Error::Invalid(_)), "{e:?}");
}
