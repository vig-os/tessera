---
type: issue
state: closed
created: 2026-10-29T14:27:11Z
updated: 2026-10-29T23:36:22Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/704
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-30T08:02:19.980Z
---

# [Issue 704]: [fix(deps): bump libz-sys to 1.1.31 (RUSTSEC-2026-0418 blocks every push via cargo-deny)](https://github.com/vig-os/tessera/issues/704)

RUSTSEC-2026-0418 (published 2026-10-28) flags `libz-sys <= 1.1.30`. `tessera/Cargo.lock` pins **1.1.29** (via `zarrs` → `tessera-io`). The `cargo-deny (licenses + advisories)` prek hook fails as a result, so **no branch can be pushed** right now, and CI's advisory check will go red the same way.

## Exposure
The build-script environment leak only happens with `cc >= 1.6.0`. Our lockfile has `cc 1.2.65`, so tessera's builds have not leaked anything, and no cache purge or secret rotation is needed. The gate still fires because it matches on the libz-sys version.

## Fix
Run `cargo update -p libz-sys --precise 1.1.31` (released 2026-10-28; it stops logging the probe command). This is a lockfile-only patch bump. Check `checks.*.feature-snapshots` afterwards: a version move can show up in the snapshots, so regenerate with `scripts/feature-snapshots.sh` if it does. Do **not** add an ignore to `deny.toml`, because the patched version exists.

Found while pushing #702's branch (`test/702-versitygw-s3-check`, head 9e6bca9), which is blocked by this.
