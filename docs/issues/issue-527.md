---
type: issue
state: open
created: 2026-09-29T15:28:09Z
updated: 2026-09-29T15:28:09Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/527
comments: 0
labels: none
assignees: none
milestone: 0.1.0-beta
projects: none
parent: none
children: none
synced: 2026-09-30T07:57:03.612Z
---

# [Issue 527]: [feat(export): tessera export bagit / export ocfl — preservation-repository projections](https://github.com/vig-os/tessera/issues/527)

Sibling projections to the RO-Crate/OCI-index/S3-prefix ones (ADR-0033): emit a valid BagIt bag (RFC 8493: bagit.txt, bag-info.txt, manifest-sha256/512.txt, tagmanifest) and an OCFL object (inventory.json with sha512 digestAlgorithm and fixity), for deposit at LC/APTrust/Chronopolis-style repositories. Purely additive. Needs SHA-2 digests (#526, or computed at export time).
