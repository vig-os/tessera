---
type: issue
state: open
created: 2026-11-20T21:26:09Z
updated: 2026-11-20T21:26:09Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/941
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-21T09:10:53.187Z
---

# [Issue 941]: [nits from the #940 review: duplicate-name refusal is borrowed from zip; DEFLATED refusal fixtures depend on the zlib build](https://github.com/vig-os/tessera/issues/941)

Follow-ups from the round-2 review of PR #940 (ADR-0066 A′ cutover):
1. `crates/tessera-io/tests/canonical_members.rs:24-31`: Rust's refusal of duplicate member names comes from the `zip` crate ("duplicate file names in central directory"), not from tessera. Add a member-name uniqueness check of tessera's own in `container.rs`, next to the STORED check, so a `zip` bump cannot silently drop the guarantee.
2. `corpus/reference_reader/make_refusal_fixtures.py:51-58`: the bytes of the DEFLATED refusal fixtures depend on the zlib build (zlib-ng, or a nixpkgs zlib bump), and the flake gate compares them byte-for-byte (`flake.nix:1425-1431`). Compare those fixtures by verdict, or pin the deflate stream, so the gate does not fail when no fault exists.

Refs: #914
