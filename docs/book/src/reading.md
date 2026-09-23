# Reading & navigating

Everything here is read-only and reads *into* the sealed `.tsra` — no unpacking. The natural order when
you're handed an unfamiliar product: **is it intact → does it conform → what's inside → give me the
data**. This walkthrough runs against the conformance corpus (`multiblock_study.tsra`: an array `volume`,
a table `roi`, and a provenance edge):

{{#include ../../../tessera/crates/tessera-cli/tests/cmd/navigating.trycmd}}

- `verify` — the seal + every block digest (integrity before trust).
- `schema` — validates the product against its embedded, versioned schema (`--json` dumps it).
- `tree` / `ls` — the whole hierarchy, or one node's children (a block, `meta`, `sources`, `extra`).
- `read` — table rows as CSV/TSV/NDJSON, projecting just the columns you ask for; the read spans blocks
  (a logical column can be assembled across a multi-block product).
- `export` — a FAIR discovery record (RO-Crate JSON-LD) for a catalog or data portal
  (see [Provenance, signing & trust](./provenance-signing.md) for the FAIR story).

For array blocks specifically — numeric overview, single-voxel probe, projections — see
[Arrays](./arrays.md).
