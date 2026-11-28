---
type: issue
state: open
created: 2026-11-27T20:50:40Z
updated: 2026-11-27T22:11:14Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/1043
comments: 0
labels: chore, area:testing, area:ingest
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-28T09:46:58.299Z
---

# [Issue 1043]: [test(ingest): guard against subprocess spawns in HDF5 test binaries (#1041 hazard)](https://github.com/vig-os/tessera/issues/1043)

From the review of PR #1042 (#1041). The vendored libhdf5 (`H5FD__sec2_open`) opens files without O_CLOEXEC and has no option to add it. So any child process spawned while another test in the same process holds an HDF5 file inherits the descriptor, and keeps HDF5's exclusive file lock after the parent closes the file (`H5Fopen: unable to lock file, errno = 11`). PR #1042 removed the one spawn: the ratchet skip test that #1031 added.
1. Add a guard so this can't return unnoticed. Either a test or check that fails if `std::process::Command` (or any spawn) appears in a crate whose test binary links HDF5 (tessera-ingest, and the tessera-cli `--bins` unit tests), or a module-level comment in `ge_hdf5` explaining the hazard, with the guard preferred.
2. Add a doc note for tessera-py users: forking or starting a subprocess while an HDF5-backed read is open keeps the file lock in the child. This is a general libhdf5 property.

Refs: #1041
