# ADR-0038 — Blob block: opaque, bit-faithful preservation tier

**Status:** Accepted (as-built) · **Issue:** #229

## Context

Tessera's thesis is "normalise vendor-proprietary at the door" → queryable `array`/`table` blocks. But the
real world ships a long tail the engine cannot (yet) parse: Siemens `.l64` listmode (multi-GB opaque
binary), GE `.7z`/`.cal` dumps, vendor PDFs and logs. Before ADR-0038 every block was *decoded* —
`BlockKind` was `Array | Table | ChunkIndex`, and even the `raw` ingest backend required a shape+dtype and
produced a typed array. There was no way to take an un-parseable file off a scanner and seal it
**bit-faithfully**, which blocks day-one migration ("capture now, understand later") and provenance
(keeping the original acquisition file beside its derived product).

## Decision

Add **`BlockKind::Blob`** — an opaque block whose payload **is** the file's bytes, stored verbatim
(uncompressed, STORED in the zip64 container), with digest `= blake3(bytes)`.

- Because the container's pack/verify path is already kind-agnostic (it stores each payload at
  `blocks/<name>` and re-hashes on read), a blob needs **no new storage path** — only a producer that
  pairs `blake3(bytes)` with the raw bytes and a self-describing descriptor `BlobSpec {filename,
  media_type?, size}`. The existing hash-on-write → Merkle `content_hash` → `manifest_hash` seal →
  signature machinery and the §7 reader verification all apply unchanged, so `tessera verify` proves
  bit-faithfulness for free.
- A built-in **`blob` product schema** (requires one `data` blob block); ingest via the declarative spec
  (`format = "blob"`, or the `"junk"` alias) and a `tessera ingest blob|junk` CLI subcommand;
  recovery via `tessera extract <tsra> <block> <out>` (byte-identical). A conformance-corpus fixture
  (`blob_opaque`) pins the golden hashes.

## FAIR framing — why this is a *tier*, not a backdoor

A blob is **F**indable, **A**ccessible, **R**eusable-as-bytes, and **integrity-verified**, but **not
Interoperable** until a decoder exists. It is therefore a *preservation companion* to the normalised
products, never a replacement: a raw blob and a derived `listmode`/`recon` product can coexist in one
sealed `.tsra` (or one collection), joined by a `derived_from` provenance edge — capture the truth off the
scanner today, decode it whenever the parser lands.

## Consequences

- **Format-level addition** — a new `BlockKind` variant. Additive: spine readers `kind`-dispatch and stay
  inert to blobs they don't understand; old products are unaffected.
- **No compression** is intentional — a blob is *faithful*, not *small*. A caller who wants compression
  should normalise into an `array`/`table` block (which is also queryable), or compress out-of-band.
- **Naming**: the canonical kind/format is `blob`; `junk` is an accepted **alias** (CLI subcommand +
  `format` tag) — same machinery, a cathartic name for vendor files that have earned it. The *stored*
  representation is always `blob` (determinism + a clean corpus).

## Alternatives considered

- *Extend `raw`* to "headerless OR opaque" — rejected: `raw` decodes to a typed array (it has a shape);
  conflating "typed but headerless" with "never decoded" muddies both. Distinct kinds, distinct intent.
- *signatures-as-hashed-block / attachment sidecar files* — heavier and out-of-container; a blob keeps the
  bytes **inside** the sealed product so one `manifest_hash` + one signature covers everything (the
  archival principle, ADR-0037).
