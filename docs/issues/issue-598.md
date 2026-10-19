---
type: issue
state: open
created: 2026-10-18T11:52:53Z
updated: 2026-10-18T11:54:03Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/598
comments: 0
labels: feature, security, area:ingest
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-19T07:17:53.845Z
---

# [Issue 598]: [[ADR-0060 P1e] feat(ingest,cli): actors under de-identification, and pseudonym minting](https://github.com/vig-os/tessera/issues/598)

Phase **P1e** of [ADR-0060](https://github.com/vig-os/tessera/blob/dev/docs/adr/0060-agentic-actors-and-prov-mapping.md) §11 (Accepted, merged in #586). Parent: #564.


- **Scope:** `tessera actor pseudonym` (§7.2); §7.4's refusal as one function in `tessera-ingest`,
  called by `ingest`, `commit --add-actor/--keep-actors` and the CLI's `tessera validate`.
- **Acceptance:** `dicom --deidentify` and `--recipient` with a ROR organisation, an ORCID person, a
  `name` or `--actors-aux` are refused, and with pseudonyms only they seal →
  `deidentified_ingest_accepts_pseudonyms_only`. `commit --add-actor` of an ORCID person onto that
  product is refused → `deidentified_commit_refuses_named_actor`. `tessera validate` flags a
  library-written de-identified product with a named actor. `actor pseudonym` output matches §7.2's
  regex, and two runs differ.
- **Depends on:** P1b (and P1d for the `--actors-aux` refusal; if P1d hasn't landed, that one assertion
  moves to P1d).

## Dependencies

#587 → #573 → #574 → #596 → #597 → P1e → P1c → P1d → P2a → P2b → P3 → P3d → P3c

Do not dispatch before its predecessors merge.

