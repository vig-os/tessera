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

### Run the head-to-head yourself

`tessera bench compare` writes the *same* synthetic volume and table to `.tsra` and to HDF5 and
reports on-disk size, write+seal, full read, projected-column read, ROI read and integrity cost:

```sh
tessera bench compare                 # both datasets, warm, median of 7
tessera bench compare --cold          # add best-effort cold-cache rows
tessera bench compare --format json   # the CI shape
```

It is built to be checkable rather than flattering:

- **Every format appears twice**, at its sensible default and at a tuned setting, with the settings
  printed on each row. HDF5's default really is contiguous and uncompressed; its tuned variant gets
  **shuffle+gzip-4** (the standard pairing — shuffle alone changes a lot, see below) and *Tessera's
  own* 64³ chunk geometry, so the ROI row compares layouts rather than chunk-size luck.
- **Every timed read is verified against the source data first** — full, projected and ROI, for each
  format and setting. A partial read that returned too little would otherwise look fast.
- **Medians of N with a `[min..max]` spread**, never a single run, and the machine, filesystem and
  library versions are printed with them.
- **The synthetic data is far more compressible than real acquisitions** (it is the generator from the
  cross-ecosystem harness, kept verbatim for comparability), so the size column is a ratio *between
  formats on identical input*, not a compression ratio to expect clinically.

#### What it actually says (Xeon w9-3575X ×88, ext4, median of 5)

**Volume, 256³ int16 (32 MiB raw) — Tessera wins on size and ROI.**

| | size | full read (warm) | ROI read (warm) |
|---|---|---|---|
| Tessera, pcodec (default) | 256.1 KiB | 0.0244s | 0.0008s |
| Tessera, zstd (tuned) | **225.5 KiB** | 0.0256s | **0.0003s** |
| HDF5, contiguous uncompressed | 32.0 MiB | **0.0162s** | 0.0005s |
| HDF5, 64³ + shuffle+gzip-4 | 698.8 KiB | 0.0506s | 0.0006s |

**Table — reported on TWO fixtures, because one fixture is a chosen answer.**

The original fixture's float columns have periods 7 and 5. That is *adversarial* for
value-distribution codecs: deflate's LZ77 window locks onto the repeating byte block, while Pco,
dictionary and bit-packing model the value distribution and cannot exploit periodicity at all. The
continuous fixture is shaped after real DUPLET listmode — a coarse millisecond clock (~99.6 % zero
deltas) plus continuous energies. Both are reported, always.

| fixture | Tessera | HDF5 shuffle+gzip-4 | |
|---|---|---|---|
| **periodic** (adversarial) | 959.4 KiB | **155.3 KiB** | HDF5 **6.2× smaller** |
| **continuous** (listmode-like) | **19.6 MiB** | 21.5 MiB | Tessera **1.10× smaller** |

Same encoder, same command, opposite verdicts — which is why publishing only one of them would be
picking the result. On real DUPLET listmode (`/events_2p`, 4M rows, 106.8 MiB raw) Tessera seals to
73.4 MiB against shuffle+gzip's 75.7 MiB — **1.03× smaller**, tracking the continuous fixture rather
than the periodic one.

Latency, on the periodic fixture (full/1-column/row-ROI, warm):

| | full read | 1 column | row ROI | write+seal |
|---|---|---|---|---|
| Tessera | 0.0062s | 0.0024s | 0.0015s | 0.3179s |
| HDF5, contiguous uncompressed | 0.0110s | **0.0017s** | **0.0001s** | **0.0361s** |
| HDF5, shuffle+gzip-4 | 0.0682s | 0.0164s | 0.0023s | 0.3372s |

#### Where Tessera loses, and why

- **The periodic fixture's size, by 6.2×.** Explained above, and it is the only place that gap
  appears: on the continuous fixture and on real data Tessera is ahead. Investigated in full in
  #493 — the container and stats account for 0.4 % of the file, and the integer column compresses
  2213×; it is entirely the two periodic float columns.
- **Writing is ~9× slower than uncompressed HDF5** (0.3179s vs 0.0361s). Tessera hashes and seals;
  HDF5 memcpys. That is the cost of the integrity guarantee, not a tuning bug.
- **Uncompressed HDF5 wins the small, raw-speed reads** — the single-column read (0.0017s vs 0.0024s)
  and especially the row window (0.0001s vs 0.0015s, ~15×), because a contiguous uncompressed slab is
  a seek plus a memcpy and nothing Tessera does can be cheaper than that.
- **Full volume read is slower than uncompressed HDF5** (0.0244s vs 0.0162s) — decompression against
  no decompression — though Tessera is ~2× *faster* than the compressed HDF5 it is actually
  comparable to (0.0506s).

Against the **compressed** configuration, which is the fair comparison for a format that always
compresses, Tessera reads the periodic table 11× faster (0.0062s vs 0.0682s), a column 6.8× faster,
and the volume 2× faster.

#### Integrity is not one number

HDF5's `fletcher32` detects corruption in the chunks you read; it is unkeyed and covers no metadata,
and anyone who rewrites a chunk rewrites its checksum. `tessera verify` re-derives every block digest
against a sealed manifest whose hash also covers metadata and provenance, and which a signature can
bind to a signer. Both catch a flipped bit; only one answers "is this the artifact that was sealed,
and by whom". The benchmark times both and says which is which.

Parquet joins the table once its Rust crates land (#460); the broader seven-format comparison
(Zarr, NeXus, NIfTI, DICOM, ROOT, Parquet) lives in `tessera/bench/ecosystems/`.

## When a plain format is the right call

Tessera earns its keep when data must be **shared, trusted, and outlive its tooling** — clinical/
scientific archives, multi-site cohorts, anything with a chain of custody. If you just need a fast local
scratch table for a single analysis and throw it away, reach for Parquet directly — that's exactly the
engine Tessera would compose for you anyway.
