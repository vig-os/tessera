---
type: issue
state: open
created: 2026-09-29T01:07:16Z
updated: 2026-09-29T06:53:00Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/482
comments: 0
labels: none
assignees: none
milestone: 0.1.0-beta
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:26.242Z
---

# [Issue 482]: [fix(core): no built-in schema sets `requires_generation` — ADR-0058 §3's recipe rule is never enforced](https://github.com/vig-os/tessera/issues/482)

Surfaced while generating the product-schema reference for #389 — which is the point of generating a
reference from the registry rather than writing one beside it: the table shows a `requires a recipe`
column, and **every one of the 14 built-in products says `no`**.

## The gap

ADR-0058 §3 introduces `ProductSchema::requires_generation` as the schema-declared "this product needs a
recipe" rule, and describes it as `true` "on schemas for computed products (a DAQ/SIM/recon output)".

The mechanism is fully built and enforced:

```rust
// tessera/crates/tessera-core/src/schema.rs:263
if self.requires_generation && m.generation.as_ref().is_none_or(|g| g.is_empty()) { … }
```

…and unit-tested (`requires_generation_is_a_schema_declared_block`, schema.rs:1190). But:

```console
$ grep -rn 'requires_generation: true' tessera/crates/
$      # no matches
```

So no built-in schema opts in. `sim`, `recon`, `dynamic_pet`, `sinogram` and friends can all seal with no
generation record at all — the ADR's rule exists as a lever nobody has pulled. A Monte-Carlo `sim` product
with no recipe is exactly the case §3 was written to prevent.

## Why it is not being fixed in #389

Setting the flag makes products that seal today **invalid**, which is a validation-policy change rather
than a docs change, and it needs a per-product decision:

- `sim` — almost certainly yes (a simulation without its seed/config is not reproducible).
- `recon` — probably yes, but a `recon` ingested from a vendor DICOM has no *recipe*, only provenance. The
  ADR's own ingest path would start failing unless vendor ingest synthesises a recipe, which is #403/§6a
  territory.
- `listmode` / `device_data` / `blob` — no: raw acquisition and preserved bytes are not computed.
- `calibration`, `transform`, `deformation_field`, `roi`, `spectrum`, `sinogram` — need a call each.

That per-product judgement, plus the migration question for already-sealed products, is what makes this
its own issue.

## Acceptance criteria

- [ ] A decision recorded per built-in product on whether it is "computed" for §3 purposes.
- [ ] `requires_generation: true` set on the products that are, with a test asserting each blocks without a
      recipe.
- [ ] The vendor-ingest paths that produce those products either synthesise a recipe or are explicitly
      exempted — whichever, `tessera ingest dicom|nifti|ge-hdf5` must still work end to end.
- [ ] Confirm against `tessera/corpus/` whether any committed fixture would newly fail (a corpus event if
      so); check rather than assume.
- [ ] The #389 schema reference's `requires a recipe` column then shows the real policy — no doc change
      needed, since it is generated.

## References

- ADR-0058 §3 · `tessera/crates/tessera-core/src/schema.rs` (`requires_generation`, `validate`)
- #389 (where it surfaced) · #403 / ADR-0056 §6a (the ingest-recipe question for `recon`)

