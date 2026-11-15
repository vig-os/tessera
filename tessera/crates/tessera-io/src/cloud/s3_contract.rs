//! Test-only: the S3 tests' **required/skip contract** (#707). Not part of the supported API.
//!
//! ONE source for both crates that run S3 tests (#726): tessera-io's `cloud.rs` declares it as the
//! private `cfg(test)` child module `cloud::s3_contract` (its `cloud::tests` import from it), and
//! tessera-cli's `tests/cloud.rs` compiles this same file via `#[path]`. An integration test cannot reach a `cfg(test)` item, and
//! the alternatives cost more than they buy: a public (even `#[doc(hidden)]`) helper would put test
//! policy in tessera-io's API, and a test-support crate or feature adds a dependency edge that
//! churns Gate B's feature snapshots (the reasoning `tests/encode_trace_interest.rs` gives for
//! `test_support.rs`, where the same `#[path]` technique stays within one crate). The cost of `#[path]`: tessera-cli's tests reach outside their package, so
//! they only build inside this workspace. The crates are not published to crates.io (see
//! `release-plz.toml`), so nothing builds them outside it.
//!
//! The file therefore compiles standalone, from `std` alone, in either crate. Its unit tests run in
//! both, and every test name carries `cloud` (tessera-io's module path, tessera-cli's
//! `cloud_s3_contract` module), so the `s3-range-read` flake check's `cloud` filter runs them next
//! to the S3 tests they guard.

use std::env::VarError;

/// Set to `1` by the `s3-range-read` flake check, which starts an S3 server for these tests.
pub(crate) const S3_REQUIRED_ENV: &str = "TESSERA_S3_REQUIRED";

/// The S3 server's URL, exported by the `s3-range-read` flake check once the server is up.
pub(crate) const S3_ENDPOINT_ENV: &str = "TESSERA_S3_ENDPOINT";

/// The contract as a pure function of the two env reads (`TESSERA_S3_REQUIRED`,
/// `TESSERA_S3_ENDPOINT`), so it is unit-tested without mutating the process environment:
///
/// | `TESSERA_S3_REQUIRED` | `TESSERA_S3_ENDPOINT` | outcome                               |
/// |-----------------------|-----------------------|---------------------------------------|
/// | unset                 | unset                 | `None`: skip (plain `cargo test`)     |
/// | unset or `1`          | set                   | `Some(endpoint)`: run against it      |
/// | `1`                   | unset                 | **panic**: the server was promised    |
/// | any                   | set, not UTF-8        | **panic**: set is never "unset"       |
/// | any other value       | any                   | **panic**: a typo must not mean skip  |
///
/// Why a marker and not "skip iff the endpoint is unset": the flake check exports the endpoint
/// at runtime from `preCheck`, because the port is only known once the server is up. If that
/// export ever broke, every S3 test would early-return and nextest would report it PASS, so
/// the check would be green while testing nothing. `TESSERA_S3_REQUIRED` is a static
/// derivation attribute, independent of `preCheck`, so a lost endpoint is a red build.
pub(crate) fn s3_endpoint(
    required: Result<String, VarError>,
    endpoint: Result<String, VarError>,
) -> Option<String> {
    let required = match required {
        Err(VarError::NotPresent) => false,
        Ok(v) if v == "1" => true,
        other => panic!("{S3_REQUIRED_ENV} must be unset or `1`, got {other:?}"),
    };
    match endpoint {
        Ok(v) => Some(v),
        Err(VarError::NotPresent) => {
            assert!(
                !required,
                "{S3_REQUIRED_ENV}=1 but {S3_ENDPOINT_ENV} is not set: the S3 server these \
                 tests were promised is missing, so they would pass without testing"
            );
            None
        }
        Err(e) => panic!("{S3_ENDPOINT_ENV} is set but unreadable: {e}"),
    }
}

/// [`s3_endpoint`] over this process's environment: the endpoint to run `test` against, or `None`
/// after printing a skip notice, so local `cargo test --features cloud` without an S3 server
/// doesn't fail. Panics where [`s3_endpoint`] does.
pub(crate) fn s3_endpoint_or_skip(test: &str) -> Option<String> {
    let Some(endpoint) = s3_endpoint(
        std::env::var(S3_REQUIRED_ENV),
        std::env::var(S3_ENDPOINT_ENV),
    ) else {
        eprintln!("skipping {test}: {S3_ENDPOINT_ENV} not set (run under flake check)"); // guardrails-ok: deliberate conditional-skip notice, not a debug leftover
        return None;
    };
    Some(endpoint)
}

#[cfg(test)]
mod tests {
    use super::s3_endpoint;
    use std::env::VarError::{NotPresent, NotUnicode};

    #[test]
    fn s3_endpoint_skips_only_when_not_required() {
        assert_eq!(s3_endpoint(Err(NotPresent), Err(NotPresent)), None);
        let ep = || Ok("http://127.0.0.1:1".to_owned());
        assert_eq!(
            s3_endpoint(Err(NotPresent), ep()).as_deref(),
            Some("http://127.0.0.1:1")
        );
        assert_eq!(
            s3_endpoint(Ok("1".into()), ep()).as_deref(),
            Some("http://127.0.0.1:1")
        );
    }

    #[test]
    #[should_panic(expected = "TESSERA_S3_REQUIRED=1 but TESSERA_S3_ENDPOINT is not set")]
    fn s3_endpoint_required_without_endpoint_panics() {
        s3_endpoint(Ok("1".into()), Err(NotPresent));
    }

    #[test]
    #[should_panic(expected = "TESSERA_S3_REQUIRED must be unset or `1`")]
    fn s3_endpoint_rejects_a_misspelled_required_marker() {
        s3_endpoint(Ok("true".into()), Err(NotPresent));
    }

    #[test]
    #[should_panic(expected = "TESSERA_S3_REQUIRED must be unset or `1`")]
    fn s3_endpoint_rejects_a_misspelled_required_marker_even_with_an_endpoint() {
        s3_endpoint(Ok("true".into()), Ok("http://127.0.0.1:1".into()));
    }

    #[test]
    #[should_panic(expected = "TESSERA_S3_ENDPOINT is set but unreadable")]
    fn s3_endpoint_rejects_a_non_utf8_endpoint_instead_of_skipping() {
        s3_endpoint(Err(NotPresent), Err(NotUnicode("\u{fffd}".into())));
    }
}
