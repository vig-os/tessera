# ADR-0049 — Recursive collections: the nesting mechanism (amends ADR-0033 §2)

**Status:** Proposed (2026-07-02) — mechanism decided via spike #293 (4 fresh-context reviews: crypto/Merkle · schema-evolution · FAIR/RDM · clinical-workflow); implementation pending. **Amends** ADR-0033 §2. Relates ADR-0028 (MMR identity), ADR-0036 (versioning DAG/refs), ADR-0042 (non-sealed `aux/`), RFC §12 (domain-agnostic engine). Level *semantics* (what a nesting level means, living/frozen cohorts, FAIR mapping) are deliberately **out of scope** → #294.

## Context

ADR-0033 §2 promised "logical collections nest by reference — and **sub-collections, recursively**," but the model is flat: `CollectionMember` is a "flat physical product," there is no member-kind discriminator, and `verify` opens `<reference>.tsra` (a product), never a child `collection.json`. So a patient-exam collection (products) is expressible; a cohort (a collection of collections) is not. This ADR decides the **mechanism** so recursion is build-ready. The *semantics* of levels are domain data, split to #294.

## Decision

### 1. One `members` list + a `kind` discriminator (not a `sub_collections` field)
A collection already carries the same `{id, content_hash, manifest_hash}` triple as a product, so a sub-collection is **just another member pinned by its `manifest_hash`**. `CollectionMember` gains:

```rust
#[derive(…)] #[non_exhaustive]
pub enum MemberKind { Product, Collection }   // bare enum, NOT Option<MemberKind>

pub struct CollectionMember {
    pub reference: String,
    pub manifest_hash: String,
    pub role: Role,
    pub derived_from: Vec<String>,
    #[serde(default, skip_serializing_if = "MemberKind::is_product")]
    pub kind: MemberKind,      // default Product; skipped-on-default → JSON byte-identical
}
```

- **`Option<MemberKind>` is rejected** — two representations for one state (`None` vs `Some(Product)`) is a hash-instability landmine.
- **A separate `sub_collections` field is rejected** — it splits the *ordered* member list the MMR folds (order is identity).
- **`skip_serializing_if` keeps `manifest_hash`/JSON corpus-neutral** — existing all-product collections serialize byte-identical under RFC 8785 JCS (an omitted key contributes zero bytes). *(Gated by a golden byte-equality test: existing collections unchanged **and** a `Collection`-kind member emits the key.)*
- **Unknown `kind` is refused, gated by `tessera_version`** — never `#[serde(other)]`-swallowed or treated-as-opaque (a content-addressed FAIR format needs to *understand* members). `#[non_exhaustive]` so downstream matchers can't lock the variant set (a third kind — `ExternalRef`/`Virtual` — stays additive).

### 2. MMR leaf domain-separation (the one crypto hardening — seal-affecting)
The parent `content_hash` MMR leaf preimage becomes **`BLAKE3(LEAF_DOMAIN ‖ kind_tag ‖ manifest_hash)`** (`kind_tag`: `0x00` product, `0x01` collection), not the bare `manifest_hash`.

- **Why:** without a kind tag in the leaf, an MMR **inclusion proof** for a hash `H` is ambiguous between "product `H`" and "collection `H`," and a malicious catalog could flip `kind` in the JSON without moving the MMR. Domain-separating the leaf makes inclusion proofs **kind-bound** and future-proofs a third kind. *(bare-`manifest_hash` leaf rejected for exactly this ambiguity.)*
- **Corpus impact — accepted, versioned:** this changes `content_hash` for **every** collection (flat ones too), so it is **not** content-neutral. Resolution: a **`tessera_version`-gated `content_hash` reconstruction** — a one-time collection-corpus regen. Justified: pre-1.0, few collection fixtures, and the soundness is load-bearing. *(The asymmetric "legacy product-leaf = bare hash" alternative was rejected — corpus-neutral but leaves product inclusion proofs ambiguous, defeating the point.)*
- `manifest_hash` (the JSON seal) is unaffected by this — only the MMR `content_hash` construction changes.

