---
type: issue
state: open
created: 2026-10-21T00:01:05Z
updated: 2026-10-25T09:11:15Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/656
comments: 1
labels: feature, security, priority:medium, effort:large, area:core, area:integrity, adr
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-26T07:24:25.404Z
---

# [Issue 656]: [feat(identity): identity envelope can carry opaque original header bytes (byte-exact source rebuild), not only a tag→value map](https://github.com/vig-os/tessera/issues/656)

Relates #566 (generalised identity envelope for non-DICOM producers), ADR-0047, ADR-0061.

## Problem
`IdentityDocument` (`identity.rs`) holds a JSON map of tag → {vr, value}. A producer that de-identifies a vendor container header (a DICOM dataset with embedded vendor free text) and must be able to **rebuild the original source file byte for byte** for the key holder cannot do so from a value map. Element order, encoding, padding and private or undecodable elements are lost.

## Ask
Allow the envelope to carry an opaque, encrypted **original-bytes** payload (plus its sha256) next to or instead of the curated map, with the same crypto-shred semantics. Shredding the key makes the bytes unrecoverable. Expose it from Rust for non-DICOM products (#566).

## Acceptance
Round trip: seal with envelope → key holder recovers the original bytes (sha256 equal) → shred → unrecoverable; the sealed manifest never contains them in clear.
---

# [Comment #1]() by [gerchowl]()

_Posted on October 25, 2026 at 09:11 AM_

**Triage (architect, 2026-10-25): accepted. It changes the sealed identity envelope, so an ADR comes first, together with #566. It's PHI-bearing, so a security pre-mortem runs before the ADR is accepted.**

The ADR (extending ADR-0047/ADR-0061, coordinated with #566) has to settle:
- **Envelope shape:** an encrypted original-bytes payload next to the curated map, or instead of it, and the schema-version bump.
- **Plaintext digest:** whether its sha256 can appear in clear at all. A plaintext hash of a low-entropy header is a confirmation oracle, so the default is that the digest lives *inside* the encrypted payload unless the ADR argues otherwise.
- **Crypto-shred:** the same key covers both the map and the bytes; shredding makes both unrecoverable.
- **Size limits and streaming** for large headers.
- **Rust API** for non-DICOM producers (#566).

The acceptance criteria stay as the issue states them, plus a test proving that neither the bytes nor any plaintext digest of them appear anywhere in the sealed archive.

Order: an ADR in the design wave after the #654 ADR.


