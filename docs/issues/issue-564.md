---
type: issue
state: open
created: 2026-10-16T16:50:36Z
updated: 2026-10-18T11:53:42Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/564
comments: 1
labels: feature, discussion
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-19T07:18:04.867Z
---

# [Issue 564]: [feat: first-class agentic actors[] block + W3C PROV mapping — producer is scalar, generation.config is opaque](https://github.com/vig-os/tessera/issues/564)

## Motivation

Agentic pipelines (LLM-coordinated code generation, review, operator approval) are now a
first-class producer category alongside DAQs, SIMs and recons. anatomist is building a
canine-CT parser whose steps are proposed, reviewed and merged through agents (Claude Code,
Codex, …) with a human approver — see MorePET/anatomist#86's proposal to map these onto
`generation.config.agent = {name, model, brief_sha, receipt_sha, pr}`.

`generation.config` can absorb this, but shoehorning it there is wrong for three reasons:

1. **`generation.config` is explicitly a generator-opaque bag** (ADR-0058 §2 — "the config
   keys are the generator's business and are never inspected by the engine"). The engine
   also validates only presence, not shape — the agent's identity (what we most want to
   compare, filter and audit on) stays untyped, free-text data.
2. **`producer` is scalar.** One `{tool, version, git_commit, …}` per product. Agentic
   generation is a *chain*: implementer agent → reviewer agent → human approver. One
   producer slot does not model that; the only honest option today is to drop the
   intermediate roles.
3. **Signing already carries one `signer`** (an ORCID iD or similar). An agent is **not**
   the signer — the human approver signs; the agent is what wrote the content that got
   signed. The two coexist, which the current model has no vocabulary for.

The gap is also externally visible: `white-paper.md:1662` says

> "The sources/ provenance DAG maps naturally to PROV-O (`wasDerivedFrom`, `wasGeneratedBy`)
>  in the RO-Crate export."

but `tessera-core/src/export.rs::ro_crate` emits only schema.org `isBasedOn` (and
DataCite `IsDerivedFrom`). No PROV mapping exists. Agentic producers are where this most
hurts — a reviewer at another site cannot see "model=claude-opus-4-7 was the generator;
owner@… acted on behalf of" without parsing a bag our engine declares opaque.

## Proposed shape

Sealed, additive, back-compat (same `#[serde(default)]` pattern as `producer`/`generation`):

```rust
pub struct Actor {
    pub role: String,            // "generator" | "implementer" | "reviewer" | "approver"
    pub kind: ActorKind,         // SoftwareAgent | Human | Organization
    #[serde(flatten)]
    pub body: ActorBody,         // enum discriminated by kind
}

pub enum ActorBody {
    SoftwareAgent {
        harness: String,                           // "claude-code", "codex", "custom"
        harness_version: Option<String>,
        model: Option<String>,                     // "claude-opus-4-7"
        sampling: BTreeMap<String, Value>,         // effort, temperature, top_p, …
        brief_sha: Option<String>,                 // blake3 of the system prompt / brief
        receipt_sha: Option<String>,               // blake3 of the agent's receipt / trace
        acted_on_behalf_of: Option<String>,        // prov:actedOnBehalfOf — the approver
    },
    Human { identity: String, acted_on_behalf_of: Option<String> },
    Organization { identity: String },
}

pub struct Manifest {
    ...
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actors: Vec<Actor>,
}
```

### Why "actors" and not just "agents"

PROV distinguishes `prov:Agent` from `prov:SoftwareAgent` and `prov:Person`. A chain
"model-X generated → model-Y reviewed → human-Z approved" needs all three kinds. The
sealed-vs-aux split (ADR-0042 principle) applies per-field:

- **Sealed** (identity-bearing, deterministic given the run): `role`, `kind`, `harness`,
  `harness_version`, `model`, `sampling.*`, `brief_sha`, `receipt_sha`, `acted_on_behalf_of`.
- **Unsealed `aux/actors.json`**: wall-clock per step, PR URL (changes on rename/merge,
  not identity), per-call token counts and costs, host.

### W3C PROV mapping (unlocks the RO-Crate white-paper claim)

`tessera-core/src/export.rs::ro_crate` adds:

