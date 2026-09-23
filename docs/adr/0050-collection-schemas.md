# ADR-0050 — Collection level schemas: terminology as registered, versioned data (#294)

**Status:** Proposed (2026-07-02, P0 as-built) — decided in the #294 design thread. **Amends** ADR-0033
(collections), **relates** ADR-0049 (recursion mechanism), mirrors the `ProductSchema` model (RFC §12).

## Context

ADR-0049 gave collections **structure** — two member kinds (`Product`, `Collection`) + recursion — but
no **semantics**: the engine knew "this collection contains those" but nothing about whether a level is
a *dataset*, a *project*, a *cohort*, an *exam*. The reviewers wanted a fixed clinical ladder
(exam→subject→cohort→site→trial) baked into the engine. That was rejected: **any fixed level vocabulary
is opinionated**, and the format already has a mechanism for domain-specific meaning — the schema
registry. So terminology becomes **schema**, symmetric with product schemas.

## Decision

**Levels are registered, versioned `CollectionSchema`s — the collection analogue of `ProductSchema`.**

- **`acq` / `derived` is not a level — it is [`Role`]** (Raw = acquisition, non-byte-reproducible →
  Compliance-WORM; Derived = regenerable → Governance-WORM). Both are products in a `.tsra`.
- **The engine is generic + fixed:** two structural member kinds + recursion (ADR-0049). It never grows
  a taxonomy.
- **`CollectionSchema { collection_schema, version, description, member_rule }`** where `member_rule`
  carries `allowed_member_kinds` (+ `recursive`) — the structural constraint the engine validates.
- **A `Collection` carries `collection_schema` (+ version via the schema)** — the `product`/`schema`
  analogue, **sealed** (part of `manifest_hash`); defaults to the permissive generic `"collection"`.
- **Core ships three levels:** `collection` (generic, any kind — the default), `dataset` (products
  only — a collection of acq + derived from one physical setup), `project` (sub-collections only,
  recursive). A **domain registers its own** opinionated levels (`exam`, `cohort`, `site`, `trial`, …)
  — unlimited named levels via schema, **zero new engine machinery** (each is structurally a
  `kind: Collection`, named + constrained by its schema).
- **The engine validates `member_rule` at seal + verify**, against the built-in
  `CollectionSchemaRegistry`; an **unknown level is permitted** (open-world — a domain level this binary
  doesn't ship is not an error), exactly like unknown product schemas.

### Worked shape
```
core (generic):        product → dataset → project → project → …
domain (registered):   recon(product) → exam → subject → cohort → site → trial
                        (each a kind:Collection, named + rule-constrained by its CollectionSchema)
```

## What's built (P0) vs deferred (P1)

**P0 (this change):** `MemberRule` + `CollectionSchema` + `CollectionSchemaRegistry` (`collection` /
`dataset` / `project`); `Collection.collection_schema` (sealed, default `collection`);
`CollectionBuilder::with_collection_schema`; `member_rule` (allowed-kinds) validated at `seal` +
`Collection::verify`; the `level: Coded` tag (ADR-0049 §5) retained as an orthogonal human display tag.
CLI `collection inspect` surfaces the schema.

**P1 (deferred):** `allowed_member_schemas` (cross-file rule — "a cohort's members are subject
collections" — needs the child resolved → a verify-time check in `tessera-io`/CLI, not core seal-time);
`CollectionSchema.fields[]` (collections gain a `metadata` map first); **embedding** the schema in the
descriptor (self-describing/offline, as products embed `Manifest.schema`); binding `collection_schema`
into `id_inputs` (identity-bearing, not just seal-bearing); a worked **domain-plugin** level ladder
(e.g. an imaging `exam/subject/cohort/site/trial` profile as registered data); the `level: Coded` →
`collection_schema` consolidation.

## Consequences

- **Seal-affecting (additive):** `collection_schema` enters the canonical bytes → every collection's
  `manifest_hash` shifts. No pinned collection goldens exist (the corpus pins only products), so no
  regen; existing collections deserialize to the permissive `collection` default and validate.
- Terminology is now **stored + versioned as schema** — the reviewers' fixed ladder is demoted to *one
  domain's registered vocabulary*, and the engine stays maximally non-opinionated.

## References

#294 (decision thread) · ADR-0033 (collections) · ADR-0049 (recursion mechanism) · RFC §12
(domain-agnostic engine; schemas as data) · `tessera-core/src/collection.rs` (`CollectionSchema`,
`CollectionSchemaRegistry`) · `tessera-core/src/schema.rs` (the `ProductSchema` this mirrors).
