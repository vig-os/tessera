---
type: issue
state: open
created: 2026-10-17T08:13:07Z
updated: 2026-10-17T08:13:07Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/591
comments: 0
labels: audit-trail
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-18T06:56:49.537Z
---

# [Issue 591]: [feat(repo): a redacting commit to erase one version from a lineage (ADR-0040 §2.1 redact)](https://github.com/vig-os/tessera/issues/591)

## Problem

There's no way to erase **one version** of a lineage from a repository you control while keeping the lineage.
ADR-0060 (PR #586) needs this for an actor identity sealed into a version, and so does any sealed field that turns out
to hold personal data.

What the current verbs do:

- `commit` pins the old tip in a `supersedes` edge with its `content_hash` (`tessera-cli/src/version.rs:124`).
- `gc` treats every source `content_hash` that names an in-repo object as reachable (`tessera-io/src/repo.rs:389-394`),
  so the old version stays reachable from the new tip and is never collected.
- `forget` deletes the whole lineage ref and log (`repo.rs:296-308`), so a later `gc` drops **every** version.

So "commit without the person, then `forget` + `gc`" (ADR-0060's first draft, and ADR-0040 §2.1's shape) either
keeps the old version or loses the whole history.

## Proposal (needs an ADR-0040 §2.1 decision)

A redacting commit, the `tessera redact` verb ADR-0040 §2.1 names but nobody has built: it writes a new tip whose edge
to the redacted version is a **tombstone**, keeping the `manifest_hash` (provable lineage) but not the
`content_hash` pin `gc` follows. It then rewrites the edges in later versions that pin the redacted one, or refuses
when there are any. `gc` then reclaims the redacted manifest object. Blocks still referenced by other versions stay.

## Acceptance

Commit v1 (actor P), then v2 (without P), then `redact <lineage> <v1>` and `gc`: v1's manifest object is gone, v2
and the lineage remain, `provenance` reports v1 as a tombstone, and `verify` on v2 passes.

Out of scope: copies already distributed (only crypto-shred reaches them, ADR-0040 §2.2).