- The product itself gets `"@type": ["Dataset", "prov:Entity"]`.
- Each `Actor` becomes an entity in `@graph`:
  - `prov:SoftwareAgent` with `prov:softwareVersion` for `SoftwareAgent`
  - `prov:Person` for `Human`
  - `prov:Organization` for `Organization`
- The product gains `prov:wasGeneratedBy` (one `prov:Activity`) with
  `prov:wasAssociatedWith` → each actor by `role`, `prov:actedOnBehalfOf` → the approver.
- `sources[]` gains `prov:wasDerivedFrom` beside the existing `isBasedOn` (both standards'
  idiomatic terms; readers consume whichever).

## Repro / motivation (concrete, reproducible)

Writing an anatomist-style job manifest + a derived product from the Python wheel today
(tessera dev sha fa581f56041b38a48a8d20f4a9d033505dad8185, 2026-10-16):

- `producer` is forced to the stock `tessera/0.0.0` stamp (see sibling issue on Python's
  missing `with_producer`); anatomist's own identity is not representable.
- The entire agent bag is written to `metadata.agent` as a JSON string, which `tessera
  inspect` does not print, and no CLI verb filters on. `generation.config` would be the
  less-wrong home, but it collapses the three agents into one opaque dict and still has
  no PROV mapping.

## Scope

- **P1** — add the sealed `actors[]` struct, back-compat (default-empty), with Python +
  CLI write paths behind it; `tessera inspect` prints actors like it now prints `generation`
  (#417). Signing continues to bind one `signer`, independent of `actors` — the two meet in
  `acted_on_behalf_of` on an actor, not through the signer field.
- **P2** — the RO-Crate + DataCite exporters emit PROV. One sentence in
  `docs/book/src/provenance-signing.md` to replace the current "producer answers who built
  this" with "producer + actors answer who" and point at the actors section.
- **P3** — reserved role names ("generator", "implementer", "reviewer", "approver",
  "owner") so a reader can filter cross-corpus. Keep the role field open; new roles are
  additive.

## Pitfalls

- The chain is still sealed state — an actor list that *tracks* the agentic process is
  provenance; an actor list that *steers* ingest is config and belongs in
  `generation.config`. The ADR owes this line explicitly.
- `receipt_sha` points at a receipt that may itself live elsewhere (another `.tsra`, an
  OCI blob, …). No format change needed to carry it — the hash is enough, and the receipt
  can be a separate `.tsra` with a `derived_from` edge back to this product's `id`.
- Not every agentic run has a human approver (unattended automation). `acted_on_behalf_of`
  stays optional.

Found while walking MorePET/anatomist's adoption path on `.tsra` as its agentic
provenance format.

## Related

- ADR-0058 — producer + generation (the sealed slots this parallels).
- ADR-0037 — signing (the signer identity, independent of actors).
- #417 — `tessera inspect` for `generation` (same operator-surface pattern applies).
- white-paper.md:1662 — the unimplemented PROV mapping promise.
---

# [Comment #1]() by [gerchowl]()

_Posted on October 18, 2026 at 11:53 AM_

Filed §11 phase sub-issues from ADR-0060 (merged in #586):

- #587 — refuse integers above 2^53 in sealed values
- P0a/P0b → #573 — forward-compatible manifest/collection/schema
- #574 — publish and commit keep sealed provenance
- P1a → #596 — sealed `actors` with validation and the carry rule
- P1b → #597 — write actors from the CLI and ingest specs
- P1e → #598 — actors under de-identification, and pseudonym minting
- P1c → #599 — actors and `tessera.digest` in the Python bindings
- P1d → #600 — the `aux/actors.json` member
- P2a → #601 — actors in the RO-Crate projection
- P2b → #602 — actors in the DataCite projection
- P3  → #603 — actor-role claim check plus pinned signer binding (closes #590)
- P3d → #604 — attested and unattested actors in `inspect`
- P3c → #605 — `tessera verify-change` binds products to an approved change

Order: #587 → #573 → #574 → #596 → #597 → #598 → #599 → #600 → #601 → #602 → #603 → #604 → #605. Each sub-issue carries its own `## Dependencies` and the "do not dispatch before its predecessors merge" rule. P4 is deferred per the ADR.


