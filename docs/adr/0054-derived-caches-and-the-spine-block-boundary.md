# ADR-0054 — Derived metadata caches and the spine/block boundary

Status: **Proposed** (2026-08-07). Extends ADR-0042 (non-sealed `aux/` members) with a general
rule for *derived columnar caches*, and draws the line ADR-0022 §6 left implicit: which metadata
belongs in the **sealed JSON spine** versus a **Vortex table block**. Motivated by the RDM /
catalog design discussion (#324 follow-on): "how does a collection present in a MinIO S3 research
data store, and should a `.tsra` carry its metadata pre-shaped as Vortex for fast ingestion?"

## Context — three questions that are one question

A research-data-management layer over collections raises what look like three separate asks:

1. **Per-`.tsra` fast metadata** — reading a collection of N products should not JSON-parse N
   manifests to answer "which products, what patient, what role".
2. **Per-collection query** — a "SQL over a collection" that joins/unions member metadata (and
   eventually member *data*) into one queryable surface.
3. **RDM presentation** — how a collection materialises in an S3/MinIO store: via mutable object
   **tags**, or as a **projects→items table**.

And underneath them a fourth, sharper one surfaced in design review: **should the metadata be
Vortex-native to begin with** — JSON demoted to a rendering the tooling prints on demand (à la
`git cat-file -p`), since "JSON is just badly-compressed text and it's bytes on disk either way"?

These are not four features. They are one boundary applied at four altitudes. This ADR fixes that
boundary so the RDM work — and every future cache — composes from a single rule instead of four
ad-hoc mechanisms.

## The load-bearing fact: the seal is over a *canonicalization*, not over file bytes

`manifest_hash = blake3(JCS-canonical-bytes of the manifest, with manifest_hash excluded)`
(RFC 8785). The identity of a `.tsra` is a function of a **frozen, codec-independent
canonicalization** of its logical metadata — *not* of whatever bytes happen to sit on disk. Every
decision below follows from protecting that property.

## Decision 1 — Sealed spine stays canonical JSON; bulk tabular metadata is a *block*

The sealed manifest (`id`, hashes, `schema` ref, `blocks` descriptors, `sources`/provenance edges,
and a handful of schema-governed scalar fields) **remains canonical JSON**. Vortex — or any
adaptive columnar codec — **must never be the sealed representation of the spine**, for one
decisive reason plus three supporting ones:

- **Decisive: Vortex has no canonical form, by construction.** Its value proposition is *adaptive*
  encoding — it chooses ALP / dictionary / frame-of-reference / bitpacking per chunk from heuristics
  that depend on the data *and the library version*. (Tessera already hit this: the "ALP-exclusion determinism
  fix" was Vortex codec choices leaking into hashes.) If Vortex were the sealed form, a Vortex
  upgrade that improves compression would change `manifest_hash` → **break every seal in the
  archive.** JSON+JCS decouples identity from codec: the canonical bytes are a function of the
  *data*, not of a compression library. This decoupling is *why the seal is trustworthy across
  time*, and no adaptive columnar format can provide it. (Deterministic CBOR/dCBOR *does* have a
  canonicalization spec and is the only real alternative — but for a 6 KB document it buys nothing
  and costs grep/diff/archival readability. Not worth it. Vortex specifically is disqualified.)
- **Archival "verify offline forever."** A JSON manifest is readable in 50 years by `cat`/`jq`/any
  language with zero Tessera code. A Vortex-sealed manifest makes the *root of trust* depend on one
  evolving Rust codec staying alive and byte-stable — a regression on FAIR's F/A/R.
- **Diff / audit.** ADR-0036's "metadata edit = 1 object, audit-trailed" and code review of a
  metadata change both rely on `git diff` showing text. A binary spine makes both blind.
- **Size is a non-argument for the spine.** The DP01 time-markers manifest is **6 KB**. Columnar
  compression saves kilobytes on a structural document whose *payload* (gigabytes) is already
  Vortex. And JSON *is* structured — a tree — which is the right shape for a heterogeneous spine
  (nested provenance, mixed-type fields). Columnar is for *homogeneous rows*.

**The corollary is the useful half.** When metadata is *bulk and homogeneous* — thousands of DICOM
per-frame tags, large dictionaries, per-event annotations — that is **data, not spine**. It goes
into a **Vortex table block**, sealed by its block digest like any other block (no special
mechanism; the format already does this for dictionary columns and per-frame tables). The promotion
threshold:

> **Promote a metadata field out of the manifest into a table block when it is (a) homogeneous rows
> and (b) large.** Heterogeneous, scalar, identity-bearing → stays a JSON spine field. Repeated,
> tabular, bulk → becomes a block.

This is what keeps the spine small, textual, and sealable *even as* metadata volume grows.

## Decision 2 — Every columnar / joined / indexed form is a derived cache, stamped with the seal it mirrors

Generalising ADR-0042's `aux/` boundary: **any pre-shaped, compressed, or joined representation of
metadata is an unsealed derived cache — never authoritative, always rebuildable, and stamped with
the sealed hash it was built from.** Freshness is a hash *comparison*, never a re-hash of payload
bytes.

Three concrete instances, one rule:

| Cache | Lives at | `built_from` | Stale when |
|---|---|---|---|
| Per-product metadata cache | `aux/metadata.arrow` (unsealed, ADR-0042 aux) | the product's `manifest_hash` | `cache.built_from ≠ manifest.manifest_hash` |
| Per-collection catalog | `<content_hash>.catalog.arrow` (delegated store) | the collection's `content_hash` | `built_from ≠ collection.content_hash` OR membership changed |
| RDM index row | RDM store (Postgres / DuckDB / S3-Select) | source seal | same hash comparison |

Read rule, uniform at every altitude:

```text
if cache.built_from == <sealed hash of source>  → trust the cache (skip the JSON/manifest parse)
else                                             → stale: ignore and rebuild from the sealed source
```

O(1) per artifact. The seal already proved the JSON authentic once; the cache only has to declare
*which* seal it mirrors. It is derivable, throwaway, and never the source of truth — exactly the
ADR-0042 sealed/unsealed line, extended from "one file" to "the whole store."

## Decision 3 — RDM presentation is a content-addressed table, not object tags

A collection materialises in S3/MinIO as **content-addressed objects + a rebuildable catalog
table**, *not* as object tags.

- **Object tags are disqualified for identity/provenance.** They are capped (~10/object), flat,
  mutable, exact-match-only, and — fatally — they live in the bucket's mutable metadata, **outside
  the seal**. Putting `patient_id` or lineage in a tag means truth lives in something editable
  without breaking any hash. That torches "verify offline forever." Tags, if used at all, are
  console breadcrumbs only.
- **The collection model is already a recursive table-of-tables** (`project ⊃ dataset ⊃ product`,
  ADR-0049/0050), which maps one-to-one onto a relational catalog:

  ```
  collections(id, collection_schema, study, content_hash, manifest_hash)
  members(collection_id, reference, role, kind, derived_from)   -- edges, from collection.json
  products(id, manifest_hash, schema, <schema-projected fields…>) -- rows, from aux/metadata.arrow
  ```

- **S3 is just addressing**: `s3://bucket/<id>.tsra`, `s3://bucket/<id>.collection.json`. The RDM
  builds the catalog (a Decision-2 cache keyed on `content_hash`) by reading `collection.json` for
  edges and each `aux/metadata.arrow` for rows. The catalog is **fully rebuildable from the bucket
  alone** — no external DB is ever authoritative.

Per the thin-waist ownership rule (#292): **Tessera owns the format + the projection function
(`manifest → catalog row`) + the drift rule; it delegates the store** (which DB, which index).

> **Terminology note.** "Catalog" here means the **queryable projects→items table** the RDM builds.
> It is distinct from `tessera-io`'s existing `MemberMetaKind::Catalog`, which names the
> `collection.json` *descriptor object* that closes a prefix/OCI projection. Different layer,
> different noun; this ADR's catalog is a derived index, not a container entry.

### The schema-projection guard (the "1-schema-of-many" invariant)

The `products` table's columns **MUST be projected from the member's registered `ProductSchema`,
never a hardcoded field set.** Concretely, for each member the catalog builder resolves its
`schema` → `ProductSchema.fields: Vec<FieldSpec>` (`tessera-core/src/schema.rs`) and emits one
column per `FieldSpec.id`, carrying that field's `dtype`/`unit`. Two schema-driven filters ride
along, both already in the type:

- **`FieldSpec.sensitivity`** (ADR-0040 §1) gates what a *shared* catalog may surface: an
  `Identifying` field is not projected into a browsable cross-tenant index by default (this is the
  #267 export-PHI-leak hazard, resolved at the source instead of per-exporter). A local/private
  catalog may include them; the tier decides, not a hardcoded column list.
- **`FieldSpec.inherit`** (ADR-0058) means a value present on a derived member's row may have been
  filled from a parent — the catalog records it as a normal column; provenance of *where it came
  from* stays in the sealed `sources`, not duplicated into the catalog.

This is the single discipline that keeps the RDM domain-agnostic: **PET/radiology is one registered
schema whose `FieldSpec`s happen to name `patient_id`/`study_date`/etc.; the catalog builder never
learns those names.** A new domain registers its own schema and its columns appear with zero
catalog-code change — exactly the ADR-0058 engine-holds-no-domain-field rule, extended to the
index. (Note: the block-column annotation triad `short_name`/`description`/`unit` already lives on
`block::table::Column`; the catalog guard is about *metadata* fields — `FieldSpec` — not data
columns, though the same "annotation is schema data, not engine code" principle governs both, cf.
#307.)

## Decision 4 — Tooling is the read/write surface; the byte format is an implementation detail

The RDM ask "let `.tsra` self-serve edit/read via info/ls" is **right and orthogonal to the
encoding** — it requires no change to the sealed byte format:

- `tessera info` / `ls` / `cat manifest` = the sanctioned **rendered** view — the `git cat-file -p`
  of a `.tsra`. Humans and OS tooling get whatever projection they ask for.
- `tessera edit` = the sanctioned **write** path: validates against schema, re-canonicalises,
  re-seals. Nobody hand-edits raw bytes.
- The per-product `aux/metadata.arrow` cache (Decision 2) is the **fast backing store** for that
  tooling, so `ls`/`info` over a 10k-member collection reads columnar, not 10k JSON parses.

This delivers everything the "Vortex-native metadata" proposal reached for — speed, structured
query, tooling-mediated access — **without** making a compression library the root of identity.

## The one-liner

**The manifest is source code; Vortex is the object file.** Keep source in canonical text
(readable, diffable, codec-independent, archival); compile to columnar (`aux/metadata.arrow` +
bulk blocks) for speed; make `tessera info` / `edit` the `cat-file -p` that mediates both. **The
moment the object file becomes authoritative, the seals are hostage to a codec — so it never is.**

## The load-bearing invariant (to falsify before landing)

**No codec upgrade, and no cache, can ever change or be mistaken for a seal.** Concretely, both
must hold:

1. **Spine codec-independence — and *why* existing seals survive a codec upgrade.** Be precise
   about two tiers with different guarantees:
   - **Block tier is byte-exact, NOT codec-independent.** A block digest is `blake3(encoded
     payload bytes as written)` (`hash.rs:15`; `tampered_block_payload_fails_on_read`). Re-encoding
     the same logical table with a different Vortex codec → different bytes → different digest.
     This is *intended* (ADR-0038 bit-faithful preservation; the S15 "pin codec versions" caveat).
   - **Existing seals survive a Vortex upgrade because sealed bytes are immutable and never
     re-encoded** — not because digests are codec-agnostic. The frozen payload keeps its digest, so
     `content_hash`/`manifest_hash` are unchanged. Re-encoding the same logical data with a new
     codec is *a new product with a new content_hash*, by design.
   - **The spine's canonicalization IS codec-independent** — JCS-canonical JSON over metadata + the
     digest *strings* has zero codec dependency. That is the property this ADR protects: "never seal
     in Vortex" is about the *spine*, where a codec would otherwise make identity hostage to a
     compression library. The block tier is intentionally byte-exact and out of that scope.
2. **Stale-is-never-fresh** — a cache whose source seal has moved MUST fail the `built_from`
   comparison. The stamp is *inside* the cache and *compared* on every read; it is never assumed,
   and freshness is never inferred from mtime, presence, or a re-hash of payload bytes.
3. **No domain field name in the catalog builder** — the `products` columns are projected from the
   member's registered `ProductSchema.fields`; the builder contains zero hardcoded field names.
   Adding a domain must require only registering a schema, not editing catalog code. The test:
   build a catalog for a product of a synthetic schema whose fields are *not* PET/DICOM names and
   assert every field appears as a column with its `dtype`/`unit`, and that an `Identifying` field
   is withheld from a shared-scope catalog.

The primary review question for the spike: *can a stale cache ever be read as fresh?* The answer
must be provably no, and the test suite must assert it (mutate a source seal, leave a stale cache
in place, assert the reader rebuilds rather than trusts). The secondary review question: *does any
domain field name appear in catalog/cache code?* — grep must come back empty.

## Consequences

- **New (additive, all unsealed):** `aux/metadata.arrow` writer + `built_from` stamp on the per-
  product path; `tessera collection catalog` building the projects→items table; a thin
  "walk-bucket → build-catalog" RDM adapter. None touch the sealed region → the conformance corpus
  (`corpus/corpus.json` + `corpus/files/*.tsra`) is **byte-identical**, exactly as ADR-0042
  required for aux members.
- **Tooling:** `info` / `ls` / `edit` gain the columnar backing store; behaviour unchanged when the
  cache is absent or stale (falls back to the JSON manifest).
- **Deferred (own ADR):** cross-product *data* query (join event columns across members) via a
  feature-gated DuckDB tier over the union of member Vortex tables, materialised as a
  `<content_hash>.duckdb` cache under the same drift rule. Not built until a real query needs it;
  the metadata catalog (tier 1) is what the RDM browses.

## Relation to other ADRs

- **ADR-0042 (non-sealed aux)** — this ADR *generalises* its sealed/unsealed line from "one file"
  to "any derived cache anywhere," and reuses the exact aux mechanism for `aux/metadata.arrow`.
- **ADR-0022 (container)** — draws §6's implicit spine/block line explicitly, with a promotion
  threshold.
- **ADR-0036 (versioning/audit)** — preserves text-diffable metadata edits; the catalog is a
  projection over the object store, not a competing source of truth.
- **ADR-0049 / ADR-0050 (recursive collections / collection schemas)** — the `project ⊃ dataset ⊃
  product` recursion *is* the catalog's table hierarchy; no new taxonomy is introduced.
- **ADR-0002 §4 (cloud reads)** — the RDM catalog is what turns cohort prune-before-fetch from an
  imperative per-member loop into a query over a rebuildable index.
- **#292 (thin-waist ownership)** — Tessera owns format + projection + drift rule; store/index/DB
  are delegated, consistent with "delegate cache."

## Non-goals

- **Not making Vortex (or any codec) the sealed metadata form.** The whole ADR exists to forbid it.
- **Not making any cache authoritative.** Caches are rebuildable projections; the sealed `.tsra` /
  `collection.json` is always ground truth.
- **Not putting identity/provenance in S3 tags.** Content-addressed objects + rebuildable catalog
  only.
- **Not hardcoding a domain column set into the catalog.** Columns are projected from the registered
  `ProductSchema.fields`; a domain (PET/radiology, genomics, anything) is one schema of many and the
  catalog builder never names its fields. This is the guard that keeps the RDM from becoming a
  radiology bag.
- **Not building the DuckDB cross-product *data* query here.** Deferred to its own ADR; only the
  metadata catalog (tier 1) is in scope.
