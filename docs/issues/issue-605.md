---
type: issue
state: open
created: 2026-10-18T11:53:28Z
updated: 2026-10-18T11:54:18Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/605
comments: 0
labels: feature, area:integrity
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-19T07:17:51.095Z
---

# [Issue 605]: [[ADR-0060 P3c] feat(cli): `tessera verify-change` binds products to an approved change](https://github.com/vig-os/tessera/issues/605)

Phase **P3c** of [ADR-0060](https://github.com/vig-os/tessera/blob/dev/docs/adr/0060-agentic-actors-and-prov-mapping.md) §11 (Accepted, merged in #586). Parent: #564.


- **Scope:** `tessera verify-change <product> --change <record> --role <role>` (§6.5), with checks
  (a)-(d); the book's change-record section with `subject_git_repo`/`subject_git_commit`, and the
  operator as an organisation whose identity each host key is pinned to with `trust add --signer`.
- **Acceptance:** a host-signed product whose `producer.git_commit` matches an approver-signed change
  record, with its `organization` operator attested by the host key → Ok, with no approver actor on the
  product → `change_approval_without_a_person_on_the_product`. Typed refusals, one test each, for: a
  mismatched commit; `dirty = true`; an unattested approver; an unattested operator (host key unpinned
  or pinned to another identity); a `software_agent` operator (`operator_must_be_attestable`); a
  record without subject keys; an `approved_change` edge pinning a different record.
- **Depends on:** P3.

## Dependencies

#587 → #573 → #574 → #596 → #597 → #598 → #599 → #600 → #601 → #602 → #603 → #604 → P3c

Do not dispatch before its predecessors merge.

