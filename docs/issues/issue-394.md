---
type: issue
state: open
created: 2026-08-19T14:02:50Z
updated: 2026-09-28T17:59:21Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/394
comments: 0
labels: none
assignees: none
milestone: 0.1.0-beta
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:40.788Z
---

# [Issue 394]: [feat(ingest): TIFF / OME-TIFF → array, with source-pyramid carry-through](https://github.com/vig-os/tessera/issues/394)

Split out of the #386 generic-ingest spike. ADR-0056 **cuts TIFF from P1** on two independent
reviewer verdicts; this issue is where it lands properly.

## Motivation

"TIFF" reads like one format next to `.npy`. It is not. A representative week in a microscopy core:

- LZW-compressed 2-D scanner output.
- ImageJ hyperstacks where `ImageDescription` encodes CZT as ASCII (`slices=64\nframes=100\nchannels=3`)
  and pages are interleaved in an order only ImageJ knows.
- BigTIFF (>4 GiB, different magic) OME-TIFF with OME-XML in `ImageDescription` — 6-D `TCZYX`, plus
  external file references (`TiffData` UUID / `BinData`) spanning sibling files.
- Aperio/Leica whole-slide `.svs` — TIFF container, JPEG-in-TIFF tiled pyramid, label + macro images
  as extra IFDs, mm-per-pixel in a private tag.
- Deflate-compressed float32 from a light-sheet rig.
- 16-bit tiled TIFF with a broken `RowsPerStrip` half the readers refuse.

**The pyramid is the point.** A whole-slide `.svs` is 40 GiB and the pathologist only ever reads
level 3. Ingesting only flat single-page TIFF and calling it "TIFF support" silently drops the
pyramid of a slide scan — the first time that happens, trust is gone.

## Blocking constraint — determinism

JPEG-in-TIFF is **not bit-reproducible**: libjpeg vs libjpeg-turbo vs mozjpeg differ in IDCT rounding
and chroma upsampling, so the same `.svs` decodes to different pixels on different hosts and
Tessera's cross-arch `content_hash` gate fails (ADR-0056 §5 hazard H2). Any lossy tile codec
(JPEG, JPEG-2000, WebP) has this property.

## Scope

- P0: uncompressed / LZW / Deflate TIFF, single-page and multi-page → array; axes named from the
  source, never guessed.
- P0: **reject** JPEG/JPEG-2000/WebP-tiled TIFF from the normalising path with an error pointing at
  `tessera ingest blob` (bit-faithful preservation, ADR-0038) — do not accept non-reproducible decode.
- P1: source-pyramid **carry-through** — the source's own levels are canonical (they are what has
  been read for a decade and what QC replicates against), not a rebuilt pyramid. Needs a design pass
  against ADR-0028's `{hash, stats}` multiscale model.
- P1: OME-XML parsing → axes/units/`world_frame` where the XML declares them.
- P1: BigTIFF.
- P2: multi-file OME-TIFF (`TiffData` UUID references) — overlaps #393.
- P2: ImageJ hyperstack `ImageDescription` CZT de-interleaving.

## Pitfalls

- Rebuilding a pyramid instead of carrying it through changes the pixels a pathologist reads.
- ImageJ's channel/z/t interleave order is convention, not declaration — must be read, never assumed.
- `.svs` label and macro IFDs may contain a slide label image with **patient identifiers burned in**
  (ADR-0040 `identifying`); they must not be silently ingested as extra array levels.
- BigTIFF magic differs — a naive reader mis-detects and produces garbage rather than erroring.

## Acceptance criteria

- [ ] LZW and Deflate TIFF round-trip to array with values equal to the source.
- [ ] A JPEG-tiled TIFF is rejected with an actionable `ingest blob` message; asserted in a test.
- [ ] A pyramidal source's levels are preserved and readable per-level.
- [ ] Cross-arch `content_hash` equality on the accepted (lossless-codec) fixtures.
- [ ] `.svs` label/macro IFDs are not ingested without an explicit opt-in flag.

## References

- #386 generic ingest · ADR-0056 §3 (P1 format set), §5 hazard H2 (lossy tile codecs)
- ADR-0028 (multiscale pyramid) · ADR-0038 (blob preservation) · ADR-0040 (sensitivity tiers)
- #393 directory-shaped sources (multi-file OME-TIFF overlaps)
