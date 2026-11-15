# Frozen snapshot — the live issue archive is on `sync/issue-mirror`

The issue files in this directory are a **frozen snapshot**. They stopped being updated when
`sync-issues.yml` was retargeted to the `sync/issue-mirror` branch, and they will drift further
from reality with every new issue.

For current state, read the issues on GitHub, or the archive on the
[`sync/issue-mirror`](https://github.com/vig-os/tessera/tree/sync/issue-mirror/docs/issues) branch,
which the daily sync keeps up to date. That sync is **incremental**: each run writes only the
issues that changed since the last run's watermark (widening to a bounded 14-day look-back if the
watermark cache was evicted). A full rebuild happens only on a `force-update` dispatch, so the
mirror is a maintained archive rather than a from-scratch regeneration.

## Why

The sync job pushes directly to its target branch via the API. `dev` carries branch protection
requiring the `nix flake check` status, and a direct push has no checks attached, so GitHub refused
every push:

```text
##[error]Required status check "nix flake check" is expected.
```

The devkit-sanctioned fix for a protected target is `DEVKIT_SYNC_TARGET` mirror mode
(vig-os/devkit#1227), which syncs to an unprotected branch instead.

Normally the mirror is folded back at release time, so the trunk copy catches up. That fold is
rendered into `release-core.yml`, which this repo does not have — `release` is in
`DEVKIT_FEATURES_DISABLED` because cargo-dist owns `release.yml` (#441). So there is currently no
path back, and these files stay frozen rather than merely lagging.

Tracked in #362, with the two upstream gaps at vig-os/devkit#1757 and vig-os/devkit#1758. When those
are resolved this directory can become live again.
