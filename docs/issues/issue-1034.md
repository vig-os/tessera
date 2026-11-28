---
type: issue
state: open
created: 2026-11-27T12:26:13Z
updated: 2026-11-27T12:26:13Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/1034
comments: 0
labels: chore, area:ci
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-28T09:46:59.984Z
---

# [Issue 1034]: [nits from the #1031 review (thread budget)](https://github.com/vig-os/tessera/issues/1034)

From the round-2 review of PR #1031 (#703). It found no blockers.
1. **The budget guard matches on text.** A new Nix check is caught only if its script mentions `/bin/tessera`, `_native.so`, `tessera_wasm` or `tessera-wheel-`. It misses a check that calls `/bin/tsra`, or one that runs tessera through a `writeShellScript` wrapper. Find a sturdier criterion: for example, any check whose build inputs include a tessera derivation must carry the budget, or the budget becomes the default for every check with an explicit opt-out list.
2. The array-budget test asserts `concurrent_target()==1`, but not that `codec_options()` is actually passed to zarrs. Pin that too.
3. If creating the private rayon pool fails once, the error is cached and every later call fails. Decide whether that is intended (fail fast) or retryable, and document it.
4. Five lines are over 120 characters: `CHANGELOG.md` (151), `docs/book/src/performance.md` (172) and `flake.nix` (154, 142, 122).
5. Make `rayon = "1"` in `tessera-io/Cargo.toml` a workspace dependency.

Refs: #703
