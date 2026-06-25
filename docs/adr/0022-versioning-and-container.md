# ADR-0022 — Versioning DAG + `.tsra` container spec

**Status:** Accepted (P0) · **Register:** D1 (done) + versioning + container · **Issue:** #196

## Context
A Tessera product is immutable and content-hashed (ADR-0020). Two things still need pinning before
any reader/writer code: (1) how a product *evolves* into a new version without mutation, and (2) the
on-disk/on-wire container — its bytes, its internal index, and how a cloud reader range-reads it.

## Decision

### D1 — fd5 supersession (done)
fd5 is superseded by Tessera; repo renamed `vig-os/fd5`→`vig-os/tessera` (history kept). The fd5 Python
app is legacy. Recorded here for completeness; no further format impact.

### Versioning = an immutable DAG (copy-on-write)
Products are never mutated in place. A new version is a **new product with a new `id`** carrying a
`sources` edge to its parent: `Source { role: "supersedes" | "derived_from", reference: <parent id>,
content_hash: <parent manifest_hash> }`. Because the edge pins the parent's `manifest_hash`, the lineage
is a tamper-evident Merkle DAG: you can walk from any version back to the scanner-signed root and verify
every hop. No version numbers in the format — ordering is the DAG; a human-facing `name`/`timestamp`
disambiguates. This is fd5's `sources/` model; it composes with the integrity chain (S16).

### Container = a STORED zip64 archive (`.tsra`)
The canonical sealed form is a single **ZIP (zip64) archive**, extension `.tsra`:
- **Entries are STORED (uncompressed).** Payloads are already pcodec/zstd-compressed; STORED keeps every
  block byte-range directly addressable (range-read / mmap) with no inflate step.
- **First entry = `mimetype`** holding `application/vnd.tessera` (STORED, uncompressed, first in the
  archive) — the EPUB/ODF trick: `file(1)` / magic sniffers identify a `.tsra` without unzipping.
- **`manifest.json`** — the product manifest (ADR-0020). A reader parses it, omits `manifest_hash`,
  recomputes over canonical JCS bytes, and checks the seal; then verifies block digests on access.
- **`blocks/<name>/…`** — payloads: zarr stores (sharded, cubic, pcodec) for array blocks; Vortex files
  for table blocks. The `BlockRef.spec` says how to open each.
- **The zip central directory IS the internal index.** It sits at the end of the file and lists every
  entry's name, size, and offset → a cloud reader issues one ranged GET for the central directory, then
  ranged GETs for just `manifest.json` and the specific block byte-ranges it needs. No whole-archive
  download. **zip64** lifts the 4 GiB / 65 k-entry limits for large studies.

**Why zip, not tar:** tar is sequential with no index — random access means scanning. zip's central
directory gives O(1) range-addressable access, which is the whole point for cloud/object-store reads.

### Exploded form (opt-in)
The same logical tree may live **unzipped as an object-store prefix** (`s3://bucket/<id>/manifest.json`,
`…/blocks/…`) for parallel multi-writer ingest and copy-on-write versioning (a new version shares
unchanged block objects). Sealing/distribution then re-wraps into one `.tsra`, or ships the prefix as an
**OCI artifact** (manifest.json ≈ the OCI config; blocks ≈ layers) for registry distribution.

## Consequences
- Read path (P2) can be specified against a concrete, range-readable layout.
- Versioning needs no new format field — it rides `sources` + `manifest_hash`, already present.
- One canonical byte stream (`.tsra`) with an opt-in exploded/OCI form for cloud-native workflows.
- Container read/write is implemented in `tessera-io` (P2/P3); this ADR is the contract.
