---
type: issue
state: open
created: 2026-11-15T13:00:00Z
updated: 2026-11-15T13:00:00Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/870
comments: 0
labels: area:ci
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-16T08:59:39.064Z
---

# [Issue 870]: [follow-up(#793): RAM-TMPDIR floor is unchecked, hook guard gaps, Docker /dev/shm escape](https://github.com/vig-os/tessera/issues/870)

Review nits on PR #866 (head ab40e6d, merged as 6e4ce13), deferred per the nits-to-follow-ups rule.

1. **The floor is untested.** `flake.nix:362` sets `ram_tmpdir_min_free_mib=256`, but no test checks that value. A mutant that sets it to 0 or 1 passes every check. Make `ram-tmpdir-guard` assert that the floor is at least the measured peak of 106.5 MiB (#793), and decide whether free space should be budgeted per check while max-jobs = 2 runs two at once.
2. **Gaps in the hook guard.** The check for a bare `exitHooks+=` / `failureHooks+=` does not scan `preConfigure`, `postConfigure` or `configurePhase`. It also misses `exitHooks[...]=` indexed assignment and a `trap … EXIT` that replaces stdenv's handler.
3. **Failed `--keep-failed` builds lose their files.** `remove_ram_tmpdir` runs in `failureHooks`, so the test temp files are deleted. Document this in TEST-PLAN, or keep the directory when `--keep-failed` is set.
4. **Docker has no escape hatch.** Inside a default Docker container, where `/dev/shm` is 64 MiB and `sandbox=false`, every protected check fails loudly. Document this, and decide whether there should be a disk-backed opt-out.
5. **Regex risk.** The eval-time regex patterns start with `.*` and run over each check's whole shell code. `std::regex` can overflow its stack on long inputs. Switch to `builtins.match` on narrower inputs, or to `lib.hasInfix`.

Refs: #793
