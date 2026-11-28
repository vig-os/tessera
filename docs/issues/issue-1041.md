---
type: issue
state: closed
created: 2026-11-27T18:19:22Z
updated: 2026-11-27T20:50:36Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/1041
comments: 1
labels: bug, area:testing, area:ingest
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-28T09:46:58.748Z
---

# [Issue 1041]: [test(ingest): ge_hdf5 block_prefix test hits 'H5Fopen: unable to lock file, errno 11' (flaky)](https://github.com/vig-os/tessera/issues/1041)

Seen on 2026-11-27 in the `ingest-gate-a-reduced` check (run 46041671562) on PR #1039, which changes docs only and cannot touch this code:

```
ge_hdf5::tests::block_prefix_renames_blocks_and_byte_identical_under_same_prefix panicked at crates/tessera-ingest/src/ge_hdf5.rs:1991
Invalid("ge-hdf5: H5Fopen(): unable to synchronously open file: unable to lock file, errno = 11 …")
```

Line 1991 is `read_events_2p(&big_h5, …)`, called right after `write_synth_2p(&big_h5, rows)` on a file in the test's own temp directory. HDF5 takes an advisory file lock on open, and errno 11 (EAGAIN) means the lock is still held when the reader opens the file. That points to the writer's file handle, or an object open inside it, outliving `write_synth_2p`; HDF5 keeps a file open until every object in it is closed. The other possibility is a second open in the same process.

**Root fix, not a workaround:** make the test helpers close every HDF5 handle before returning (scope the `File`/`Dataset` values, or close them explicitly), and add a test that writes, then reopens for reading, in a loop. Do NOT disable locking with `HDF5_USE_FILE_LOCKING=FALSE`: that only hides the leak, and readers in production would still hit it.

Refs: #208
---

# [Comment #1]() by [gerchowl]()

_Posted on November 27, 2026 at 07:13 PM_

This happened a second time, in a different test: `ge_hdf5::tests::streamed_2p_ingest_is_byte_identical_to_whole_file_read` (run 46048892587, PR #1039, docs-only). Two tests in two consecutive runs: this blocks unrelated PRs, so it is being fixed now.

