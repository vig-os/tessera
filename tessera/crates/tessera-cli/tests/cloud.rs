//! Integration test for the `tessera inspect <URL>` / `verify <URL>` cloud entry points (#225).
//!
//! Drives the real binary (resolved via `CARGO_BIN_EXE_tessera`) against a MinIO instance
//! seeded with a sealed `.tsra` — the binary opens it over the wire via `tessera_io::open_url`,
//! verifies the seal + every block digest, and exits 0. Skipped when `TESSERA_S3_ENDPOINT` is
//! unset so local `cargo test --features cloud` without MinIO doesn't fail; the
//! `minio-range-read` flake check is the authoritative runner.

#![cfg(feature = "cloud")]

use std::path::Path;
use std::process::Command;

use tessera_core::block::array::ArraySpec;
use tessera_core::ProductBuilder;
use tessera_io::array::{self, ArrayData};
use tessera_io::pack;

fn tessera() -> Command {
    Command::new(env!("CARGO_BIN_EXE_tessera"))
}

/// Same fixture as `tests/sign.rs::sealed_tsra` (small int16 volume), kept inline so this file
/// stays self-contained and the test deps stay minimal.
fn sealed_tsra(out: &Path) {
    let mut spec = ArraySpec::new(vec![8, 8, 8], "int16");
    spec.codec = "pcodec".into();
    let data = ArrayData::I16((0..512).map(|k| k as i16).collect());
    let (bref, payload) = array::array_block("volume", &spec, &data).unwrap();
    let mut b = ProductBuilder::new("recon", "DPcloud", "d", "2024-01-01T00:00:00Z");
    b.add_block_ref(bref);
    let manifest = b.seal().unwrap();
    pack(&manifest, &[payload], out).unwrap();
}

/// End-to-end CLI cloud test: seed a `.tsra` to MinIO, then `tessera verify s3://<bucket>/<key>`
/// must exit 0 with the same "OK ... verified" output the local-path verify produces.
///
/// **Name carries `cloud_`** so a single `cargo nextest run ... cloud` filter (the flake check)
/// catches both this binary's tests and the `cloud::*` tessera-io unit tests in one pass.
#[test]
fn cloud_cli_inspect_and_verify_over_s3_url() {
    let endpoint = match std::env::var("TESSERA_S3_ENDPOINT") {
        Ok(v) => v,
        Err(_) => {
            eprintln!("skipping: TESSERA_S3_ENDPOINT not set (run under flake check)"); // guardrails-ok: deliberate conditional-skip notice
            return;
        }
    };
    let bucket = std::env::var("TESSERA_S3_BUCKET").unwrap();
    // Seal a `.tsra` locally then upload it via the same object_store builder open_url uses, so
    // the test exercises the same auth/endpoint env-resolution the CLI relies on at runtime.
    let dir = tempfile::tempdir().unwrap();
    let local = dir.path().join("cli-cloud.tsra");
    sealed_tsra(&local);

    // Upload via object_store directly (the CLI is read-only; pushing is out of scope).
    use object_store::aws::AmazonS3Builder;
    use object_store::path::Path as ObjPath;
    use object_store::{ObjectStore, ObjectStoreExt, PutPayload};
    use std::sync::Arc;
    use tokio::runtime::Builder as RtBuilder;
    let region = std::env::var("AWS_REGION").unwrap_or_else(|_| "us-east-1".into());
    let access = std::env::var("AWS_ACCESS_KEY_ID").unwrap();
    let secret = std::env::var("AWS_SECRET_ACCESS_KEY").unwrap();
    let store: Arc<dyn ObjectStore> = Arc::new(
        AmazonS3Builder::new()
            .with_endpoint(&endpoint)
            .with_bucket_name(&bucket)
            .with_region(region)
            .with_access_key_id(access)
            .with_secret_access_key(secret)
            .with_allow_http(true)
            .build()
            .unwrap(),
    );
    let key = ObjPath::from("cli/cloud.tsra");
    let rt = RtBuilder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let bytes = std::fs::read(&local).unwrap();
    rt.block_on(store.put(&key, PutPayload::from(bytes)))
        .unwrap();

    // `tessera verify s3://...` — uses the same TESSERA_S3_ENDPOINT / AWS_* env the test runner set.
    let url = format!("s3://{bucket}/cli/cloud.tsra");
    let out = tessera().args(["verify", &url]).output().unwrap();
    assert!(
        out.status.success(),
        "verify {url} failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("verified (1 blocks)"),
        "expected 'verified (1 blocks)' in stdout, got: {stdout}",
    );

    // `tessera inspect s3://...` — must print the same manifest summary as a local read.
    let out = tessera().args(["inspect", &url]).output().unwrap();
    assert!(out.status.success(), "inspect {url} failed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("product=recon") && stdout.contains("DPcloud"),
        "expected product/name in inspect output, got: {stdout}"
    );
}
