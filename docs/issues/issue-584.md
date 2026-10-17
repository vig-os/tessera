---
type: issue
state: open
created: 2026-10-16T23:40:05Z
updated: 2026-10-16T23:40:05Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/584
comments: 0
labels: feature
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-17T06:32:40.377Z
---

# [Issue 584]: [feat(py/format): zero-copy and cache — raw array codec, Arrow zero-copy tables + fixed_shape_tensor, DLPack, in-process chunk LRU](https://github.com/vig-os/tessera/issues/584)

### Context

anatomist operates on ~0.8 GB per dog, loops over a cohort many times (groupwise atlas, EM/MLEM-family, leave-one-out), and will train a distilled CNN on tessera outputs (phase 9). The cost of **decode on every read** adds up, and some paths that LOOK zero-copy aren't. The FEATURE-MATRIX already acknowledges "Python Arrow zero-copy is a separate pending perf item"; this issue broadens it to the rest of the hot path.

### Measurements and gaps

| Path | Today | Target | Reference |
|---|---|---|---|
| `r.array("volume")` | pcodec decode → owned bytes → `np.frombuffer` (one copy from the owned bytes, but the decode itself is unavoidable at ~1 GiB/s per core; a 0.8 GB volume costs ~0.8 s) | optional `codec = "none"` so the array block IS raw dtype bytes; then `np.frombuffer` on an mmap'd zip STORED entry is genuine zero-copy. Reserve pcodec/zstd for cold storage. | zarr v3 `bytes` codec with no filters |
| `r.table_arrow("events")` | Vortex → `{col: ndarray}` dict → `pa.table(dict)` (not zero-copy from the file; decode allocates) | expose Vortex's own Arrow array output directly; return `pyarrow.Table` whose buffers alias the decoded Vortex arrays | Vortex's `ArrayRef → arrow::ArrayRef` already exists in the Rust crate |
| Patches of fixed shape across many rows | would need a struct column of per-element primitives | the Arrow canonical **extension type** `arrow.fixed_shape_tensor` (and `variable_shape_tensor`) so a table block can hold a column of `[C,H,W,D]` patches with one declared shape | Arrow Columnar Format: Canonical Extension Types |
| GPU hand-off | numpy → `torch.from_numpy(...).cuda()` (one host→device copy) | `r.array_dlpack("volume")` returning a DLPack capsule; `torch.utils.dlpack.from_dlpack`, `cupy.from_dlpack` take it zero-copy on the host side (and from `cupy.ndarray` for GPU-resident decode once that lands) | DLPack spec |
| Atlas-loop re-reading the same volumes | each `read_block` re-reads from disk and re-decodes (no cache) | an in-process `LRUChunkCache` keyed by `(product_id, block_name, chunk_index)` so a decoded 64³ chunk survives the next sweep | zarr-python v3 `LRUStoreCache` |
| xarray / dask on a `.tsra` | not possible without extraction | the "OME-Zarr store adapter" in #(adapters-bundle issue) unlocks this with no new work here | — |

### Proposal

1. Add a `"none"` codec to `ArraySpec.codec` (SPEC §5a) with the same spec rules the others follow (STORED in zip, chunk grid unchanged, bit-exact round-trip). Writer picks per-block, as today.
2. In `tessera-py`, add `read_array_arrow`, `read_table_arrow_zerocopy` (if Vortex's output supports it), `read_array_dlpack`, `read_array_cuda` (behind feature flag).
3. Add the Arrow `fixed_shape_tensor` extension type as a recognised column dtype in the table encoder/decoder.
4. Add an opt-in `tessera.Cache(size_bytes)` wrapping the `Reader` with an LRU over decoded chunks.

### Not asking for

- a GPUDirect/KvikIO path right now — anatomist defers that per its ARCHITECTURE §4 (whole volumes fit in GPU memory; GDS deferred until micro-CT scale).
- MONAI Dataset / OME-Zarr store export — those live in the sibling adapter bundle issue.

