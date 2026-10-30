---
type: issue
state: closed
created: 2026-10-29T11:40:17Z
updated: 2026-10-30T00:55:50Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/702
comments: 2
labels: area:ci
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-30T08:02:20.958Z
---

# [Issue 702]: [test(io): run the S3 range-read check against versitygw (the fleet's S3), not MinIO](https://github.com/vig-os/tessera/issues/702)

The `minio-range-read` flake check (`flake.nix:831-890`) proves tessera's cloud reads against a MinIO server on loopback: the tail prefetch replaces the EOCD and central-directory GETs, and blocks a cohort query doesn't need are never fetched. Tessera's client is generic S3 (`object_store`'s `AmazonS3Builder` in `tessera-io/src/cloud.rs`), so the server is only a fixture. MinIO is the wrong one:

- **It isn't what we run.** The fleet's S3 is versitygw (exo-pet/exo-fleet: the ingest and catalog VMs, the object lock on `fleet-backup`, the `checks/versitygw-*.nix` suite), so today's check proves range-read behaviour against a server we don't deploy.
- **It needs an exception.** nixpkgs marks `minio` insecure and abandoned upstream. `flake.nix:843-867` whitelists it with an argument that holds only inside the nix build sandbox. #688 found that ksb's ARC pods run Nix with `sandbox = false`, which voids that argument.
- **versitygw is packaged** in nixpkgs (1.8.0) and runs with a posix backend in a temp dir on loopback. That is the same shape as today's fixture, minus the insecure-package override.

## Scope
- Replace MinIO with versitygw (posix backend) in the range-read check, rename it to say what it proves (e.g. `s3-range-read`), and drop the `minio` insecure override and its rationale comment.
- Keep every assertion (GET counts, prune-before-fetch). Any behaviour that differs between the two servers (path-style vs virtual-hosted addressing, ETag/Range semantics, error codes) is a finding to document, not to paper over.
- Check `registry-roundtrip`'s loopback registry for the same reason, and record whether it also depends on sandbox isolation.

## Acceptance
- `nix build .#checks.x86_64-linux.<renamed check>` passes against versitygw.
- `git grep -n minio` returns only history and docs explaining the switch.
- The FEATURE-MATRIX and ADR-0002 §4 evidence rows name the new check.

Refs: #225, #688

---

# [Comment #1]() by [gerchowl]()

_Posted on October 29, 2026 at 11:40 AM_

**Owner decision (2026-10-29):** MinIO is no longer properly supported, so tessera won't use it. versitygw replaces it as the S3 test server, and the `minio` insecure override is removed outright, not kept as a fallback.

---

# [Comment #2]() by [gerchowl]()

_Posted on October 29, 2026 at 12:34 PM_

A finding from #688 that probably affects this check once it runs on the ksb runners (`sandbox = false`).

On ksb, a check's `$TMPDIR` is `build-dir` (`/nix/var/nix/builds`), which sits on the pod's single `/nix` volume (xfs on `/dev/vdf`). Every concurrent build writes to that volume; crane's `target.tar.zst` outputs are several GiB each. While 4 streaming writers ran on `/nix` in a `ksb-vig-os-large` pod (probe run https://github.com/vig-os/tessera/actions/runs/40872939839), a loopback `distribution` registry with **filesystem** storage under that volume behaved like this:

| request | idle | under disk load |
|---|---|---|
| `GET /v2/` (no disk) | 1 ms | 1–2 ms |
| `POST …/blobs/uploads/` (creates files) | ≤ 13 ms (one 2.4 s) | **60 s cap hit 3× in a row**, then 5 s |
| `PUT` blob | ≤ 13 ms | up to 11 s |

The same registry with `inmemory` storage stayed at or under 8 ms under the same load. This was the root cause of `registry-roundtrip` failing on ksb: the readiness GET passes, then the upload times out. MinIO's `503 SlowDownWrite` in the same failed leg looks like the same stall, seen through MinIO's own write throttling.

So a versitygw posix backend whose root is under `$TMPDIR` will probably stall in the same way on ksb, whatever the sandbox does. In #700 the registry checks moved to in-memory storage, plus port 0 (the real port read from the log), a private `$TMPDIR` dir for config and log, a bounded readiness wait that prints the log, and an exit trap (`loopbackRegistry` in flake.nix).


