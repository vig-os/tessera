---
type: issue
state: open
created: 2026-09-08T07:30:11Z
updated: 2026-09-28T17:59:08Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/413
comments: 0
labels: none
assignees: none
milestone: 0.1.0-beta
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:37.478Z
---

# [Issue 413]: [Schema-declared recipe keys: make `ingest_decoder` self-describing and required-when-claimed](https://github.com/vig-os/tessera/issues/413)

## Motivation

ADR-0056 §6a (#403, PR #405) puts the decoder identity in the sealed provenance recipe bag under the
well-known key `ingest_decoder`. Every lens on that spike named the same residual and initially accepted
it as the price:

> *"The bag is non-opinionated, so nothing forces `ingest_decoder` to be present, well-named or
> well-formed. A 2050 reader gets a stable name to reach for and no assurance anyone used it."*

§6a now argues that residual conflates **two** gaps, both closable with mechanisms this project already
has — and that accepting it was a mistake driven by treating the bag as schema-less.

- **Gap A — discoverability.** `seal()` already embeds the resolved product schema into the manifest
  (`product.rs:136-140`) so a `.tsra` carries its own contract. Nothing stops the builtin `table` /
  `array` schemas from *describing* the recipe key. Then the artifact explains `ingest_decoder` to a
  reader holding nothing but the file — no external ADR, no registry lookup.
- **Gap B — presence.** The parsimony objection that ruled out a mandatory *field* was to requiring a
  value non-Rust producers **cannot compute**. It was never an objection to requiring the **key**.
  "Name the decoder you used" is universally computable; only the resolved-feature digest is
  Rust-shaped.

So: **require the key, recommend the triple.**

## Decision / proposed approach

Extend the schema layer — **not** the manifest — so recipe keys can be described and required the same
way metadata fields already are.

1. **`ProductSchema` describes generation keys.** A list of specs (id, description, dtype, and the
   existing `required` / `recommended` severity ladder) covering entries in the generation config bag,
   alongside today's `fields` which cover `manifest.metadata`. Embedded at seal like the rest of the
   schema, so the artifact is self-describing about its recipe.
2. **`validate()` enforces them per key.** `ProductSchema.requires_generation` today is all-or-nothing
   ("this product needs *a* recipe") and is checked on the same path as required fields, on the stated
   principle that *"the policy lives in the schema, not the engine"*. This is that pattern one level
   finer: required *keys within* the recipe.
3. **The builtin `table` / `array` schemas declare `ingest_decoder` as required**, with the
   build-honest triple documented as the recommended form. Tessera's ingest always writes the triple; a
   foreign producer satisfies the requirement with whatever honestly names its decoder
   (`"pyarrow 15.0.0"`).
4. **Shape validation when present.** A malformed value should fail rather than pass as an opaque blob,
   even where the key itself is only recommended.
5. **Operator backstop: `verify --require-recipe`** — same idiom as §7's `--require-classified` and
   ADR-0037's `--require-signer`, for archives that want this hard at their own boundary.

## What already exists

- `tessera/crates/tessera-core/src/product.rs:136-140` — schema embedding at seal (the whole basis of
  Gap A's fix)
- `tessera/crates/tessera-core/src/schema.rs:57-80` — `FieldSpec` with the `required` / `recommended`
  ladder; `:95` / `:119` the constructors; `validate()` around `:226`
- `ProductSchema.requires_generation` + its `validate()` arm — on
  `feature/324-generation-provenance-adr`, the coarser version of exactly this rule
- ADR-0056 §7 — the builtin `table` / `array` schemas this would extend, and the `--require-classified`
  precedent

## Scope

**P0**
- [ ] `ProductSchema` can describe generation-config keys (id / description / dtype / severity)
- [ ] `validate()` enforces required recipe keys; recommended ones surface in the existing warn tier
- [ ] `table` / `array` declare `ingest_decoder` required; the triple documented as recommended form

**P1**
- [ ] Shape validation for a present-but-malformed value
- [ ] `verify --require-recipe`
- [ ] A test that a foreign-shaped value (`"pyarrow 15.0.0"`) satisfies the requirement — the
      non-Rust-producer path must be demonstrably open, not merely asserted

**P2**
- [ ] Consider whether other well-known recipe keys deserve the same treatment (the companion
      canonicalisation digest, if it lands)

## Pitfalls

- **Do not let "required key" become "required Rust triple".** The whole point is that the obligation is
  universally satisfiable. If review pressure ever narrows the accepted shape, the parsimony objection
  §6a answered comes straight back. The P1 foreign-value test is the guard.
- **Open-world products must stay unconstrained.** Products unknown to the registry embed no schema and
  carry no obligation — that escape hatch is load-bearing and must not be closed by accident.
- Adding required anything to a builtin schema is a **validation-behaviour change**: products that
  previously validated may stop. Scope it to the new `table` / `array` schemas, which have no existing
  artifacts, rather than retrofitting vendor schemas.
- This is schema surface, and schema surface is versioned. A later change to the key's meaning needs a
  schema version bump, not a silent redefinition.

## Acceptance criteria

- [ ] A sealed generic-ingest `.tsra` explains `ingest_decoder` from its own embedded schema
- [ ] A `table` / `array` product with no decoder recipe key fails validation
- [ ] A non-Rust-shaped decoder value satisfies the requirement, proven by test
- [ ] ADR-0056's open gap updated to point here

## Dependencies

Gated on **#409** — `ProductSchema.requires_generation` and the generation bag itself are on
`feature/324-generation-provenance-adr` and have not reached `dev`.

## References

- ADR-0056 §6a "Closing the bag residual" + §7; #403 / PR #405
- #409 (provenance model must reach `dev`), #386 (ingest impl that would carry this), #408, #407, #406

Refs: #403
