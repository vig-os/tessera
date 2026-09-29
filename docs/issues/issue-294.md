---
type: issue
state: closed
created: 2026-07-02T13:33:24Z
updated: 2026-09-28T17:54:48Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/294
comments: 2
labels: discussion, effort:large
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:55.729Z
---

# [Issue 294]: [spike: collection level semantics — non-opinionated domain-supplied level tag + living/frozen cohorts + FAIR mapping (Q2, split from #293)](https://github.com/vig-os/tessera/issues/294)

Split from the #293 recursion spike. #293 decided the **mechanism** (can collections nest — `kind` discriminator, MMR leaf domain-separation, recursive verify); it is being built as the hardened Q1 model. **This issue (Q2) is the level *semantics*** — deliberately deferred because it's a domain data-model that risks being designed wrong without real cohort/trial usage.

## Governing principle (decided) — the engine is level-agnostic

Any fixed level vocabulary is **opinionated** — "Study" and "exam/cohort/site/trial" alike. So:

- **The nesting mechanism says nothing about levels.** Tessera's core nouns stay generic: *product · collection · member · sub-collection · nest*. No `exam`/`cohort`/`study` enum in `tessera-core`.
- **Level meaning is domain data, per field** — a collection carries an **optional, open `level` tag** (a `Coded { _vocabulary, _code }` — the same pattern the format already uses for coded enum fields — or a free string), supplied by the domain, `None` by default; the engine carries it, never interprets it.
- This is RFC §12's spine philosophy one level up: **schema-driven, domain-agnostic; taxonomies are registered *data*, not engine code** — level vocabularies become domain **profiles/plugins** (like device-schema plugins).
- Payoff: this **dissolves the "ban Study" collision** — the engine opines *no* level name, so there's nothing to ban. A medical-imaging profile defines `exam/subject/cohort/site/trial`; other domains bring their own.

## Open questions for the spike

