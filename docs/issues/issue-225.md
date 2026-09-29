---
type: issue
state: closed
created: 2026-06-28T15:07:26Z
updated: 2026-09-28T17:54:28Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/225
comments: 2
labels: area:core, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:53:01.840Z
---

# [Issue 225]: [Cloud reads: async object-store range-read backend for .tsra (S3/MinIO); nix MinIO service-mock dev/test — spike & land](https://github.com/vig-os/tessera/issues/225)

## What
Make a sealed `.tsra` (and the exploded S3-prefix layout) **range-readable directly from object storage** (S3/GCS/Azure/HTTP) — fetch only the byte ranges needed (central dir → manifest → the one block), never a full download. Lands the async backend ADR-0002 §4 deferred, behind the existing sync `Reader` (generic over Read+Seek) as an additive variant.

## Why this is THE use case for async/tokio (the one place it pays)
Range reads are **latency-bound network I/O**, not CPU-bound — so async fans out hundreds/thousands of concurrent GETs on a few threads. This is the *read/distribution* side; it is NOT for ingest/encode (that's CPU-bound → std-threads). See #224 for the runtime split.

## Cohort-scale read (recorded design intent)
Two fan-out axes:
1. **intra-product** — an ROI = hundreds of chunk GETs at once within one image.
2. **inter-product / cohort-scale** — the *same* small read across *thousands* of images ('SUV-max ROI from every PET study', 'column en0 from 10k listmode products', 'z=64 slice from all CTs').

What makes cohort reads FAST (not just possible) is the format avoiding most bytes:
- range-readable `.tsra` (S6 / `range::CountingReader` proved locally) → 1-few GETs/image
- **prune-before-fetch**: the Merkle {hash,stats} pyramid (ADR-0028) often answers 'which images match?' from the tiny header — *without* reading any data block; fetch only survivors
- Vortex filter-pushdown + row_index (S10) → only matching row-groups/columns
- content-addressing → block hash = global cache/CDN key (dedup across the cohort)

**Convergence:** cohort-scale read = this cloud backend + collections/catalog (#223, to enumerate 'all images') + stats/pyramid (ADR-0028, to prune). Compute stays in the query engine (DuckDB/Polars/Arrow); Tessera makes the I/O not the bottleneck.

## Dev/test via nix (operator hosts MinIO; we mock in the gate)
Reuse the established loopback service-mock pattern (the OCI `distribution` registry check): a flake `check` spins **MinIO** on 127.0.0.1, `mc`/awscli2 seeds a bucket with a `.tsra`, the async backend does ranged GETs, asserts byte-identity + that only the needed ranges are fetched (CountingReader-style). All in nixpkgs: `minio`, `minio-client`, `awscli2`, `s3cmd`.

## Deliverables
- `object_store`-based async read backend (add the crate; instantiate a tokio runtime HERE — the legit use), behind the sync Reader surface.
- `tessera bench`/CLI: open a `.tsra` by S3 URL; ranged block read + verify.
- flake `check`: MinIO mock, ranged-read byte-identity + bytes-touched assertion.
- ADR update (ADR-0002 §4 un-defer as additive async variant; cross-ref #224 ADR-0034).

Context: branch `spike/tessera-core`. Builds on S6 range-read. Relates to #223 (catalog) + ADR-0028 (stats prune).
---

# [Comment #1]() by [gerchowl]()

_Posted on June 28, 2026 at 04:27 PM_

**Spike landed (199df79, branch spike/tessera-core) — staying open for the full landing.**

Proven: a sealed `.tsra` range-reads straight from object storage (MinIO/S3) **prune-before-fetch** — the `minio-range-read` flake check boots MinIO on loopback and runs the cloud test against it (30s real network I/O); it asserts over the wire that (a) open+manifest-parse fetches < the whole archive, (b) reading 1/8 blocks costs < the other 7 combined and < half the archive, (c) the S3-sourced block bytes are byte-identical to a local Reader (seal verified over the network).

Design (from a fresh-eyes pre-mortem): `cloud` cargo feature (optional object_store 0.14[aws] + tokio) — default build/test gain ZERO deps, tessera-core untouched; `ObjectStoreReader` adapts `ObjectStore` to the existing sync `Read+Seek` `Reader::from_reader` path (no public read-API change); one per-reader current-thread tokio runtime drives reqwest (the one legit tokio use, ADR-0034 §4). Hermetic-build fix: `SSL_CERT_FILE`=`pkgs.cacert` (reqwest's client builder needs a CA bundle even for plain-http loopback).

**Deferred to the full landing (NOT faked in the spike):**
- **Cohort fan-out** — needs the #223 catalog (enumerate 'all images') + an ADR-0028 stats consumer (prune-before-fetch *across* products). The spike proves the single-product seam only.
- Intra-product parallel block fetch; public cloud API + CLI (`tessera` open-by-URL); tail-prefetch buffer (each tiny zip tail read is currently a separate GET); retries/auth/presigned; GCS/Azure backends.
- ADR-0002 §4 un-deferral (land with the public-API shape decision).

Note: the check strips MinIO's nixpkgs `knownVulnerabilities` flag (upstream-abandoned) to use it as a sandboxed loopback mock; `garage` is a clean S3-compatible swap if preferred.

---

# [Comment #2]() by [gerchowl]()

_Posted on September 28, 2026 at 05:54 PM_

Closing as **done** — verified on `origin/dev` in the 2026-09-28 backlog triage.

Evidence: commit 199df79 spike land; 1eaddcf 'public open_url s3://·http; 64KiB tail-prefetch; cohort prune-before-fetch'; MinIO service-mock nix check green.

https://claude.ai/code/session_01XdERKMVDAwfMJSKdTytNnK