### 3. Recursive `verify` (offline; dispatch on `kind`)
- `Product` → open `<reference>.tsra`, check `manifest_hash` == pinned.
- `Collection` → open child `<reference>.collection.json`, verify **its** seal, then **recurse** into its members.
- **Cycle guard** = a `seen`-set of `manifest_hash`es — kept as **defense-in-depth against malformed/hand-edited catalogs, not a security property** (content-addressing is acyclic *by construction*: a member cannot pin an ancestor hash that does not exist yet without breaking BLAKE3 preimage resistance).
- **Offline-verify must hold** — recursion touches no network; every child descriptor must be locally reachable/co-located. When a child is absent, `verify` reports *"cannot fully verify (child unreachable)"* — never a silent pass.

### 4. Resolution & builder ergonomics
- **`kind` is authoritative; the resolver derives the on-disk suffix** (`<ref>.tsra` vs `<ref>.collection.json`) — the filename suffix is **never** a second source of truth for type.
- **Typed builder handles** carry `(reference, manifest_hash)` as one unit (kills the wrong-ref/wrong-hash bug class):
  `add_product(&ProductHandle, derived_from)` · `add_subcollection(&CollectionHandle)`; raw `add_member` stays `pub(crate)`.
- **`derived_from` is peers-only** — a cross-nesting-boundary dependency must be materialized as a member of the enclosing collection, not a dangling reference.

### 5. The engine stays level-agnostic (RFC §12)
Any fixed level vocabulary is opinionated — "Study" *and* "exam/cohort/site/trial" alike. So the mechanism carries **no** level enum: core nouns stay generic (**product · collection · member · sub-collection · nest**). A collection carries an **optional, open, domain-supplied `level` tag** (`Coded { _vocabulary, _code }`, the format's existing coded-enum pattern; `None` by default) — the engine carries it, never interprets it. Level taxonomies are **domain profiles/data** (#294), like device-schema plugins. This also dissolves the DICOM-"Study" collision — the engine opines no level name at all.

## Consequences

- **One-time collection `content_hash` regen** (versioned) from the leaf domain-separation; `manifest_hash` corpus stays neutral.
- The three projections (RO-Crate / OCI-index / S3-prefix) must **recurse** — P2, tracked with #294's FAIR mapping (nested crates; `IsPartOf`→concept-DOI; DOIs at citable levels only; k-anonymity on aggregates).
- Level *semantics*, living-vs-frozen cohorts, and the domain level vocabulary → **#294** (kept out of the mechanism deliberately).
- Inclusion/consistency proofs compose across nesting **because** leaves are domain-separated (§2).

## Alternatives considered (and why they lost)

| Alternative | Verdict |
|---|---|
| `sub_collections: Vec<…>` field | ✗ splits the ordered member list the MMR folds |
| `Option<MemberKind>` (None = product) | ✗ two representations for one state → hash-instability landmine |
| bare `manifest_hash` MMR leaf (no kind tag) | ✗ inclusion proofs ambiguous product-vs-collection; `kind` flippable without moving the MMR |
| asymmetric "legacy product-leaf = bare hash" | ✗ corpus-neutral but leaves product proofs ambiguous — defeats §2 |
| a fixed `LevelKind::{Exam,Cohort,Site,Trial}` enum | ✗ opinionated; violates RFC §12 domain-agnosticism → open `level` tag instead (#294) |

## References

Spike #293 (issue + 4-review consolidation comment) · #294 (level semantics) · ADR-0033 (collections) · ADR-0028 (MMR identity + proofs) · ADR-0036 (versioning DAG/refs) · ADR-0042 (`aux/`) · RFC §12 · `tessera-core/src/collection.rs` · `tessera-io/src/collection.rs`
