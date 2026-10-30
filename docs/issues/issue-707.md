---
type: issue
state: open
created: 2026-10-30T00:56:02Z
updated: 2026-10-30T00:56:02Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/707
comments: 0
labels: bug
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-30T08:02:19.567Z
---

# [Issue 707]: [test(io): S3 cloud tests pass vacuously when TESSERA_S3_ENDPOINT is unset in the s3-range-read check](https://github.com/vig-os/tessera/issues/707)

## Problem
`tessera-io/src/cloud.rs` `s3_store_or_skip` (≈:436) returns early when `TESSERA_S3_ENDPOINT` is unset, and nextest
reports the test as PASS. Inside the `s3-range-read` flake check, the variable comes from the check's `preCheck`. If
that export ever breaks (a rename, a refactor, a hook-order change), all 6 S3 tests go green while testing nothing.
The only signal today is the `versitygw on http://…` echo in the log. This predates #702 and was found in the fresh
review of PR #706.

## Root-cause fix
Make the skip impossible where the server is supposed to exist. For example, the flake check exports a "required"
marker (`TESSERA_S3_REQUIRED=1`), and `s3_store_or_skip` panics if the marker is set but the endpoint is missing.
Or the check asserts that nextest actually ran the S3 tests rather than early-returning them. A plain `cargo test`
outside the check keeps skipping. Add a test that fails before the fix: run the check with the endpoint export
removed, and it must go red.

## Nit (same area)
`flake.nix` ≈:932 brings in `python3` only to pick a free port. Consider a lighter way, e.g. letting versitygw bind
:0 and reading the port back, if versitygw supports that, or a small shell/socat idiom. Keep the
fail-hard-on-port-theft property the review verified.

