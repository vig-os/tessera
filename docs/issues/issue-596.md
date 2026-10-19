---
type: issue
state: open
created: 2026-10-18T11:52:42Z
updated: 2026-10-18T11:53:59Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/596
comments: 0
labels: feature, area:core, area:integrity
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-19T07:17:54.586Z
---

# [Issue 596]: [[ADR-0060 P1a] feat(core): sealed `actors` with validation and the carry rule](https://github.com/vig-os/tessera/issues/596)

Phase **P1a** of [ADR-0060](https://github.com/vig-os/tessera/blob/dev/docs/adr/0060-agentic-actors-and-prov-mapping.md) §11 (Accepted, merged in #586). Parent: #564.


- **Scope:** the `Actor` and `ActorKind` types (§5.7); `validate_actors` with every §10.1 refusal,
  including §7.2's forms and pseudonym shape, §7.3's email pattern and the `id_inputs` ban (§3);
  `ProductBuilder::add_actor` and the constructors (§10.1); the seal check; the carry rule in
  `from_manifest`/`publish`/`commit --keep-actors` semantics (library level); `Error::InvalidActor`;
  the role constants; the SPEC `actors` row; the `actors_chain` corpus fixture; the book section
  (§6.3-§6.5, §7.5's "what is safe today").
- **Acceptance:** the JSON byte form of §5.1 equals a committed golden → `actor_json_shape_golden`.
  One `invalid_actor_*` test per §10.1 rule, including `invalid_actor_email_in_sampling` (a nested
  `sampling` string), `invalid_actor_at_in_name`, `invalid_actor_pseudonym_shape` and
  `actors_in_id_inputs_is_refused`. Every semver form in §7.3's table is accepted in
  `harness_version`, `harness` and a nested `sampling` string (`pkg@1.2.3`, `tool@latest`,
  `pkg@1.0.0-alpha`, `pkg@1.0.0-alpha.beta`, `pkg@1.0.0-beta.rc`, `pkg@v1.0.0-beta.rc`,
  `pkg@1.0.0-rc.1+build.5`, `pkg@1.0.0+exp.sha.5114f85`, `npm:@scope/pkg@2.1.0-next.3`,
  `pkg@^1.2.0-beta.x`, `pkg@~1.2.3-rc.a`) → `version_pins_are_not_emails`. The three refused emails in
  the table are refused, and every documented false negative passes (the digit-led domains, a quoted
  local part, an IDN domain, an address literal) → `email_pattern_matches_its_table`.
  `tool@next.beta` is refused → `domain_shaped_tag_is_refused`. An empty list leaves all 8 goldens
  unchanged. Different actors, same `id`, different `manifest_hash` → `actors_are_version_not_lineage`.
  `from_manifest` carries none, keep-actors drops `reviewer`/`approver`, and publish copies verbatim →
  `carry_rule_never_inherits_an_approval`. A kept delegation to a dropped approver is refused →
  `kept_delegation_needs_reapproval`. An unknown kind using only known field names round-trips, is
  listed, and is refused on re-seal → `unknown_actor_kind_is_opaque`.
- **Depends on:** P0b, #574; a release containing P0 (Decision 1).
- **Split if needed:** (1) types, `validate_actors`, seal check and golden; (2) the carry rule, the
  corpus fixture and the book.

## Dependencies

#587 → #573 → #574 → P1a → P1b → P1e → P1c → P1d → P2a → P2b → P3 → P3d → P3c

Do not dispatch before its predecessors merge.

