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

**Table — three formats, two fixtures.** One fixture is a chosen answer, so both are reported.

The periodic fixture's float columns have periods 7 and 5, which is *adversarial* for
value-distribution codecs: deflate's LZ77 window locks onto the repeating byte block, while Pco,
dictionary and bit-packing model the value distribution and cannot exploit periodicity. The
continuous fixture is shaped after real DUPLET listmode — a coarse millisecond clock with
Poisson-varying run lengths plus continuous energies.

Every format is shown at its default *and* tuned, with the standard float tuning applied to each:
HDF5 gets **shuffle**+gzip, Parquet gets **BYTE_STREAM_SPLIT**+zstd (its counterpart, with the
dictionary off since a dictionary over continuous floats defeats the split).

| size | periodic | continuous |
|---|---|---|
| Tessera | 959.4 KiB | **19.6 MiB** |
| HDF5, shuffle+gzip-4 | **155.3 KiB** | 21.5 MiB |
| HDF5, uncompressed | 61.0 MiB | 61.0 MiB |
| Parquet, uncompressed (crate default) | 40.9 MiB | 44.9 MiB |
| Parquet, snappy (pyarrow/Spark default) | 23.0 MiB | 44.8 MiB |
| Parquet, zstd-4 + BYTE_STREAM_SPLIT | 4.0 MiB | 23.4 MiB |

On the **continuous** fixture — the representative one — Tessera is smallest: 1.10× under HDF5
shuffle+gzip and 1.19× under tuned Parquet. On the **periodic** one HDF5 wins by 6.2×, and Tessera
beats tuned Parquet 4.3×. On real DUPLET listmode Tessera seals to 73.4 MiB against shuffle+gzip's
75.7 MiB (**1.03× smaller**), tracking the continuous fixture.

Latency on the continuous fixture (warm):

| | full read | 1 column | row window |
|---|---|---|---|
| Tessera | 0.0309s | 0.0160s | 0.0119s |
| HDF5, uncompressed | **0.0098s** | **0.0014s** | **0.0001s** |
| HDF5, shuffle+gzip-4 | 0.1266s | 0.0479s | 0.0043s |
| Parquet, snappy | 0.0483s | 0.0125s | 0.0010s |
| Parquet, zstd-4 + BSS | 0.0751s | 0.0245s | 0.0021s |

#### Where Tessera loses, and why

- **The periodic fixture's size, by 6.2× to HDF5.** Explained above, and only there: on the
  continuous fixture and on real data Tessera leads. Investigated in #493 — the container and stats
  are 0.4 % of the file and the integer column compresses 2213×; it is entirely the periodic floats.
- **Every small, raw-speed read.** Uncompressed HDF5 wins the single column (0.0014s vs 0.0160s) and
  the row window (0.0001s vs 0.0119s), because a contiguous uncompressed slab is a seek plus a
  memcpy. **Parquet also beats Tessera on both** (0.0125s and 0.0010s with snappy) — column
  projection is its headline strength, and a row window is a row-group read it is built for.
  Tessera's column and window reads decode from the whole sealed block, so they carry the block's
  bytes even when only part is wanted.
- **Writing is ~9× slower than uncompressed HDF5.** Tessera hashes and seals; HDF5 memcpys. That is
  the cost of the integrity guarantee, not a tuning bug.
- **Full volume read is slower than uncompressed HDF5** (0.0244s vs 0.0162s) — decompression against
  none — though Tessera is ~2× *faster* than the compressed HDF5 it is comparable to (0.0506s).

Against the **compressed** configurations, which are the fair comparison for a format that always
compresses, Tessera reads the continuous table 4× faster than shuffle+gzip HDF5 and 2.4× faster than
tuned Parquet.

#### Integrity is not one number

HDF5's `fletcher32` detects corruption in the chunks you read; it is unkeyed and covers no metadata,
and anyone who rewrites a chunk rewrites its checksum. `tessera verify` re-derives every block digest
against a sealed manifest whose hash also covers metadata and provenance, and which a signature can
bind to a signer. Both catch a flipped bit; only one answers "is this the artifact that was sealed,
and by whom". The benchmark times both and says which is which.

Parquet has no integrity row to time: the format permits an optional per-page CRC32, but
parquet-rs 58's writer never emits one, so for the files this benchmark writes there is nothing to
check. The broader seven-format comparison (Zarr, NeXus, NIfTI, DICOM, ROOT, Parquet) lives in
`tessera/bench/ecosystems/`.

## When a plain format is the right call

Tessera earns its keep when data must be **shared, trusted, and outlive its tooling** — clinical/
scientific archives, multi-site cohorts, anything with a chain of custody. If you just need a fast local
scratch table for a single analysis and throw it away, reach for Parquet directly — that's exactly the
engine Tessera would compose for you anyway.
