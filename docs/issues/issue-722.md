---
type: issue
state: closed
created: 2026-11-02T03:29:14Z
updated: 2026-11-02T06:09:48Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/722
comments: 0
labels: bug
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-02T08:51:35.380Z
---

# [Issue 722]: [ci(nix): summary passes on an empty check set; chain the registry EXIT trap; document cache-trust assumption](https://github.com/vig-os/tessera/issues/722)

These are the nits from the fresh review of PR #700 (#688, head 8bcb099). The review found no blockers.

1. **Summary passes over an empty set** (`.github/workflows/nix-check.yml` ~:348-379). The summary trusts the set of
   checks from `nix eval`. If eval returns `{}` with rc 0, it prints "0 passed, 0 failed" and the leg goes green. This
   predates #700. Assert the expected checks BY NAME against a literal count (the same rule as the "guard the referent"
   lesson), so that a missing or empty set fails the leg. Add a test that fails before the fix: a stubbed eval returning
   `{}` must turn the leg red.
2. **`trap … EXIT` replaces the builder's trap** (`flake.nix` ~:478, the registry check). Save the existing EXIT trap and
   chain to it (stdenv may set one), or use stdenv's `exitHooks` the way `s3-range-read` does.
3. **Trust in the cached row.** Counting a substituted check as passed is sound only while fork code cannot write to
   the shared ksb binary cache. State that assumption next to the lookup loop, and check that it holds (link the
   cache's write policy).

