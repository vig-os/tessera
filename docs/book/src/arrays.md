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
  overview is physically meaningful without decoding.
- `slice --index z,y,x` — pull a point, line (`0,0,:`), or plane (`z,:,:`) as CSV. Only the intersecting
  chunks are read.
- `project` — collapse one axis into a 2-D image: `--mode max` (MIP, the classic PET/CT overview),
  `mean`, or `sum`.
- `pyramid` — build a multiscale overview (full-res + 2× downsampled levels) into a new **derived**
  `.tsra`, each level carrying its `at_level` affine (OME-Zarr `multiscales` geometry) and a
  `derived_from` provenance edge back to the source.

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
