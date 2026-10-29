---
type: issue
state: closed
created: 2026-10-29T05:07:09Z
updated: 2026-10-29T07:27:00Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/695
comments: 1
labels: bug, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-29T08:04:00.199Z
---

# [Issue 695]: [fix(io): WriteSession append_block and create don't fsync the directory entries they create](https://github.com/vig-os/tessera/issues/695)

Found while auditing durable writes for #686 (`tessera/crates/tessera-io/src/write.rs`). These writes are not covered by `write_durable`, because they create a file or directory in place rather than replace a whole file, and they don't fsync the directory entry they create:

1. **`WriteSession::append_block`**: the fragment is created, written and `sync_all`'d, but its parent directory (`blocks/` or `blocks/<a>/…`) is never fsync'd. `append_staged` does this in `place()`, but `append_block` doesn't. After a power loss the journal line, which is fsync'd after the fragment, can be on disk while the fragment's directory entry is not. `recover` then stops at that line, and a block whose append returned `Ok` is lost. That breaks "the journal line is the commit point".
2. **`WriteSession::create`**: `journal.jsonl` is created with `truncate` and `sync_all`'d, but the session directory isn't fsync'd after that. The header's `write_durable` fsyncs the directory, but it runs *before* the journal is created. `fs::create_dir_all` of the stage dir, `blocks/` and `staging/` isn't fsync'd into the parent either. A crash right after `create` can leave a session with no journal, so `recover` fails with `NotFound`.

Fix: fsync the fragment's parent in `append_block` before the journal line (one `sync_dir`, as `place()` does). In `create`, make the directories with the durable directory helper and fsync the stage dir once the journal exists. Testing it needs a power-loss model, or a seam that records the order of fsyncs, because a test process can't lose the page cache.

---

# [Comment #1]() by [gerchowl]()

_Posted on October 29, 2026 at 06:06 AM_

Fold in from the PR #696 review (at 3bd059e, same code):

1. `write.rs:205-207` `create_dirs_durable`: it calls `create_dir`, then `sync_dir`, and only then `created.insert`, which contradicts the rustdoc ("recorded … as soon as it exists"). If the directory fsync fails, `fragment_guard` leaks the empty directory, and a later append of `c` hits EISDIR. Record the directory before the fsync, and add a test with a fault seam on the directory fsync.
2. `write.rs:368`: `recover` calls `quarantined_names(dir)?` after the journal rewrite, so a corrupt quarantine journal is reported only after the disk has already changed. Validate it before changing anything, which keeps the "changes nothing" wording true.
3. The `#[cfg(test)]` seam `FAIL_NTH_DURABLE_RENAME` is never reset if a test panics before it fires. Reset it with a drop guard.

