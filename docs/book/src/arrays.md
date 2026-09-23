# Arrays: stats, slice, project, pyramid

Array blocks are dense N-D grids (Zarr v3, 64³ cubic chunks, `pcodec`). The cubic grid means orthogonal
and ROI reads only touch the chunks they intersect — a single voxel decodes one chunk, not the volume.

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
