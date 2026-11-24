---
type: issue
state: open
created: 2026-11-24T10:39:09Z
updated: 2026-11-24T10:39:28Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/964
comments: 1
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-24T11:59:38.769Z
---

# [Issue 964]: [Long-term evidence: RFC 4998 Evidence Record Syntax (archive timestamps renewed before certs/algorithms expire)](https://github.com/vig-os/tessera/issues/964)

## Motivation
Retention of 10–20 years outlives signing certificates and possibly hash/signature algorithms. A signature verified today may not be verifiable in 2040. RFC 4998 (Evidence Record Syntax, ERS; RFC 6283 for XML) solves this with hash-tree-based archive timestamps that are renewed periodically (timestamp renewal and hash-tree renewal), and fits tessera's Merkle structure.

## Proposal
- An evidence record per product, or per collection via one hash tree over many `manifest_hash`es, stored as an unsealed `aux/evidence/` member or a sidecar.
- A `tessera evidence renew` operation: re-timestamps before the TSA certificate expires (timestamp renewal), and re-hashes with a new algorithm when the old one weakens (hash-tree renewal; uses the hash agility from #526).
- `verify --evidence` proves existence and integrity at the original sealing time, through the whole renewal chain.

## Pitfalls
- Renewal is an operational duty: it needs a scheduler and alerting (in kit: a kit-flow job), or the chain breaks silently.
- Collection-level trees must survive members being deleted (crypto-shred), so the proof must not require the deleted content.

## Acceptance
- [ ] ERS create/renew/verify with goldens; [ ] renewal across a simulated algorithm change; [ ] works after crypto-shred of a member.

Refs #209, #526, #796, the qualified-seal issue.
---

# [Comment #1]() by [gerchowl]()

_Posted on November 24, 2026 at 10:39 AM_

Related: qualified seal #963; migration record #965.

