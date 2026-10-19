---
type: issue
state: open
created: 2026-10-18T11:53:18Z
updated: 2026-10-18T11:54:13Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/603
comments: 0
labels: feature, security, area:core, area:integrity
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-19T07:17:51.884Z
---

# [Issue 603]: [[ADR-0060 P3] feat(core,cli): actor-role claim check plus pinned signer binding (one PR; closes #590)](https://github.com/vig-os/tessera/issues/603)

Phase **P3** of [ADR-0060](https://github.com/vig-os/tessera/blob/dev/docs/adr/0060-agentic-actors-and-prov-mapping.md) §11 (Accepted, merged in #586). Parent: #564.


- **Scope:** P3a, `ProductSchema.required_actor_roles`, worded everywhere as a claim check (§8); and
  P3b, §6.4's pinned-identity rule with `verify-sig --require-signer-role <role>` and #590's fix to
  `--require-signer`, sharing one lookup (Decision 6). They ship together: P3a never merges alone.
- **Acceptance:** a schema requiring `approver` refuses a manifest without one and accepts one with it.
  A `commit --keep-actors` of an approved version fails the claim check until `--add-actor`
  re-approves → `approval_does_not_survive_commit`. Built-in embedded schema bytes are unchanged (8
  goldens). A key pinned to A that signs with label B fails `--require-signer-role approver` when the
  approver is B, and also when it is A (label/pin mismatch) → `signer_role_uses_the_pin_not_the_label`.
  An unpinned key, or `--pubkey`, refuses → `signer_role_needs_a_pinned_key`. #590's tests pass. The
  persona trycmd is extended with the approver chain.
- **Depends on:** P1b.

## Dependencies

#587 → #573 → #574 → #596 → #597 → #598 → #599 → #600 → #601 → #602 → P3 → P3d → P3c

Do not dispatch before its predecessors merge.

