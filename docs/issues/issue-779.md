---
type: issue
state: open
created: 2026-11-08T12:02:26Z
updated: 2026-11-08T12:03:41Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/779
comments: 0
labels: feature, effort:medium, area:core, area:io, adr
assignees: none
milestone: none
projects: none
parent: 778
children: none
synced: 2026-11-09T09:04:21.937Z
---

# [Issue 779]: [feat(schema): declared media columns — typed binary (image/jxl, image/svg+xml, …) per row, without opening the opaque-bytes door](https://github.com/vig-os/tessera/issues/779)

## Motivation

The scitadel-hub document model (epic #778) stores each paper's **figures** next to its text: vector graphics extracted losslessly from the PDF as SVG, and raster figures as JPEG XL: embedded JPEGs are transcoded losslessly (bit-exact reconstruction of the original JPEG, so its hash still verifies, and about 20% smaller), other rasters use JPEG XL lossless, including 16-bit images that WebP can't hold. Lossy re-encoding is never used for storage; WebP/AVIF thumbnails are display views only (owner decision 2026-11-08). The figures arrive in the millions (estimate: a 2×10⁶-paper subset with about 5 figures each is about 10⁷ figures at 10–300 KB each), keyed by `(work_id, fig_id)` and read together with the caption row. The original PDFs stay **outside** tessera (content-addressed hashes and URLs into a separate artefact store). Only the extracted figures come in.

Other consumers have the same shape: thumbnails or key images per row, small per-event binary payloads, signatures, and serialized descriptors.

## What already exists (verified at `dev` `26159de`)

- **There is no byte column.** `ColumnData` (`tessera/crates/tessera-io/src/table.rs:82`) has integer and float columns, `Bool`, `Utf8` (`:100`) and `Nullable` (`:122`), and no binary variant.
- **Binary is rejected on purpose at ingest.** `tessera/crates/tessera-ingest/src/arrow_table.rs:636-645` rejects `Binary`/`LargeBinary`/`BinaryView`/`FixedSizeBinary` because "Tessera tables carry no opaque byte columns (base64-in-utf8 would lie about the type and defeat the codec)".
- **The Blob block is one opaque file per block.** It is ADR-0038's preservation tier: `tessera/crates/tessera-core/src/block/blob.rs:1-13` and `BlobSpec` `:21`, with `filename`, `media_type` and `size`. About 10⁷ figures would mean about 10⁷ blocks, each with a manifest entry and a digest. That model doesn't fit per-row media.

## The gap

There is no row-aligned home for typed binary media. The objection to opaque bytes is right, so the answer is **not** an untyped `Binary` column. It is a **declared media column**: the bytes are typed by a sealed IANA media type, so a reader knows what they are and no type is falsified.

## Proposal

1. **A declared column kind, `media{media_type | media_type_column}`,** stored as a Vortex binary (`VarBinView`) column.
   - It is declared in the schema, so the undeclared-`Binary` rejection at ingest stays as it is (the same pattern as #768's declared vector columns).
   - The media type is either fixed per column (`image/jxl`) or comes from a sibling `Utf8` column, for mixed SVG/JPEG XL.
   - **SVG is text:** allow `media` over a `Utf8` carrier too, so SVG benefits from string compression (#628).
2. **Compression:** an already-compressed type (JPEG XL, WebP, AVIF, PNG, JPEG) is stored without a second codec pass. Text media follows the string path. Make sure a media column never runs the string-size path, which is #628's problem.
3. **Bounds:**
   - a per-value cap (configurable, default for example 16 MiB), so a huge value is pointed at the Blob block or an external store instead;
   - per-chunk statistics limited to length (min/max/sum of byte length), never the content.
4. **Reading:**
   - `tessera read` writes values to files (`--extract-to dir/{work_id}_{fig_id}.{ext}`), never to CSV;
   - the DataFusion provider (#654) exposes the column as Arrow `BinaryView`, with field metadata carrying the media type.
5. **Integrity:** values are covered by the block digest like every column. An optional per-row blake3 column, `media_hash`, allows deduplication and cross-product references.

## Acceptance

- [ ] A table with a declared `media{image/jxl}` column of 10⁶ values round-trips byte-identically. The sealed size is about the sum of the payloads plus a small overhead (measured).
- [ ] An undeclared Arrow `Binary` is still rejected, and the error suggests declaring a media column.
- [ ] Mixed SVG/JPEG XL through a media-type column. The SVG values follow the text codec path.
- [ ] Determinism: same input, same bytes, on x86_64 and aarch64 (Gate A/B).
- [ ] `tessera read --extract-to` writes files with extensions derived from the media type.

## Related

- Epic #778. ADR-0038 (Blob tier), ADR-0056 (normalise vs preserve), #768 (the declared-column precedent), #628 (string path), #654 (provider).


