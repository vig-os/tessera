# Arrays: stats, slice, project, pyramid

Array blocks are dense N-D grids (Zarr v3, 64³ cubic chunks, `pcodec` by default). The cubic grid means
orthogonal and ROI reads only touch the chunks they intersect — a single voxel decodes one chunk, not the
volume.

## Choosing the codec

The writer supports three array codecs, chosen per block (`Builder.add_array(..., codec=...)` in Python):

| codec | wins on | why |
|---|---|---|
| `pcodec` (default) | acquisitions — CT, PET, anything with detector noise | models the *numeric distribution*, which noise does not destroy |
| `zstd` | synthetic ramps, masks, packed bitfields | exploits long *byte-level* repeats (LZ77), which noise does destroy |
| `auto` | when you don't know | encodes with both and keeps the smaller; costs a double encode at write |

Neither dominates. On a 256³ int16 volume, measured through the Python binding:

| volume | `pcodec` | `zstd` | `auto` picks |
|---|---|---|---|
| phantom + detector noise (acquisition-shaped) | **13.12 MiB** | 15.97 MiB | `pcodec` |
| pure linear ramp (synthetic) | 0.25 MiB | **0.22 MiB** | `zstd` |

On acquisition-shaped data the advantage is **size-invariant** — pcodec/zstd ≈ 0.82 from 64³ to 320³ —
so the absolute saving grows linearly with the volume; real acquisitions measured −21% (CT) and −33%
(PET) against zstd. On a synthetic ramp the ratio *moves with the array size*, which is why a
benchmark built on one can mislead in either direction.

All three are per-chunk codecs, so partial reads work identically whichever was used, and the manifest
records the **concrete** codec: `auto` is resolved at write time and a reader never sees it.

