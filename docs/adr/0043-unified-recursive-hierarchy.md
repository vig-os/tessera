# ADR-0043 — The unified recursive hierarchy: one {hash, stats, referencing} node at every scale

**Status:** Proposed (spike for #215). Synthesises the layers that #214 (chunk-index), #260 (multiscale
pyramid), #259 (aux sidecars), #223 (collections), and #216 (composition/N-D) each add — showing they
are **the same node repeated at different scales**, not five separate mechanisms.

## Context

Tessera has grown a set of features that each attach a layer to the product:

- **chunk** level — a `ChunkIndex` block carrying per-chunk `{hash, stats}` for confirm-and-prune reads (#214).
- **block** level — each block's `blake3` digest, rolled into `content_hash` (the Merkle root).
- **product** level — the sealed `manifest_hash`, and a detached-then-embedded **signature** (ADR-0037, #259).
- **scale** level — a multiscale **pyramid** of downsampled array blocks + axis **projections** (#260),
  each derived block carrying the parent's affine scaled by `WorldFrame::at_level` (ADR-0028/0030).
- **collection** level — an MMR over member products, a study/cohort (#223, ADR-0033).
- **derived / aux** — provenance and signatures ride as non-sealed `aux/` members (#259, ADR-0042),
  or as sealed `derived_from` products (versioning, ADR-0036).

Looked at separately they are five features. Looked at together they are **one recursive shape**.

## Decision — the recursive node

Every level of Tessera is the **same node**: an addressable unit carrying three descriptors, composed
(not inherited) by *feature-by-presence* (ADR-0029):

1. **hash** — a `blake3` digest (a leaf's own; an interior node's Merkle/MMR root over its children).
2. **stats** — optional summary (`min/max/mean/std`, count, fill) — the same reduction `tsra stats`
   computes over an array, computed once per node and carried, so an overview never decodes the payload.
3. **referencing** — optional `(transform, unit, frame)` (ADR-0032): a chunk's offset in its array, an
   array's voxel→world affine (ADR-0030), a pyramid level's `at_level` affine, a product's `timestamp`.

The **same descriptor triple** applies at chunk, block, pyramid-level, product, and collection scale.
That is the unification: read/verify/prune/summarise/reference are **one set of operations** parameterised
by scale, not five bespoke code paths.

### The scales, concretely

| Scale | Node | hash | stats | referencing |
|---|---|---|---|---|
| chunk | a 64³ cube (#214) | `blake3(chunk)` | per-chunk min/max/… | index-offset in the array |
| block | an array/table (`BlockRef`) | block digest | (derived from chunk stats) | array affine (ADR-0030) |
| level | a pyramid step (#260) | derived block digest | reduced stats | `at_level` affine (ADR-0028) |
| product | a `.tsra` manifest | `manifest_hash` (seal) | product-level summary | `timestamp` / study |
| collection | an MMR of members (#223) | MMR root | cohort roll-up | study/cohort frame |

### What rides *inside the seal* vs *alongside* (the ADR-0042 rule)

- **Sealed** (in `manifest_hash`): the manifest, blocks (incl. pyramid levels + chunk-index), schema, `extra/`,
  sources. These are the "tracked" nodes — hashed, tamper-evident, deterministic.
- **Aux / seal-ignored** (#259/ADR-0042): the signature, ingest wall-clock, field-encryption envelope —
  carried in the one file (`aux/`) but excluded from the seal (the `.gitignore`-for-the-seal rule).

Derived *data* that must be integrity-linked (a pyramid level, a reconstruction) is a **sealed derived
block/product** with a `derived_from` edge (ADR-0036); derived *metadata* that can't be sealed without
breaking determinism or signature semantics is an **aux member**. The recursion draws that line the same
way at every scale.

## Fused streaming

Because every node carries `{hash, stats}`, the write engine (ADR-0026, tessera-io) can emit them in **one
pass**: as chunks stream through the encode ring, their hashes + stats are accumulated into the chunk-index,
folded into block digests, and (for the coarsest pyramid level) downsampled inline — so the pyramid,
chunk-index, and Merkle root are **byproducts of the write**, not extra passes. Reads invert it: the
coarsest level answers an overview, a stats query hits the carried summary, and a chunk-index prune skips
shards before any decode (the confirm-and-prune win of #214, extended up the scales).

## Consequences

- **One mental model + one API surface.** `verify` / `stats` / `slice` / `project` / prune are the same
  operations at chunk, level, product, collection scale — the CLI verbs already reflect this (`stats`,
  `slice`, `project` over arrays; `verify`, `inspect` over products; collections via #223).
- **Composition over inheritance (#216).** A node is defined by which descriptors are *present*, never by a
  type hierarchy — a bare index-space chunk and a georeferenced pyramid level differ only by which optional
  descriptor they carry.
- **This ADR is a synthesis, not new format bytes.** It states the invariant the children implement:
  #214 (chunk `{hash,stats}`), #260 (level), #259/ADR-0042 (aux vs sealed line), #223 (collection MMR),
  #216 (composition). Each lands independently; this pins that they compose into one recursive hierarchy.

## Non-goals

- Not a new container or a rewrite — the existing STORED-zip `.tsra` + manifest already expresses every
  scale; this ADR names the pattern so future layers slot in without a sixth bespoke mechanism.
- Not mandating a pyramid or chunk-index on every product — all optional, feature-by-presence.

## References
#215 (this), #214 (chunk-index), #260 (pyramid + projections), #259/ADR-0042 (aux / self-contained),
#223/ADR-0033 (collections), #216 (composition/N-D), ADR-0026 (streaming), ADR-0028 (multiscale downsample),
ADR-0029 (feature-by-presence), ADR-0030 (spatial), ADR-0032 (referenced coordinates), ADR-0036 (versioning).
