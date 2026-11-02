//! Integration test for the `tessera inspect <URL>` / `verify <URL>` cloud entry points (#225).
//!
//! Drives the real binary (resolved via `CARGO_BIN_EXE_tessera`) against an S3 server
//! seeded with a sealed `.tsra` — the binary opens it over the wire via `tessera_io::open_url`,
//! verifies the seal + every block digest, and exits 0. Skipped when `TESSERA_S3_ENDPOINT` is
//! unset so local `cargo test --features cloud` without a server doesn't fail, unless
//! `TESSERA_S3_REQUIRED=1` says a server was promised (see `s3_endpoint`); the
//! `s3-range-read` flake check (versitygw on loopback) is the authoritative runner.

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

/// The S3 endpoint, or `None` to skip, from the `TESSERA_S3_REQUIRED` / `TESSERA_S3_ENDPOINT`
/// reads. A copy of `tessera_io::cloud::tests::s3_endpoint` (#707), which documents the
/// required/skip contract in full; the two cannot share code without new public API. In short:
/// without an endpoint the test skips, unless `TESSERA_S3_REQUIRED=1` (set by the
/// `s3-range-read` flake check) promised a server, in which case it panics instead of passing
/// without testing. Any other marker value, or an endpoint that is set but not UTF-8, panics
/// too, so neither a typo nor an unreadable value can turn into a skip.
fn s3_endpoint(
    required: Result<String, std::env::VarError>,
    endpoint: Result<String, std::env::VarError>,
) -> Option<String> {
    let required = match required {
        Err(std::env::VarError::NotPresent) => false,
        Ok(v) if v == "1" => true,
        other => panic!("TESSERA_S3_REQUIRED must be unset or `1`, got {other:?}"),
    };
    match endpoint {
        Ok(v) => Some(v),
        Err(std::env::VarError::NotPresent) => {
            assert!(
                !required,
                "TESSERA_S3_REQUIRED=1 but TESSERA_S3_ENDPOINT is not set: the S3 server this \
                 test was promised is missing, so it would pass without testing"
            );
            None
        }
        Err(e) => panic!("TESSERA_S3_ENDPOINT is set but unreadable: {e}"),
    }
}

/// Names carry `cloud_` so the `s3-range-read` check's `cloud` filter runs them next to the test
/// they guard.
#[test]
fn cloud_s3_endpoint_skips_only_when_not_required() {
    use std::env::VarError::NotPresent;
    assert_eq!(s3_endpoint(Err(NotPresent), Err(NotPresent)), None);
    assert_eq!(
        s3_endpoint(Ok("1".into()), Ok("http://127.0.0.1:1".into())).as_deref(),
        Some("http://127.0.0.1:1")
    );
}

#[test]
#[should_panic(expected = "TESSERA_S3_REQUIRED=1 but TESSERA_S3_ENDPOINT is not set")]
fn cloud_s3_endpoint_required_without_endpoint_panics() {
    s3_endpoint(Ok("1".into()), Err(std::env::VarError::NotPresent));
}

/// End-to-end CLI cloud test: seed a `.tsra` to S3, then `tessera verify s3://<bucket>/<key>`
/// must exit 0 with the same "OK ... verified" output the local-path verify produces.
///
/// **Name carries `cloud_`** so a single `cargo nextest run ... cloud` filter (the flake check)
/// catches both this binary's tests and the `cloud::*` tessera-io unit tests in one pass.
#[test]
fn cloud_cli_inspect_and_verify_over_s3_url() {
    let Some(endpoint) = s3_endpoint(
        std::env::var("TESSERA_S3_REQUIRED"),
        std::env::var("TESSERA_S3_ENDPOINT"),
    ) else {
        eprintln!("skipping: TESSERA_S3_ENDPOINT not set (run under flake check)"); // guardrails-ok: deliberate conditional-skip notice
        return;
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