{{#include ../../../tessera/crates/tessera-cli/tests/cmd/arrays.trycmd}}

- `stats` — shape, dtype, chunking, and **both** the raw stored range and the *physical* (rescaled)
  range. For CT that's Hounsfield units (`1·raw + −1024 HU`); the rescale is carried on the block so the
  overview is physically meaningful without decoding. `--json` adds `exact` and `method` (below).
- `slice --index z,y,x` — pull a point, line (`0,0,:`), or plane (`z,:,:`) as CSV. Only the intersecting
  chunks are read.
- `project` — collapse one axis into a 2-D image: `--mode max` (MIP, the classic PET/CT overview),
  `mean`, or `sum`.
- `pyramid` — build a multiscale overview (full-res + 2× downsampled levels) into a new **derived**
  `.tsra`, each level carrying its `at_level` affine (OME-Zarr `multiscales` geometry) and a
  `derived_from` provenance edge back to the source.

## Stats without decoding

`count`, `min`, `max`, `sum` are **monoids**: a chunk's value folds from its voxels, and a parent's
folds from its children. So when an array block carries a `{hash, stats}` chunk-index sidecar
(`<block>.cidx`, [ADR-0028](https://github.com/vig-os/tessera/blob/dev/docs/adr/0028-unified-hierarchy.md)
§3), the whole-array statistics are already computed — `stats` reads them off
`ChunkIndex::aggregate()` and never touches the data block. `mean` and `std` come free with them.

On a 127.7 M-voxel int16 volume (487×512×512, 64³ chunks, 151 MiB sealed) the difference is not subtle:

| | warm | cold | peak RSS |
|---|---|---|---|
| with `.cidx` (index aggregate) | **0.0061s** | **0.0079s** | **12.4 MiB** |
| without (full decode) | 0.5795s | 0.7113s | 876.5 MiB |

**95× faster warm, 90× cold, 71× less memory** — and the sidecar that buys it is 85 KiB, 0.06 % of the
file. Both paths report identical `min`/`max`/`mean`/`std` to the last bit, because the full-decode path
reduces through the same `i128` monoid the index does.

The output always says which path ran, because an approximation must never be mistaken for an exact
answer:

| `method` | `exact` | when |
| --- | --- | --- |
| `chunk-index-aggregate` | `true` | a usable sidecar was present — no decode |
| `full-decode-exact-integer` | `true` | integer array, decoded and reduced in exact integer arithmetic |
| `full-decode-f64-float` | `false` | float array — `f64` accumulation is never correctly rounded |
| `full-decode-f64-integer-overflow` | `false` | integer array whose exact accumulator overflowed |

`exact: true` means the value is the correctly-rounded result of exact arithmetic over every sample.
An `f64` accumulation is not that — its error grows with the sample count — so a float array reports
`exact: false` however small the error looks. (JSON has no NaN or Infinity, so a non-finite statistic
serialises as `null`: read that as "not available", never as zero.)

A sidecar is **refused**, and `stats` falls back to decoding, unless *all* of these hold:

- it declares `indexes: <block>`;
- the block's dtype is an integer **narrower than 64 bits** — `sum_sq` folds in `i128`, which a 64-bit
  sample can overflow, and until [#523](https://github.com/vig-os/tessera/issues/523) is fixed the
  *write* path wraps silently, so such an index may already be wrong;
- its entry count matches the block's chunk grid, and its aggregate covers every voxel;
- the roll-up itself does not overflow.

Being slow is recoverable; confidently reporting a wrong `min`/`max` is not.

> The index is currently bound to its block by **name**, not by content digest, so it cannot be
> *proved* to describe the block's current bytes. `tessera commit` therefore removes `<name>.cidx`
> whenever `<name>` is removed or replaced. Binding the index to the block's digest is a sealed-layout
> change and is a mandatory part of the one format event being specified for
> [#522](https://github.com/vig-os/tessera/issues/522).

> Integer arrays only, for now. Float arrays have no exact integer monoid, and a value **histogram**
> needs a monoid that does not exist yet — tracked in
> [#522](https://github.com/vig-os/tessera/issues/522), which has to settle the bin edges at write time
> to stay decode-free.

Coordinates: an array optionally carries a voxel→world affine (`world_frame`, LPS mm); when present,
`slice`/`stats` become world-aware (`--world`). When absent — as in this corpus fixture — the tools stay
in index space rather than inventing coordinates.

## Output formats

`slice` and `project` write CSV by default, and four other shapes with `--format`:

| `--format` | what you get |
| --- | --- |
| `csv` / `tsv` | one line per row — the default, fine for a plane you are eyeballing |
| `json` | one self-describing object: the full `shape`, `dtype`, `source_dtype`, `rows_emitted`, `truncated` and the values |
| `npy` | NumPy `.npy` at **the array's own dtype** (`<i2`, `<u8`, `|b1`, …) — **the lossless path**; `np.load()` it directly |
| `png` | 8-bit greyscale preview, auto-windowed on the plane's own min/max (`--window lo,hi` to override) |

`npy` and `png` are binary, so they must be redirected — writing them to a terminal is refused rather
than spewed:

```console
tessera slice ct.tsra volume --index "32,:,:" --format npy > plane.npy
tessera project ct.tsra volume --axis z --format png  > mip.png
```

`npy` carries the dtype the product stores, so a 64-bit integer survives exactly — `f64` has a 53-bit
mantissa and could not promise that. A `--physical` rescale or a reducing projection (`--mode mean`/`sum`)
genuinely computes floats, so those are written `<f8` and the JSON `dtype` says `float64`; `--mode max`
picks an existing sample, so it keeps the native dtype.

**`png` is a preview, not data.** Eight bits cannot hold a Hounsfield range, let alone a float activity
map, so the mapping is lossy by construction. The window actually used and the source dtype are written
into the PNG's `tEXt` chunks, so a preview that has been copied out of context still says what it is —
and NaN/inf samples render as black, indistinguishable from the window's low end. Use `npy` when you
mean the numbers.

### The row cap

Text output at an **interactive terminal** stops after 20 rows and tells you so on stderr — a preview, so
a 512-wide plane cannot flood your scrollback. Piped or redirected output is **never** capped by default,
because a script silently receiving 20 of 4097 rows is data loss rather than a courtesy. `--limit N` and
`--all` apply anywhere; `--limit` with `npy`/`png` is an error, since a truncated binary artifact is
corrupt rather than short.