1. **`level` tag shape** — free string vs `Coded {_vocabulary,_code}` vs reference to a registered level-vocabulary. (Lean: `Coded`, reusing the existing pattern; a domain profile pins the allowed codes.)
2. **Living vs frozen cohorts** — enrollment churns; a cohort is a **mutable lineage handle (ADR-0036 ref) + frozen content-addressed snapshots** (DB-lock / interim / publication). Model the handle↔snapshot split; a frozen snapshot IS a sealed collection, the living handle is a ref. Don't conflate the manifest with "the cohort."
3. **Missing levels as a domain *vocabulary*, not engine enums** — the clinical review's `exam / subject-timeline / cohort-arm / site / trial` ladder ships as a **medical-imaging level profile (data)**, not baked into core. Open-ended depth.
4. **FAIR / Invenio mapping keyed off the domain level tag** (not engine enums): Record vs Community per level; DOIs at the citable levels **never at the org level**; `IsPartOf` → parent **concept-DOI** (not version-DOI); **nested RO-Crates** (crate-per-child) not monolithic `hasPart`; **k-anonymity on aggregate fields** at export.
5. **Capabilities the clinical review flagged** — exclusion sets *as members* (not deletions); patient de-dup across cohorts (stable pseudonymous **subject key**); **query-materialized cohorts** (the query is a persisted, re-runnable artifact whose result *becomes* a frozen snapshot); multi-modal grouping *within* an exam; site-scoped access / per-site keys.
6. **Corrections are explicit** — `--adopt`/`rebind` + audit diff; never auto-follow lineage tips (matches #292 custodian design + regulatory reality, 21 CFR Part 11 / ICH E6).

## Non-goals

- **Any engine-hardcoded level name or taxonomy in `tessera-core`.** Levels are domain data.
- Freezing the level model before real trial/cohort usage validates it.

## References

#293 (Q1 mechanism + 4-review consolidation) · ADR-0033 (collections) · ADR-0036 (versioning DAG / refs) · ADR-0042 (aux/) · RFC §12 (domain-agnostic engine; taxonomies as data) · #292 (custodian verbs)
---

# [Comment #1]() by [gerchowl]()

_Posted on July 2, 2026 at 02:45 PM_

## Decision (from design discussion): levels are **registered, versioned `CollectionSchema`s** — option (b)

Confirmed direction: collections get the **full schema treatment**, symmetric with product schemas, **including versioning**. Not a loose `Coded` tag.

### The two registries, one pattern
| | Core ships (generic) | Domains register (opinionated) |
|---|---|---|
| `.tsra` products | `recon`, `listmode`, `blob`, … | device/domain ProductSchemas *(exists, RFC §12)* |
| **collections** | **`dataset`, `project`** | **field-specific `CollectionSchema`s — this build** |

Same registry mechanism (entry-point plugins), same discipline (additive-only, stable ids, embedded-for-offline-validation, major-version-refused-not-mis-read).

### `acq` / `derived` is NOT a level — it's `Role`
Both are just blocks/products in a `.tsra`. `Role::Raw` = acq (a measurement fixed to a time, **non-byte-reproducible** → Compliance-WORM) · `Role::Derived` = computed (regenerable → Governance-WORM, writer-determinism applies). No new structure.

### The model
- **Engine (fixed, generic, taxonomy-agnostic):** exactly **two structural types — `product`, `collection` — plus recursion** (`kind: Product|Collection`, already shipped in #306). This never grows a taxonomy.
- **`dataset`** = a collection of products from one physical setup (replaces the patient-specific \"exam\").
- **`project`** = a collection of datasets *or* projects, **recursive** (replaces the whole cohort/site/trial ladder with one recursive concept; `project-of-projects` = the `kind: Collection` recursion).
- **Unlimited named levels via schema, zero new engine machinery:** a domain registers level-schemas that *name + constrain* the recursion. e.g. clinical registers `exam ⇒ products-only`, `subject ⇒ members are exam-collections`, `cohort ⇒ members are subject-collections`, … — all structurally `kind: Collection`, distinguished + validated by their `CollectionSchema`. Depth + naming + rules = schema; structure = mechanism.

### `CollectionSchema` (registered + versioned — mirrors `ProductSchema`)
```
CollectionSchema {
  id, schema_version,            // versioned; additive-only; stable id; never resurrect a retired id
  name, description,
  member_rule {                  // the structural constraint the engine validates
    allowed_member_kinds,        // [Product] (dataset) | [Collection] (project) | both
    allowed_member_schemas?,     // e.g. cohort ⇒ members conform to "subject"
    recursive,
  },
  fields[],                      // required/optional, like ProductSchema
}
```
- A collection carries **`collection_schema` + `schema_version`** (the product/schema_version analogue) → resolves the registered schema; **part of the collection's identity + seal**.
- Engine **validates the declared schema's `member_rule`** on seal/verify (member kinds, member-schema constraints, nesting) — whichever registered schema; stays taxonomy-agnostic.

### Versioning
- `CollectionSchema` versioned exactly like `ProductSchema`: additive evolution, stable ids, embedded in the product for **offline** validation, a future *major* refused not mis-read.
- The pinned `(collection_schema, schema_version)` is in the collection's `id_inputs`/seal → identity-bearing.
- Collections remain versionable on the ADR-0036 objects/refs DAG (already true) — orthogonal to schema versioning.

### Relationship to shipped code
- **#306 mechanism unchanged** — `kind: Product|Collection` + recursion *is* the dataset/project structure.
- **`level: Option<Coded>` (shipped in #306) is the transitional form** → evolves **additively** to `collection_schema` + `schema_version` resolving a registered `CollectionSchema`. No rework of the recursion mechanism.

### Build (this issue)
- [ ] `CollectionSchema` type + a registry (mirror `SchemaRegistry`), core `dataset` + `project` schemas with `member_rule`s.
- [ ] `Collection.collection_schema` + `schema_version` (additive; deprecate/map `level: Coded`); pin in `id_inputs`/seal.
- [ ] Engine validation of `member_rule` at seal + `verify` (member kinds / member-schema / recursion).
- [ ] Domain-plugin path (a registered opinionated ladder as the worked example, e.g. imaging `exam/subject/cohort/site/trial`).
- [ ] ADR (amends ADR-0033, relates ADR-0049) when scheduled for build.

Supersedes the earlier fixed-ladder framing (exam/subject/cohort/site/trial as engine levels): those become *one domain's registered vocabulary*, not core.

---

# [Comment #2]() by [gerchowl]()

_Posted on September 28, 2026 at 05:54 PM_

Closing as **done** — verified on `origin/dev` in the 2026-09-28 backlog triage.

Evidence: commit ceed652 'feat(core): collection level schemas — terminology as registered/versioned data (ADR-0050, #294) (#315)'.

https://claude.ai/code/session_01XdERKMVDAwfMJSKdTytNnK

