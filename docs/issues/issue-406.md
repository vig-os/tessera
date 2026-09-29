---
type: issue
state: open
created: 2026-08-21T08:48:52Z
updated: 2026-09-28T17:59:02Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/406
comments: 1
labels: none
assignees: none
milestone: 0.1.0-beta
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:38.992Z
---

# [Issue 406]: [Bind each sealed decoder pin to a content-addressed source archive (the seal's pointer must outlive crates.io)](https://github.com/vig-os/tessera/issues/406)

## Motivation

ADR-0056 §6a (#403, PR #405) decided that `ingest_decoder` seals a **build fact** — decoder name +
`=`-pinned version + resolved-decode-feature digest. That decision rests on the field being
*mechanically verifiable forever*: a reader can, in principle, rebuild the pinned decoder, feed it the
source, and confirm `content_hash` bit-for-bit.

The archival lens on the #403 spike accepted the decision but named the condition it depends on, and
that condition is **not currently met**:

> *"crates.io is a live service on a 30-year horizon. If arrow-rs 58.3.0 is yanked, or crates.io itself
> is not preserved to 2050, the semver becomes a pointer at a dead reference and the reader loses the
> ability to reconstruct the decoder. Option 1's archival guarantee is contingent on preservation
> infrastructure Tessera does not own."*

This is the one respect in which the rejected profile id was genuinely stronger: a profile is a
*specification* recoverable from in-repo ADR text, which is archived alongside the codebase, whereas a
version string is a *pointer* into third-party infrastructure. §6a records this as an open gap; this
issue is where it gets closed.

Note the asymmetry with Tessera's own code: `TESSERA_VERSION` names a contract whose source we archive
ourselves, in this repo, forever. Decoder pins name contracts we do not archive at all.

## Decision / proposed approach

Make each `=` pin on the seal path resolve to a **content-addressed, independently preserved source
blob**, so the sealed pointer has a durable referent:

1. On every seal-path decoder pin bump, resolve the crate to its published source tarball, compute its
   sha256, and commit `{crate, version, sha256, retrieval_hint}` to a tracked
   `tessera/corpus/ingest-source-digests.json`. `Cargo.lock` already carries the checksum — the new
   work is *lifting it into a reviewed, seal-adjacent artifact* and giving it a retrieval story.
2. Lodge the tarball in a WORM archive that outlives crates.io — Software Heritage is the obvious
   candidate (it already archives crates.io, so this may reduce to *recording the SWHID* rather than
   pushing anything).
3. Consider a fifth sealed format field, `ingest_decoder_source: {sha256, retrieval_hint}`, binding the
   sealed version string to a content-addressed blob. **This needs its own argument** — it is more
   sealed format surface, and ADR-0056 §6 sets a deliberately high bar ("each of them changes what the
   values mean"). A digest that only makes an existing sealed field *retrievable* may or may not clear
   that bar; deciding is part of this issue, not a foregone conclusion.
4. Gate it: absence of a source-digest entry for any decoder on the seal path fails CI, so the record
   cannot silently fall behind the pins.

## What already exists

- `docs/adr/0056-generic-ingest-normalise-vs-preserve.md` §6a — the decision this backstops, and its
  "Open gaps" entry naming this exact hole
- `docs/adr/0056-…` §5 H9 — the `=`-pin discipline the hook would attach to
- `docs/adr/0057-…` §5 — Gate B's committed feature snapshots; the same "commit a snapshot, diff it in
  CI" shape, one level out
- `tessera/Cargo.lock` — already carries per-crate checksums; the raw material
- ADR-0042 — the sealed-vs-`aux/` boundary any new sealed field must argue past

## Scope

**P0**
- [ ] Decide whether the source digest is a sealed format field or a tracked repo artifact only
- [ ] `tessera/corpus/ingest-source-digests.json` (or equivalent) generated + committed for every
      seal-path decoder pin
- [ ] CI gate: every seal-path pin has a current entry; a pin bump without one fails

**P1**
- [ ] Software Heritage (or equivalent) archival + recorded SWHID per pinned version
- [ ] `tessera verify --explain-decoder` or similar: given a sealed `ingest_decoder`, print the
      retrieval path a future reader would follow

**P2**
- [ ] Extend beyond decoders to the full seal path (vortex, zarrs, pcodec, zstd, blake3) — the same
      argument applies to the *encoder*, which is arguably more load-bearing

## Pitfalls

- **This can quietly become "mirror all of crates.io".** Scope it to the seal path — the crates whose
  behaviour can move a `content_hash` — and say so explicitly, or it never ships.
- A sealed source digest is **more sealed format surface**, and ADR-0056 §6's bar is high. If it does
  not change what the values *mean*, it may belong in a tracked repo artifact rather than the manifest.
  Argue it, don't assume it.
- Software Heritage already archives crates.io. If so, the honest deliverable may be *recording the
  SWHID* rather than performing an upload — verify before building an ingestion pipeline.
- A digest that nobody can act on is theatre. The retrieval story (what a 2050 reader actually *does*
  with the field) is the deliverable, not the hash.
- Yanked-crate handling: a yank does not remove the tarball today, but the policy must not assume that
  holds forever.

## Acceptance criteria

- [ ] Every `=`-pinned seal-path decoder has a committed, CI-enforced content-addressed source record
- [ ] A written retrieval procedure a reader can follow without crates.io
- [ ] ADR-0056's "Open gaps" entry updated to point at the resolution

## References

- ADR-0056 §6a + "Open gaps" (`docs/adr/0056-generic-ingest-normalise-vs-preserve.md`)
- ADR-0057 §5 (Gate B snapshot discipline)
- #403 (the decision this backstops), #405 (the PR)

Refs: #403
---

# [Comment #1]() by [gerchowl]()

_Posted on August 21, 2026 at 09:28 AM_

**Reframing after the second pass on #403.** This issue was filed while ADR-0056 §6a sealed the decoder
identity. §6a has since been reversed (PR #405): the decoder identity is **not sealed** — it is recorded
in `aux/provenance.json`, because the seal already brackets the transform via the `ingested_from` source
digest plus `content_hash`.

The issue still stands, with two adjustments:

- **The pointer that needs a durable referent is now in `aux/`, not the manifest.** That *weakens* the
  urgency slightly (a dangling reference in a diagnostic record is less serious than one in the seal)
  but does not remove it: §6a's answer to "an aux record is unauthenticated" is that a claimed decoder
  is **falsifiable** — you can re-decode the source under the claim and check it reproduces the sealed
  `content_hash`. That rebuttal only works while the decoder source is still obtainable. This issue is
  what keeps it obtainable, so it is now load-bearing for §6a's central argument rather than a nicety.
- **P0 item 1 is largely settled.** "Sealed format field vs tracked repo artifact" resolves toward the
  tracked artifact, since §6a declined to seal the decoder identity itself; sealing a digest *of* it
  would be inconsistent. The remaining question is narrower: the record's location and its CI gate.

Related: #407 (signed `aux/` appendix — the other half of making an aux-recorded decoder claim
trustworthy) and #408 (the canonicalisation hazards found in the same pass).

