---
type: issue
state: open
created: 2026-11-24T10:39:07Z
updated: 2026-11-24T10:39:07Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/963
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-24T11:59:39.396Z
---

# [Issue 963]: [Qualified electronic seal backend: X.509 (ZertES/eIDAS) seal over a SHA-256 fixity hash, with a qualified RFC 3161 timestamp](https://github.com/vig-os/tessera/issues/963)

## Motivation
Converted records (e.g. DICOM ingested to .tsra, kept pixel-identical, originals not retained) need evidentiary weight in Swiss/EU proceedings. Swiss business-record rules (GeBüV Art. 9–10) accept documented migration with integrity protection; a **qualified electronic seal + qualified timestamp** (ZertES; eIDAS in the EU) makes that record strong evidence. tessera signs with ed25519 only today (ADR-0037); qualified seals use X.509 certificates (ECDSA P-256 / RSA) whose keys live in a qualified signature creation device at a recognised provider (remote signing APIs).

## Proposal
- An additive signature backend `x509-qseal`: detached CMS/CAdES (or JAdES) signature over the manifest, carried as `aux/signatures/<key_id>.sig` like ed25519 signatures, never mutating identity.
- Signed digest: the **SHA-256 fixity co-hash** of the manifest (depends on #526); blake3 stays the identity hash.
- Qualified **RFC 3161 timestamp** token over the same digest (makes the RFC-3161 item of #209 concrete); TSA URL and policy OID recorded.
- Verification: certificate chain to a configured trust list (Swiss/EU trusted lists), revocation info captured at signing time (OCSP/CRL embedded) so later verification does not depend on the CA being online.
- Key custody stays outside tessera: the signer is a pluggable trait; kit-sign (nerdmachines/kit#121) provides the remote-QSCD adapter.

## Pitfalls
- ed25519 is not accepted by qualified CAs; the format must hold multiple signatures with different algorithms.
- The seal must cover provenance fields that today sit outside the seal (#407, #824).
- Remote signing services rate-limit and cost per signature: batch-sealing (sign a Merkle root over many products) may be needed.

## Acceptance
- [ ] backend + verify with a test CA and test TSA; [ ] embedded revocation data; [ ] multi-signature file (ed25519 + x509) verifies; [ ] docs page.

Refs #209, #526, #407, #577. Consumer: nerdmachines/vetgraph (veterinary imaging archive).
