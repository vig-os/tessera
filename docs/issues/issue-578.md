---
type: issue
state: open
created: 2026-10-16T23:03:25Z
updated: 2026-10-16T23:03:25Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/578
comments: 0
labels: documentation
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-17T06:32:42.725Z
---

# [Issue 578]: [docs: make ADRs and capabilities discoverable (ADR index on dev + use-case guide: de-identify vs crypto-shred vs encrypt vs sign)](https://github.com/vig-os/tessera/issues/578)

Found while dogfooding from MorePET/anatomist (2026-10-17).

**What went wrong:** two agents designing anatomist's adoption (anatomist #91, #103 / ADR-0007) built a crypto-shredding scheme on **whole-file per-subject encryption (ADR-0055, Proposed)** plus `share --to`, and took several review rounds to find out that:
- tessera **already ships** crypto-shred de-identification: ADR-0047 rung 1, `ingest dicom --crypto-shred --recipient` → `aux/identity/identity.age`, inside the file and outside the seal, with `reidentify` and schema-driven identity fields;
- ADR-0055 itself says `share --to` is **not erasure**.

The only place this is easy to find is one row of `docs/FEATURE-MATRIX.md`.

**Discovery gaps:**
1. **ADRs referenced in code aren't on `dev`.** `git ls-tree origin/dev` has no ADR files: ADR-0047 is cited only in `Cargo.toml`, `crates/tessera-ingest/Cargo.toml`, `main.rs`, `trust.rs` and CHANGELOGs, and ADR-0055 likewise. Agents end up reading ADRs from issues or other branches without knowing their status.
2. **There's no use-case guide.** Nothing maps "I need to …" to the feature and its status:
   - remove personal identifiers but keep re-identification possible (ADR-0047 envelope);
   - erase a subject (destroy the identity key; **not** `share --to`);
   - share with a recipient (ADR-0055, proposed);
   - prove origin (ADR-0037 signing and `verify-sig --pubkey`, cf. #576);
   - provenance (ADR-0058 `producer` / `generation`).
3. **The identity envelope is DICOM-only and CLI-only:** NIfTI or tabular ingest and the Python Builder can't attach it (#566). The docs don't say so.

**Proposal:**
- Commit the ADRs (or a `docs/adr/README.md` index with each ADR's number, title, status and link) on `dev`.
- Add `docs/GUIDE-privacy-and-provenance.md`, a use-case → command → status table with the "not erasure" warnings.
- Link both from the README and FEATURE-MATRIX.

**Upstream lesson:** a consumer (or its agents) should check the FEATURE-MATRIX first. The anatomist briefs now say so.
