---
type: issue
state: open
created: 2026-08-07T14:11:22Z
updated: 2026-09-28T18:00:07Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/365
comments: 0
labels: priority:medium, effort:large, area:core, spike
assignees: none
milestone: backlog / research
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:44.382Z
---

# [Issue 365]: [spike: derived metadata caches + spine/block boundary (ADR-0054) — RDM catalog, aux hash-stamped cache, schema-projection guard](https://github.com/vig-os/tessera/issues/365)

## Spike: derived metadata caches + the spine/block boundary (ADR-0053)

Design artefact: **`docs/adr/0053-derived-caches-and-the-spine-block-boundary.md`** (Status: Proposed).
Extends ADR-0042 (non-sealed `aux/`) and draws ADR-0022 §6's implicit spine/block line explicitly.
Motivated by the RDM/catalog design discussion off #324: *how does a collection present in a MinIO/S3
research data store, and should a `.tsra` carry its metadata pre-shaped as Vortex for fast ingestion?*

### The one boundary, applied at four altitudes

Four asks that are really one rule (**sealed JSON = ground truth; every columnar/joined/indexed form
is an unsealed derived cache stamped with the seal it mirrors; freshness is a hash *comparison*, never
a re-hash of bytes**):

1. **Spine stays canonical JSON; bulk tabular metadata is a *block*.** Vortex must never be the sealed
   form — it has no canonical bytes by construction (adaptive ALP/dict/FoR per chunk, version-dependent),
   so a codec upgrade would move `manifest_hash` and break every seal. Corollary: homogeneous+large
   metadata (per-frame DICOM tags, big dictionaries) → a Vortex table block, digest-sealed like any block.
2. **Derived caches, uniform drift rule.** `aux/metadata.arrow` (per product), `<content_hash>.catalog.arrow`
   (per collection), RDM index rows — each carries `built_from = <sealed hash>`; read trusts iff
   `built_from == source seal`, else rebuilds. Additive, unsealed → conformance corpus byte-identical.
3. **RDM = content-addressed projects→items table, NOT S3 object tags.** Tags are mutable and live
   outside the seal (torches "verify offline forever"). The `project ⊃ dataset ⊃ product` recursion maps
   1:1 onto `collections`/`members`/`products` tables, rebuildable from the bucket alone.
4. **Tooling is the surface.** `tessera info`/`ls`/`cat manifest` = the `git cat-file -p` render;
   `tessera edit` = validate→re-canonicalise→re-seal. No byte-format change; the aux cache is the fast backing.

### The schema-projection guard (keeps it "1-schema-of-many", not a PET/radiology bag)

The `products` catalog columns are **projected from the member's registered `ProductSchema.fields:
Vec<FieldSpec>`** (`tessera-core/src/schema.rs`) — one column per `FieldSpec.id`, carrying its
`dtype`/`unit`. `FieldSpec.sensitivity` (ADR-0040) gates what a *shared* catalog surfaces (an
`Identifying` field is withheld by default — resolves the #267 export-PHI-leak at the source).
**The catalog builder contains zero hardcoded domain field names.** PET/radiology is one registered
schema whose FieldSpecs happen to name `patient_id`/`study_date`; the builder never learns those names.
This is the ADR-0052 "engine holds no domain field" rule, extended to the index.

### Load-bearing claims to falsify before landing

- **Can a stale cache ever be read as fresh?** Must be provably no. Test: mutate a source seal, leave a
  stale cache in place, assert the reader **rebuilds** rather than trusts. Freshness never inferred from
  mtime/presence.
- **Can a block-codec upgrade move a seal?** Must be no — the seal is over JCS-canonical JSON of the
  spine + block *digests*; the spine has no codec dependency at all.
- **Does any domain field name appear in catalog/cache code?** `grep` for `patient_id`/`study_date`/DICOM
  tag names in the catalog builder must come back empty.

### Scope

**In:** the `aux/metadata.arrow` writer + `built_from` stamp (per-product); `tessera collection catalog`
(the projects→items table, tier 1); a thin walk-bucket→build-catalog RDM adapter; the three
falsification tests above. All unsealed → corpus untouched.

**Deferred to its own ADR:** cross-product *data* query (join event columns across members) via a
feature-gated DuckDB tier over the union of member Vortex tables, materialised as `<content_hash>.duckdb`
under the same drift rule. Not built until a real query needs it.

### Relates to

ADR-0042 (aux boundary, generalised) · ADR-0022 §6 (spine/block line made explicit) · ADR-0049/0050
(collection recursion = catalog hierarchy) · ADR-0052 (schema-driven, engine-holds-no-domain-field) ·
ADR-0040 §1 + #267 (sensitivity-gated catalog columns) · #292 (thin-waist: own format+projection+rule,
delegate store) · #307 (Column annotation gap — sibling on the data tier) · #299/#304/#302 (the RDM/query cluster).

_Design already aligned in discussion; this issue tracks the spike → falsify → land cycle._

