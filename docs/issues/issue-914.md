---
type: issue
state: open
created: 2026-11-19T05:25:24Z
updated: 2026-11-19T05:25:24Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/914
comments: 0
labels: feature, priority:high, semver:major, area:core, area:integrity
assignees: none
milestone: none
projects: none
parent: 897
children: none
synced: 2026-11-19T09:52:03.612Z
---

# [Issue 914]: [feat(format)!: A′ seal cutover: seal over stored bytes, manifest.seal member, alpha layout refused (ADR-0066 step 4)](https://github.com/vig-os/tessera/issues/914)

Part of the ADR-0066 implementation plan (docs/adr/0066-seal-encoding.md, merged in #900; epic #897). **Step 4 of 9: the A′ format cutover.** One PR, because the layout changes once. Closes #901.

- **Crates:** `tessera-core`, `tessera-io`, `tessera-cli`, `tessera-ingest`, `tessera-py`, `tessera-wasm`, the Python reference reader, `corpus/`, SPEC and the ADRs.
- **Depends on:** step 2 and step 3.
- **Scope:** as listed in ADR-0066 step 4:
  - SPEC §2/§4/§6/§7/§9, plus dated amendments to ADR-0020 D4, ADR-0022, ADR-0037, ADR-0054 and ADR-0060;
  - a `manifest.seal` member (entry 2) and the stricter member set, in both the container and the exploded form; collections get `collection.seal`;
  - every reader verifies through `from_stored`, and the in-band `manifest_hash` key is refused;
  - **alpha-layout products are refused with an error naming the format found and the format required. There is no migration verb (rule 10).**
  - repository objects hold the stored bytes; `WriteSession`, sign and verify read the seal member;
  - the reference reader computes seal = `blake3(file)`, checks canonical form with `jcs`, keeps I-JSON, and refuses lone surrogates explicitly;
  - `seal-cases.json` is rewritten as `(bytes, seal)` vectors.
- **Acceptance:**
  - all 13 re-packed products verify, with every `corpus.json` golden unchanged from the PR's base;
  - an alpha-layout product is refused, and the error names both formats;
  - Rust, the reference reader and `tessera-wasm` reach the same verdict on every seal case, including #901's `64.0`;
  - a test per writer door shows that `manifest.seal` equals `blake3(manifest.json)`.

