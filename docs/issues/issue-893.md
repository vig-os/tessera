---
type: issue
state: open
created: 2026-11-16T15:39:14Z
updated: 2026-11-16T15:39:14Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/893
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-17T10:09:17.257Z
---

# [Issue 893]: [test/dev: #883 nits — split the path-collision core test; devshell python lacks blake3 for the reference reader](https://github.com/vig-os/tessera/issues/893)

Follow-up nits from the final #883 (#873) adversarial review.

1. `a_key_spelling_a_declared_path_cannot_exempt_it` (tessera-core forward_compat) asserts the rendered path first, so under a revert it fails on rendering and never reaches the bypass assertion; only the CLI test `a_crafted_key_cannot_exempt_a_member_field` guards the bypass. Split it so the core bypass assertion has its own guard.
2. The devshell's `python3` has no `blake3`, so `python3 -B corpus/reference_reader/run.py` fails with ModuleNotFoundError outside CI; agents ran it from a scratch venv. Add blake3 (and `jcs` if missing) to the devshell's Python so the documented command works.

Refs: #873
