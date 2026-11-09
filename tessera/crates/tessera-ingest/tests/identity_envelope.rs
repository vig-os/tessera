//! An ADR-0047 envelope written through `tessera-identity` and `tessera-io` alone decrypts through
//! `tessera-ingest`'s reader, and the re-export names the same types (#762).
//!
//! This test is built inside `tessera-ingest`, so it cannot itself show that the producer half builds
//! without that crate. The `identity-deps` flake check does: `tessera-identity`'s graph has no hdf5.

use tessera_io::conformance::fixtures;
use tessera_io::{add_aux_members, pack, AuxMember, Reader};

/// The producer half. Every path it names is in `tessera_identity` or `tessera_io`.
fn produce(path: &std::path::Path, recipient: &str) -> tessera_identity::IdentityDocument {
    let f = fixtures().into_iter().next().unwrap();
    pack(&f.manifest, &f.payloads, path).unwrap();
    let doc = tessera_identity::IdentityDocument {
        v: tessera_identity::IdentityDocument::VERSION,
        dicom_header: serde_json::json!({"0010,0010": {"vr": "PN", "value": ["SMITH^JOHN"]}}),
        curated_identifying: serde_json::Map::new(),
    };
    let recipient = tessera_identity::parse_recipient(recipient).unwrap();
    let envelope = tessera_identity::encrypt_identity(&doc, &[recipient]).unwrap();
    add_aux_members(
        path,
        &[AuxMember::new(
            tessera_identity::AUX_IDENTITY_NAME,
            envelope,
        )],
    )
    .unwrap();
    doc
}

#[test]
fn an_envelope_written_without_ingest_decrypts_through_ingest() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("product.tsra");
    let (secret, public) = tessera_ingest::identity::generate_keypair();
    let written = produce(&path, &public);

    let mut reader = Reader::open(&path).unwrap();
    let envelope = reader
        .read_aux(tessera_ingest::identity::AUX_IDENTITY_NAME)
        .unwrap();
    let read: tessera_ingest::identity::IdentityDocument =
        tessera_ingest::identity::decrypt_identity(&envelope, &secret).unwrap();
    // The re-export names the same type, so the two documents compare directly.
    assert_eq!(read, written);
    // The aux member rides outside the seal: `open` re-checked the seal, and every payload still verifies.
    reader.verify_payloads("product.tsra").unwrap();
}
