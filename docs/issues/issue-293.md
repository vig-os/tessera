---
type: issue
state: closed
created: 2026-07-02T13:19:42Z
updated: 2026-09-28T17:54:45Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/293
comments: 2
labels: discussion, refactor, effort:medium
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:56.076Z
---

# [Issue 293]: [spike: collection recursion model — sub-collections (patient→cohort→consortium), identity folding + recursive verify](https://github.com/vig-os/tessera/issues/293)

## Motivation / why

Collections (ADR-0033, #223) are the blocker for the real hierarchy users want: **product (series/recon) → patient exam → cohort → consortium.** ADR-0033 §2 *promises* "logical collections nest by reference — and sub-collections, **recursively**," but the model is **flat**: `CollectionMember` is typed "a flat physical product," there's no member-kind discriminator, and `verify` (PR #282) opens `<reference>.tsra` — a *product*, never a child `collection.json`. So today you can build a patient-exam collection (products), but **not** a cohort (a collection of patient-collections). This spike decides the recursion model so the full feature becomes build-ready.

## Decision / proposed approach (best guess — refute or agree)

**Single `members` list + a `kind` discriminator**, not a separate `sub_collections` field:

- A collection already carries the *same* `{id, content_hash, manifest_hash}` triple as a product. So a **sub-collection is just another member pinned by `manifest_hash`** — `kind` only disambiguates (a) how to *resolve* it on disk (`.tsra` vs `collection.json`) and (b) how `verify` *recurses*.
- `CollectionMember { reference, manifest_hash, role, derived_from, kind: Product | Collection }`. `kind` defaults to `Product`, `#[serde(skip_serializing_if = default)]` → **existing collections serialize byte-identical (corpus-neutral)**.
- **`content_hash` (MMR) is unchanged** — it folds each member's `manifest_hash` regardless of kind (a leaf is a leaf). The parent seal transitively commits to the whole tree: child bytes change → child `manifest_hash` changes → parent `content_hash`/seal change. Tamper-evidence propagates for free (consistent with ADR-0028 MMR).
- **`verify` dispatches on `kind`:** `Product` → open `<reference>.tsra`, check `manifest_hash`; `Collection` → open child `collection.json`, verify *its* seal + recurse into *its* members. **Cycle guard** (a `seen` set of manifest_hashes) prevents infinite recursion.
- **On-disk resolution:** a member resolves to `<reference>.tsra` (product) or `<reference>.collection.json` (sub-collection) beside the parent (extends `prefix_layout`); offline-verify requires every child descriptor to be reachable/co-located.

**Alternative considered:** a separate `sub_collections: Vec<CollectionMember>` field. Cleaner typing, but splits the *ordered* member set across two lists (member order is significant + folded into `content_hash`) and complicates the MMR fold + the projections. Leaning against.

## Terminology (nail it — DICOM "Study" collision)

- **product** = series / recon (`.tsra`).
- **L1 collection** = one **patient exam** = all products of a scan session ≈ today's `study` field ≈ **DICOM "Study"** (Patient > Study > Series).
- **L2 collection-of-collections** = a **research cohort / trial** — the user's "study with multiple patients" (a *different* sense than DICOM Study).
- **L3** = multi-site consortium.

## What already exists

- `tessera-core/src/collection.rs` — `Collection` + `CollectionMember` + MMR `content_hash` + `CollectionBuilder`.
- `tessera-io/src/collection.rs` — 3 projections (`to_rocrate` / `to_oci_index` / `prefix_layout`).
- PR #282 — `collection inspect/ls/verify` (flat, member-`manifest_hash` mismatch detection).
- ADR-0042 non-sealed `aux/` (for `aux/origin.json`); ADR-0036 objects/refs DAG (collections are versionable nodes).

## Scope

**P0 — the recursion decision (this spike):**
- [ ] `kind: Product | Collection` vs `sub_collections` field — decide + rationale.
- [ ] MMR identity folding for a child collection (leaf-by-manifest_hash?).
- [ ] Recursive `verify` + **cycle guard** + on-disk resolution rule.
- [ ] Back-compat / corpus-neutrality (serde default, existing collections unchanged).
- [ ] Terminology mapping (product / patient-exam / cohort / consortium).

**P1 — verb surfaces (spec, lighter):**
- [ ] Authoring/curation: `collection create / add / from-query` (define over EXISTING `.tsra`, incl. a stat-predicate cohort).
- [ ] Custodian/versioning: `outdated` (pinned member's lineage has a newer version) · `rebind` (re-pin → new collection version + diff/consistency proof) · `aux/origin.json` stamping on pull (#292).

**P2 — projection nesting:**
- [ ] RO-Crate nested `hasPart` (Dataset-in-Dataset) · OCI **index-of-indexes** · S3-prefix recursion · InvenioRDM Community/subcommunity vs nested-Record mapping.

## Pitfalls

- **Cycles** — a collection (transitively) referencing an ancestor → infinite `verify`. Needs a `seen` guard; is a cycle an error or just pruned?
- **Offline-verify must hold** — recursive verify can't require the network. Every child `collection.json` must be locally reachable (co-located / embedded), else `verify` degrades to "can't fully verify" — how is that surfaced honestly?
- **Role/WORM at the collection level** — is a *sub-collection* member Raw or Derived? The role→WORM mapping (raw=Compliance) is product-shaped; ambiguous for a nested collection.
- **On-disk ambiguity** — `<id>.tsra` vs `<id>.collection.json` vs `<id>/collection.json`; the resolution rule must be deterministic and the 3 projections must all recurse.
- **Depth/perf over cloud** — a deep tree of small `collection.json`s = many round-trips; the tail-prefetch / range-read story is product-shaped.
- **Back-compat trap** — if `kind`'s default *ever* serializes, every existing collection hash moves (corpus regen). Must be provably skip-on-default.

## Acceptance criteria

- [ ] A decided recursion model (member-kind + identity-folding + recursive-verify + cycle guard) with written rationale vs the `sub_collections` alternative.
- [ ] The patient/study/cohort terminology mapping recorded.
- [ ] Authoring + custodian verb surface sketched.
- [ ] Proven corpus-neutral (existing collections serialize byte-identical).
- [ ] Enough to write the ADR-0033 amendment + build.

## References

ADR-0033 (collections + 3 projections) · ADR-0028 (MMR identity) · ADR-0036 (versioning DAG) · ADR-0042 (aux/) · #223 · #282 · #291 (transport) · #292 (ownership boundary) · `tessera-core/src/collection.rs` · `tessera-io/src/collection.rs`
---

# [Comment #1]() by [gerchowl]()

_Posted on July 2, 2026 at 01:23 PM_

## Spike review consolidation (4 fresh-agent reviews: crypto/Merkle · schema-evolution · FAIR/RDM · clinical-workflow)

### Where they agree (strong signal → bank it)
- **Recursion is right, and `kind`-on-a-single-`members`-list beats a `sub_collections` field** (crypto + schema). The ordered member list the MMR folds must not be split.
- **Corpus-neutral for `manifest_hash`** is achievable (skip-on-default `kind`) — but see the tension below re: `content_hash`.
- **Ban "Study" at the user surface** — FAIR and clinical reviewers *independently* insisted (DICOM Study = one exam; trialists' "study" = the cohort). Use **series / exam / subject / cohort / site / trial**. Non-negotiable.
- **Corrections never auto-propagate** — explicit re-version / `--adopt` + audit diff (FAIR + clinical + the earlier custodian design).

### The real decision points (disagreement / reframe)
1. **Leaf domain-separation vs corpus-neutrality (the sharp one).** Crypto: MANDATORY — make the MMR leaf preimage `BLAKE3(domain_tag ‖ kind_tag ‖ manifest_hash)`, else an inclusion proof for a hash is ambiguous product-vs-collection and `kind` is flippable without moving the MMR. BUT that changes **`content_hash` for every existing collection** → conflicts with schema's corpus-neutral claim (which holds only for `manifest_hash`/JSON). **Resolution needed:** accept a one-time, `tessera_version`-gated collection `content_hash` reconstruction (clean) vs. asymmetric "legacy product-leaf = bare hash, collection-leaf = tagged" (corpus-neutral but hacky).
2. **Fixed L1/L2/L3 vs open-ended levels.** Clinical: the 4-level hardcode is wrong — **missing `subject-timeline`** (same patient across timepoints — every trial needs it) **and `site`** (multi-center access/IRB/QC). Real ladder = series → exam → subject → cohort/arm → site → trial (≈6). Recommendation: **don't hardcode depth** — carry an open-ended typed `level_kind` tag; nesting depth is open.
3. **Living vs frozen cohorts.** Clinical: enrollment churns daily; a pure-immutable collection mismatches it as UX. Need a **mutable cohort *handle* (ADR-0036 lineage/ref) + frozen content-addressed *snapshots*** (DB-lock / interim / publication). CoW already supports it — just don't conflate the frozen manifest with "the cohort."

### New pitfalls surfaced (beyond the issue body)
- `Option<Kind>` = two representations for one state = hash-instability landmine → use a **bare `#[non_exhaustive]` enum + free-fn skip predicate**; **refuse unknown `kind` gated by `tessera_version`** (never `serde(other)`/treat-as-opaque).
- Typed builder handles `add_product(ProductHandle)` / `add_subcollection(CollectionHandle)` — carry `(reference, manifest_hash)` as one unit (kills wrong-ref/wrong-hash).
- `derived_from` = **peers only** (no cross-nesting-boundary refs).
- Filename-suffix (`.tsra` vs `.collection.json`) as type-tag = **second source of truth** → `kind` authoritative, resolver derives suffix.
- Cycle `seen`-guard = defense-in-depth, **not** security (content-addressing is acyclic by construction).
- FAIR export: **`IsPartOf` must target the parent's *concept-DOI*, not a version-DOI** (else every correction orphans children — the "FAIR-broken" item); DOIs at L0/L1/L2 **never L3**; **nested RO-Crates** (crate-per-exam) not monolithic `hasPart` at L2+; **k-anonymity on aggregate fields** at export (cohort pages leak n / age-sex histograms).
- Invenio mapping per level: L0 product & L1 exam → **Record**; L2 cohort → **Community (+ optional citable Record)**; L3 consortium → **Community only**.
- Missing capabilities flagged: multi-modal grouping *within* an exam; patient de-dup across cohorts (stable pseudonymous subject key); exclusion sets as members; site-scoped access; query-materialized cohort as a persisted, re-runnable artifact whose result *becomes* a frozen snapshot.

### Verdict
Recursion **confirmed**; core model (`kind` discriminator) **holds** — with **3 mandatory hardenings** (leaf domain-separation · refuse-unknown-kind/`non_exhaustive` · kill filename-type-tag) and **2 scope expansions** (open-ended typed `level_kind` + the missing subject/site levels · living-handle vs frozen-snapshot). Terminology fix (ban "Study") is unanimous + cheap. FAIR export mapping is well-specified for P2.

---

# [Comment #2]() by [gerchowl]()

_Posted on September 28, 2026 at 05:54 PM_

Closing as **done** — verified on `origin/dev` in the 2026-09-28 backlog triage.

Evidence: commit a2534bc 'feat(core): recursive-collection mechanism — MemberKind + domain-separated leaf + level tag (ADR-0049 Part 1) (#306)' + 6971bef 'recursive collection verify (ADR-0049 Part 2) (#309)'.

https://claude.ai/code/session_01XdERKMVDAwfMJSKdTytNnK

