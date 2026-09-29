---
type: issue
state: closed
created: 2026-07-01T10:54:40Z
updated: 2026-09-28T17:54:31Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/253
comments: 1
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:53:01.079Z
---

# [Issue 253]: [feat(cli): array/volume exploration — `tsra slice` / `stats` (index-native + `--world` mm addressing) + clear error for `read <array>`](https://github.com/vig-os/tessera/issues/253)

## Problem
`tsra read` is **table-only** (builds a `LogicalTableView`). On an **array** block (`recon` volume, μ-map) it fails with the cryptic `serialize: missing field 'columns'`. And there's **no** CLI way to look at a volume at all — only `ls` (spec), `extract` (raw bytes), or drop to Python/zarr. A user with `ct-lung.tsra volume int16 [890,512,512]` can't peek a slice or get value stats.

## Design — index-native, `--world` mm on top
A volume is `[z,y,x]` C-order, LPS-canonical (ADR-0030). Index is the primitive (always available); the voxel→world **affine** (`ArraySpec.world_frame`, optional) enables mm addressing when present (ADR-0029 feature-by-presence).

**1. Clear the cryptic error (quick).** `tsra read <array-block>` → typed error: *"'volume' is an array block, not a table — use `tsra slice` / `tsra stats` / `tsra extract`."*

**2. `tsra stats <FILE> <BLOCK>`** — "general looking at it": shape · dtype · codec · min/max/mean/std · (optional) histogram · whether a `world_frame` is present + its voxel spacing. Decodes lazily (streams chunks; never the whole 890³ into RAM).

**3. `tsra slice <FILE> <BLOCK>`** — pull a 2-D plane (or 1-D line / point):
- **By index:** `--index "445,:,:"` (numpy-style, C-order z,y,x) → the axial plane; `--index ":, 256, :"` a coronal, etc.
- **By world (mm):** `--world-z 12.3` (or `--world "L,P,S"`) → resolve to the nearest voxel via the inverse affine; error if the product has no `world_frame`.
- **Output:** `--format csv|tsv|npy|png` (CSV of the 2-D array for small; `.npy` for lossless downstream; `.png` with a window/level for a quick look). Applies `rescale_slope/intercept` for physical units when `--physical`.

**4. (later) ROI/box:** `--index "400:500, 200:300, :"` → sub-volume to `.npy` — the sharded-read path (S5 zarrs), prunes chunks by the 64³ chunking + ChunkIndex (#214).

## Scope / phasing
- Phase 1: the clear error + `tsra stats` + `tsra slice --index` (CSV/npy). Pure index, no affine.
- Phase 2: `--world` mm addressing (inverse affine) + `--physical` rescale + `png`.
- Phase 3: sub-volume box + chunk-pruned reads (ties to S5 / #214 / cloud #225).

Reuses the array decode path (`tessera_io::array`); the reader already has chunked access. Feature-gated `png` (image crate) if we add rendering. trycmd walkthrough on a synthetic small volume.
---

# [Comment #1]() by [gerchowl]()

_Posted on September 28, 2026 at 05:54 PM_

Closing as **done** — verified on `origin/dev` in the 2026-09-28 backlog triage.

Evidence: Phase 1 landed 44221b3 (#257 index-native); Phase 2 landed 1ecbd32 (#263 --world mm addressing); 023c977 (#254) 'richer tsra read slicing + clear array-block error'.

https://claude.ai/code/session_01XdERKMVDAwfMJSKdTytNnK

