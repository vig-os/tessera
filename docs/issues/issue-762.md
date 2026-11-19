---
type: issue
state: open
created: 2026-11-04T01:25:42Z
updated: 2026-11-18T14:57:45Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/762
comments: 2
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-19T09:52:25.096Z
---

# [Issue 762]: [feat(io): hdf5-free Rust API for the ADR-0047 identity envelope writer](https://github.com/vig-os/tessera/issues/762)

## Context
[private-repo] packs raw PET scanner files into a stage-0 `.tsra` ([private-repo]#138, ADR-0001 amendment 2026-11-04, [private-repo]#162). The pack must carry an ADR-0047 identity envelope (`aux/identity/`) with the non-allowlisted header fields of every packed file.

## Gap (at `4504efb`)
The ADR-0047 envelope writer `encrypt_identity` lives in `tessera-ingest/src/identity.rs:70`. `tessera-ingest/Cargo.toml:19,22` depends on `hdf5-metno` and its `-sys` crate. A Rust producer that depends only on `tessera-io` and `tessera-core` cannot write an envelope without pulling in libhdf5 and the whole ingest crate.

Relates #566, which covers the Python/CLI `attach-identity` surface. This issue is the Rust library surface.

## Ask
Move the envelope primitives (the identity document type, `encrypt_identity`, and the recipient validation already at `:74`) into an hdf5-free crate, such as `tessera-io` or `tessera-core` or a small `tessera-identity` crate. `tessera-ingest` would then re-export them.

---

# [Comment #1]() by [gerchowl]()

_Posted on November 8, 2026 at 08:11 PM_

Consumer status: on [private-repo], the stage-0 raw pack is now complete except for the identity envelope. That covers the typed allowlisted header tree in the manifest and a CI leak oracle, on visionx-rs main at 04b9084.

The remaining piece, the crypto-shredded ADR-0047 envelope inside the raw pack, is tracked in [private-repo]#186. It is blocked on this issue. We aren't working around it; until this lands, the extension point stays documented and unimplemented.

---

# [Comment #2]() by [gerchowl]()

_Posted on November 13, 2026 at 11:55 AM_

From the [private-repo] architect, 2026-11-13: this issue and #763 block visionx-rs stage-0 raw pack P4 ([private-repo]#186, the identity envelope inside the raw pack). Is there an ETA, and will it ship in alpha.6? We won't work around it; we'll wait for the release.

