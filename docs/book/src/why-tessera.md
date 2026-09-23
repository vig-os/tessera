# Why Tessera? (vs Parquet, HDF5, Zarr)

> **Short version.** Parquet, HDF5 and Zarr are *storage* formats — they hold bytes well. Tessera is a
> *FAIR data-product* format: it makes **identity, integrity, provenance and versioning** first-class,
> and **composes the best codec per data shape** (Zarr + [pcodec](https://github.com/mwlon/pcodec) for
> arrays, [Vortex](https://github.com/spiraldb/vortex) for tables) underneath. If you'd otherwise reach
> for "Parquet + a sidecar hash + a metadata JSON + a naming convention," Tessera is that, sealed into
> one verifiable object.

## The honest framing

Tessera does **not** reinvent compression. Internally it *uses* Zarr, pcodec and Vortex — the same
engines you'd pick anyway. What it adds is everything **around** the bytes that scientific and clinical
data needs and that a bare storage format leaves to you:

- a single **`manifest_hash` seal** (blake3 over the canonical manifest) that transitively commits to
  every block digest and all metadata — tamper anywhere is detectable by one hash;
- a **content-addressed identity** (`id`) and a Merkle-Mountain-Range **`content_hash`**, so the same
  logical data is byte-reproducible **across machines and CPU architectures** (a gated property);
- a **provenance graph** (`derived_from`, producer, generation recipe) carried *inside* the product;
- **git-shaped versioning** (`init/import/commit/log/diff/publish/seal`) over a content-addressed store;
- **signing + a trust store**, **OCI push/pull** distribution, **cloud range-reads** (fetch only the
  bytes a query needs), and **FAIR / RO-Crate** export.

## Capability comparison

`native` = built in and sealed · `bolt-on` = possible but you assemble/maintain it yourself · `—` = no.

| Capability | **Tessera** | Parquet | HDF5 | Zarr |
| --- | --- | --- | --- | --- |
| Dense N-D arrays | native (Zarr v3 + pcodec) | — | native | native |
| Large event tables | native (Vortex) | native | via compound types | — |
| **Best codec per shape in one file** | **native** | tables only | one-size-fits | arrays only |
| Whole-object integrity **seal** | **native** (`manifest_hash`) | per-page CRC only | — | — |
| Content-addressed **identity** | **native** | — | — | — |
| **Cross-arch byte reproducibility** | **native** (gated) | not guaranteed | not guaranteed | codec-dependent |
| **Provenance graph** in-product | **native** | bolt-on | bolt-on (attrs) | bolt-on (attrs) |
| **Versioning / audit** (git-shaped) | **native** | — | — | — |
| **Signing / trust** | **native** | — | — | — |
| Cloud **range-read** (prune-before-fetch) | **native** | row-group/column | limited | native (chunks) |
| **OCI** distribution | **native** | bolt-on | bolt-on | bolt-on |
| Vendor **ingest normalisation** (DICOM, GE-HDF5, NIfTI) | **native** | — | — | — |
| **FAIR / RO-Crate** export | **native** | — | — | — |
| Self-describing, **versioned schema** | **native** (`ProductSchema`) | schema only | attrs | attrs |

## "Why not just Parquet + a hash file?"

You can get *some* of the top rows by hand: Parquet for tables, a `sha256` sidecar, a `metadata.json`,
a folder convention for versions. The problem is that those pieces are **not bound together** — the hash
doesn't cover the metadata, the provenance link is a filename, a re-export silently changes bytes, and
nothing proves the JSON belongs to the data. Tessera's seal binds them into **one object you can verify
offline, decades later**, with a single command:

```bash
tessera verify study.tsra     # re-checks the seal + every block digest
```

## The numbers we have (and the ones we're adding)

- Arrays: Zarr + pcodec is **−21 % (CT) / −33 % (PET) vs zstd**, lossless, with sharded ROI reads.
- Tables: Vortex float columns compress via pcodec and land competitive with — often smaller than —
  Parquet+zstd on continuous scientific data (issue #380).

A head-to-head `tessera bench compare` (same data → `.tsra` vs Parquet vs HDF5: size, column-projection
and ROI-slice latency, cold-cache cloud read) is tracked as a follow-up so these claims ship as
reproducible numbers, not assertions.

## When a plain format is the right call

Tessera earns its keep when data must be **shared, trusted, and outlive its tooling** — clinical/
scientific archives, multi-site cohorts, anything with a chain of custody. If you just need a fast local
scratch table for a single analysis and throw it away, reach for Parquet directly — that's exactly the
engine Tessera would compose for you anyway.
