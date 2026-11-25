---
type: issue
state: open
created: 2026-11-24T20:42:02Z
updated: 2026-11-24T20:42:02Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/974
comments: 0
labels: chore, area:io, area:integrity
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-25T10:51:47.807Z
---

# [Issue 974]: [nits from the #973 review (fenced writer)](https://github.com/vig-os/tessera/issues/974)

From the fresh review of PR #973 (#946, head 573554a7). It found no blockers.
1. The public `set_ref` can point a ref at a version whose blocks are gone. A temporary test showed it: `write_object(B)`, then `gc` (reclaims B), then `put_manifest(M)`, then `set_ref(L, None, M)` returns Ok, and `read_object(B)` fails. Each call holds the lock; only the sequence is unprotected. No caller in the tree does this. Fix: `advance_ref` checks that the manifest's blocks exist.
2. A commit checks only that the blocks it does not supply exist, as the ADR documents. A `--set` commit over a torn shared block returned rc 0, and the later `seal` hit an integrity error. Decide whether commit verifies the hashes of inherited blocks, or document that `seal`/`verify` is where that gets caught.
3. **Durability, found by reasoning (no test yet):** when a store finds an intact object and skips writing it, it does not fsync the shard directory. Suppose a writer was killed after its rename but before its directory fsync. The next commit skips that object and fsyncs only `refs/`, so after a power loss on a filesystem that doesn't order metadata, the ref can outlive the object's directory entry. Fix: fsync the shard directory on the skip path too, with a test through the kill-point seam.
4. A corrupt ref is refused when it is read, not when the repo is opened (repo.rs:118-120). `tessera log` exits 0 on a corrupt ref because it reads the cache (relates to #971). Either align this with #946's "error on open" or document the behaviour.

Refs: #946
