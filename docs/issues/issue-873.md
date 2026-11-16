---
type: issue
state: open
created: 2026-11-15T16:24:53Z
updated: 2026-11-15T21:30:35Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/873
comments: 1
labels: bug, area:core, area:integrity
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-16T08:59:37.539Z
---

# [Issue 873]: [fix(core): forward-compatible collections and embedded schemas (ADR-0060 P0b)](https://github.com/vig-os/tessera/issues/873)

Follow-up to #573 (ADR-0060 §2.2, §11 "P0b"). The #573 fix (P0a) verifies a product **manifest** over the parsed
JSON object and names the keys the struct doesn't model (`Manifest::newer_fields`). The same rule is still missing
for:

- **`Collection`** (`tessera-core/src/collection.rs`): `canonical_bytes`/`compute_manifest_hash` re-serialise the
  struct and `from_json_verified` verifies that, so a newer collection field fails verify as tampering and a
  spliced one passes — the #573 bug, on collections. Fix through the same shared functions
  (`canonical::parse_strict`, `canonical::hash_without`) and a declared-field walk.
- **`ProductSchema::from_value`** (`schema.rs`): drops unknown rules, so `validate_manifest` reports "valid" against
  a rule it never evaluated. Report unknown keys at any depth (top level, `fields[]`, `blocks[]`) and refuse with
  `Error::NewerFormatFields`; `inherit: false` / `requires_generation: false` must still validate.
- **SPEC §5**: add the missing `producer`, `generation`, `ingest_transform` rows.
- **`tessera inspect --help`** (`tessera-cli/src/main.rs`, "Fields are added compatibly (readers default them …)"):
  state the forward rule (newer fields verify, warn, and block a re-seal).

Acceptance (ADR-0060 §11 P0b): `unknown_schema_rule_is_not_validated`, `collection_forward_compat`; the reference
reader's `run.py` exits 0.

---

# [Comment #1]() by [gerchowl]()

_Posted on November 15, 2026 at 09:30 PM_

This is in scope for alpha.6. The PR #875 adversarial review (2026-11-15) showed that `Collection::from_json_verified` still accepts a spliced key: a probe printed `COLLECTION spliced forged:true -> Ok("OK")`. That is the same verify-bypass class as #573, which is now fixed for product manifests in e8517d9.

The owner's alpha.6 plan is one security release that closes the verify bypasses, so this goes into it. Reuse #875's shared parsed-object seal function rather than writing a second implementation.

