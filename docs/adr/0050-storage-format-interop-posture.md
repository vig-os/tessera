# ADR-0050 — Storage-format interop posture: sealed product, OME-NGFF at the array layer

**Status:** Proposed (spike for #287; from the design pass in `docs/spikes/tsra-explorer.md` § "Prior art
& reuse"). Decides where Tessera aligns with the cloud-native-array ecosystems (OME-Zarr, Icechunk) and
where it deliberately diverges. No new format bytes — pins an interop posture the facade/bridge issues
(#289/#290) implement.

## Context

Two adjacent ecosystems solve "cloud-native N-D arrays" and tempt adoption:

- **OME-Zarr / OME-NGFF** — a layout convention over a **Zarr store**: a keyed bag of chunk-objects plus
  imaging metadata (`multiscales`, `axes`, `coordinateTransformations`). The de-facto open format for
  bioimaging, spreading to pathology/radiology. Mature renderer ecosystem (viv, neuroglancer, vtk.js,
  napari).
- **Icechunk** (by **Earthmover**, Apache-2.0 Rust crate) — "git for Zarr": snapshots + branches/tags +
  chunk manifests over object storage, with **serializable multi-writer transactions**. Convergent with
  our CoW model (ADR-0036) on the surface.

The question #287 forces: **adopt one as Tessera's storage format, or keep the `.tsra` sealed product and
*adapt*?** The answer turns on separating what is *essential* to Tessera from what is *incidental*.

**The framing: OME-Zarr/Icechunk are *stores* (a directory/prefix of many objects); `.tsra` is a
*product* (one sealed, content-addressed, signed, versioned file of heterogeneous blocks).** That
container-level difference is Tessera's reason to exist (ADR-0022/0036/0037/0042/0043). The *array-chunk*
difference is not: Tessera arrays are already **Zarr v3** (ADR-0023), and the array metadata already
carries the pieces NGFF needs — pyramid levels (#260) and voxel→world affine (ADR-0030, which already
notes OME-Zarr per-level transforms are derivable).

| Axis | OME-Zarr / Icechunk | `.tsra` | essential to Tessera? |
|---|---|---|---|
| Container | many chunk-objects (dir/prefix) | **one sealed file**, manifest + packed blocks | **yes** |
| Identity | NGFF: n/a · Icechunk: **random 12-byte snapshot IDs** | **content-addressed** (`manifest_hash`) | **yes** |
| Integrity / signing | none / none | Merkle seal (ADR-0028/0043) / ed25519 (ADR-0037) | **yes** |
| Content kinds | Zarr arrays only | Array + **Vortex Table** + Blob (ADR-0023/0024/0038) | **yes** |
| Regulatory | FAIR-for-bioimaging | + WORM · PHI/de-id · audit (ADR-0033/0040) | **yes** |
| Coordinates | `axes` + `coordinateTransformations` | affine referencing (ADR-0030/0032) | overlapping |
| Multiscale | `multiscales` | pyramid (#260, ADR-0043) | overlapping — maps cleanly |
| **Array chunk encoding** | **Zarr v3 chunks + codecs** | **Zarr v3 (ADR-0023)** | **incidental — already the same** |

Almost every difference is essential (the container); the one incidental difference (array chunks) is
already aligned. That asymmetry decides the posture.

## Decision

1. **Keep the `.tsra` sealed-product container as the format.** Content-addressed, Merkle-sealed,
   ed25519-signed, CoW-versioned, heterogeneous (Array + Vortex Table + Blob). Adopting a multi-object
   store — and, for Icechunk, **random** snapshot IDs with **no integrity hash or signature** and
   **arrays only** — would dissolve exactly these differentiators. Content-addressing over random IDs is a
   *deliberate* divergence for a verifiable archival product, not an oversight.

2. **Adopt OME-NGFF at the Array-block layer, and expose it via a read facade — not by changing the
   container.** Tessera arrays are already Zarr v3; align the array metadata (pyramid → `multiscales`,
   affine → `coordinateTransformations`, via the `ome_zarr_metadata` crate) so that a **`tsra serve` store
   facade** presents the chunks *inside* a sealed `.tsra` as a standard Zarr/NGFF store. The mechanism is a
   chunk-key → in-container **range read**, driven by the existing chunk index (#214, ADR-0028) — i.e. a
   kerchunk / VirtualiZarr-style reference set. Result: viv / neuroglancer / vtk.js / napari read a
   `.tsra` **unmodified**, with near-zero renderer forking (#289). Arbitrary-angle MIP/projection is a
   *renderer* capability (ray-marchers), **not** an NGFF-spec feature, so it needs no format fork.

3. **Do not adopt Icechunk as the format; borrow and bridge instead.** Borrow its **multi-writer
   transaction protocol** where our single-writer CoW is weaker (#288), and its **virtual-chunk** pattern
   (external byte-range refs — which validates the facade) (#289). Optionally **bridge** import/export
   (Icechunk snapshot → sealed `.tsra`; a tsra lineage → Icechunk repo) to reach the versioned-Zarr world
   (#290). Tessera is not reinventing Icechunk — it targets the sealed, signed, verifiable, heterogeneous
   *product* Icechunk does not.

## Consequences

**Positive**
- Inherits the mature OME-Zarr renderer ecosystem for **~zero renderer code** — the work is a metadata
  shim + a store facade over bytes we already hold, not a re-encode.
- Keeps every Tessera differentiator (seal, signature, content-addressing, Vortex tables, provenance,
  WORM/PHI) untouched — the facade is additive and read-only; `content_hash`/`manifest_hash` are
  unchanged (consistent with the ADR-0042 aux/non-sealed line).
- Standards-native interop points (S3, OCI, NGFF, DataCite/RO-Crate, plus an optional Icechunk bridge)
  compose into the unbundled platform (#299) without a proprietary lock-in.

**Negative / risks**
- The facade must emit NGFF-conformant metadata and translate chunk keys → ranges; the **NGFF transform
  vocabulary is still evolving** (affine support is recent), so some Tessera referencing may ride as an
  extension alongside the standard fields.
- A facade is a surface to maintain and test (round-trip a `.tsra` through a real Zarr renderer — the
  feasibility gate in #289).
- Two write paths remain conceptually distinct: arrays (Zarr v3, NGFF-facing) vs. tables (Vortex,
  Arrow/DataFusion-facing). This is intended (data-shape routing), not a defect.

## Alternatives considered

- **Adopt OME-Zarr/Icechunk as the whole format** — rejected: dissolves the sealed product (loses
  content-addressing, integrity, signing, single-file portability, and non-array content).
- **Bespoke internal array format + a fat adaptor** — rejected: Tessera arrays *are* Zarr v3, so a fat
  adaptor is self-inflicted drift for no gain.
- **wasm-decode in the browser (read Vortex/zarrs client-side)** — rejected as the interop path: ships
  whole volumes over the wire and does not cross-compile today; the facade keeps decode native (see the
  compute·data·viz split in the explorer spike).

## Related

ADR-0022 (container), ADR-0023 (array = Zarr v3), ADR-0024 (table = Vortex), ADR-0028/0043 (Merkle +
unified hierarchy + chunk index), ADR-0030/0032 (spatial referencing / quantities), ADR-0036 (versioning
& audit), ADR-0037 (signing), ADR-0042 (aux/non-sealed members). Issues #287 (this ADR), #288 (Icechunk
txn), #289 (virtual chunks / NGFF facade), #290 (Icechunk bridge); design in `docs/spikes/tsra-explorer.md`.
