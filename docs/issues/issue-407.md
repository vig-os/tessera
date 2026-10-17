---
type: issue
state: open
created: 2026-08-21T09:27:46Z
updated: 2026-10-16T17:15:53Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/407
comments: 1
labels: none
assignees: none
milestone: 0.1.0-beta
projects: none
parent: none
children: none
synced: 2026-10-17T06:32:51.184Z
---

# [Issue 407]: [Signed `aux/` appendix: bring forensic provenance into the ADR-0037 signature scope](https://github.com/vig-os/tessera/issues/407)

## Motivation

ADR-0056 §6a (#403, PR #405) decided the decoder identity is **provenance, not identity**: it is
recorded in `aux/provenance.json` rather than sealed, because the seal already brackets the ingest
transform (`ingested_from` source digest + `content_hash`) and a decoder id adds attribution, not
detection.

One lens dissented through both passes of the spike, and its objection is not answered — only scoped:

> *"An `aux/`-only record is unauthenticated. `SignedPayload` binds `manifest_hash` only, so an
> adversary re-packaging a `.tsra` flips the decoder claim without breaking any signature; and a
> stripped record is byte-indistinguishable from one that never existed."*

That is true, and it is **not specific to the decoder**. It applies equally to `ingested_at`,
`producer` and `host` — every forensic fact ADR-0042 deliberately placed outside the seal. The
principled fix is therefore not to promote one field into the seal (which §6a rejected on
meaning-vs-context grounds, and which would re-import the churn) but to give `aux/` its own integrity
story.

The archival lens's substrate argument is the sharpest statement of why this matters:

> *"Over 30 years, disk/tape/handover is the substrate; OCI is a distribution mechanism of the moment,
> and only when consumers pin digests rather than tags. Optimising integrity for the substrate that
> already provides it, and abandoning it on the one that does not, inverts the preservation posture."*

## Decision / proposed approach

Extend the ADR-0037 signing envelope with an **optional second scope** covering forensic `aux/`
members, so a signature can attest "these were the provenance facts at signing time" without making
`aux/` immutable or seal-bearing.

Sketch, to be argued not assumed:

1. A canonical digest over a **declared subset** of `aux/` — the forensic namespace
   (`aux/provenance.json` and any future forensic entries), explicitly **excluding**
   `aux/signatures/` to avoid the obvious circularity.
2. That digest becomes an optional bound field in `SignedPayload`, alongside `manifest_hash`. Absent
   ⇒ today's semantics exactly, so every existing signature stays valid.
3. `verify-sig` reports three states rather than two: seal-verified · seal+provenance-verified ·
   provenance-diverged-since-signing. The third is informational, never a hard verification failure —
   `aux/` must stay editable by design (ADR-0042), and a later annotation is not tampering.

The subtlety that makes this non-trivial: signing currently **writes into** `aux/signatures/`, so any
aux-covering digest must be computed over a namespace that excludes the signature entries, and the
ordering (stamp provenance → compute digest → sign → embed signature) must be pinned in the ADR.

## What already exists

- `tessera/crates/tessera-core/src/signing.rs:43-53` — `SignedPayload`, the envelope to extend
- `docs/adr/0037-signing-trust-model.md` — the envelope's design and its "bind the envelope, not the
  seal alone" history (the ADR-0037 §0 bug #1 fix is the precedent for adding bound fields)
- `docs/adr/0042-self-contained-tsra-aux-members.md` — the `aux/` namespace, its deliberate
  non-sealed status, and the reserved sub-namespaces
- `tessera/crates/tessera-io/src/provenance.rs:86-106` — `stamp_ingest_provenance`, which writes into
  an already-sealed `.tsra` (the write-ordering constraint above)
- `docs/adr/0056-…` §6a "The dissent, recorded" — the argument this issue answers

## Scope

**P0**
- [ ] Decide the covered namespace and the exclusion rule for `aux/signatures/`
- [ ] Optional bound digest in `SignedPayload`; absent ⇒ current semantics, all existing signatures valid
- [ ] Pin the write ordering (provenance before signing) in ADR-0037

**P1**
- [ ] `verify-sig` three-state reporting + a test that a post-signature aux edit is *detected* but
      does not fail seal verification
- [ ] Round-trip test: sign → push → pull → verify still reports provenance-verified

**P2**
- [ ] `--require-provenance-signed` for operators who want it as a hard gate

## Pitfalls

- **Circularity.** The signature lives in `aux/`. Covering all of `aux/` with a digest bound into the
  signature is self-referential; the namespace split must be explicit and tested.
- **Do not make `aux/` immutable.** ADR-0042's whole point is that aux is annotatable after sealing
  (this is how `tessera sign` works at all). A diverged provenance digest is *information*, not a
  verification failure — getting this wrong breaks signing itself.
- **Do not let this re-import wall-clock into identity.** `provenance.json` carries `ingested_at` and
  `host`; nothing here may reach `manifest_hash`, or re-ingest byte-identity dies (see the rejected
  `provenance_hash` alternative in ADR-0056).
- Signing is opt-in, so this does not make provenance universally tamper-evident — it makes it
  tamper-evident *for signed artifacts*. Say so plainly rather than overselling.

## Acceptance criteria

- [ ] A signed `.tsra` can attest its forensic provenance, with existing signatures unaffected
- [ ] Post-signature aux edits are detectable, and do not break seal verification
- [ ] ADR-0037 amended; ADR-0056 §6a's open gap updated to point here

## References

- #403 / PR #405 (ADR-0056 §6a — the decision that surfaced this), #406 (source-archive preservation)
- ADR-0037, ADR-0042, ADR-0056 §6a

Refs: #403
---

# [Comment #1]() by [gerchowl]()

_Posted on October 16, 2026 at 05:15 PM_

Dogfooding anatomist (MorePET/anatomist#91) at tessera `dev` `fa581f5`: a concrete use case for the signed-aux appendix beyond the ADR-0056 §6a decoder-id framing.

**Scenario.** Each dog's products (ingest + derived) are wrapped by `age`-encrypted whole-file envelopes (anatomist #91's emulation of ADR-0055). The outer `.sig.json` binds `{ciphertext_hash, recipients, signer}` and lets an auditor who holds **no** decryption key still prove the ciphertext is the one the signer attested to.

**What that auditor still can't prove without decrypting:** that a derived product descends from a specific parent. `derived_from <parent manifest_hash>` lives **inside** the sealed manifest, which is inside the ciphertext. So "lineage integrity" today reduces to "give me the key or trust me."

**Why this maps to #407.** The signed-aux appendix would let a producer emit a non-identifying lineage projection into `aux/` (e.g. a canonical `aux/lineage.json` with `{self_manifest_hash, derived_from: [parent_manifest_hash, …], supersedes: […]}`) and have the ADR-0037 signature cover it. For whole-file encryption, that same aux projection can also travel **outside** the ciphertext (next to the outer `.sig.json`) and be bound into the outer envelope — same digest, two carriers, one chain. The decoder/`producer`/`host` motivation of #407 still stands; this adds the "lineage without decrypt" surface anatomist specifically needs for #86.

Not asking to expand #407's scope — more that `aux/lineage.json` is a natural tenant of the covered namespace, and the whole-file encryption carrier (#378) is the second case where the appendix pays. If the aux namespace stays decoder/provenance-only, anatomist can file this as a sibling.

Repro inputs: anatomist #91 spike comment. Synthetic data only, no PHI.


