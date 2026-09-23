---
type: issue
state: open
created: 2026-08-04T10:33:21Z
updated: 2026-08-04T10:33:21Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/362
comments: 0
labels: none
assignees: none
milestone: none
projects: none
relationship: none
synced: 2026-08-04T10:40:48.991Z
---

# [Issue 362]: [ci(sync-issues): 361-file backlog trips GitHub's secondary rate limit](https://github.com/vig-os/tessera/issues/362)

## Status

The **credential half is fixed**. `sync-issues.yml` had failed every day since ~2026-07-10 with

```
Failed to create token for "tessera" (attempt 1):
  A JSON web token could not be decoded — 401
```

A new App (`tessera-sync-issues-bot`, app id `4483218`) was created and installed on `vig-os/tessera`, and `APP_SYNC_ISSUES_ID` / `APP_SYNC_ISSUES_PRIVATE_KEY` were re-minted under the names the workflow already reads — no workflow change needed. Verified in run [30900653998](https://github.com/vig-os/tessera/actions/runs/30900653998): **`Generate a token: success`**.

## The remaining problem

That run then failed one step later:

```
Commit and push changes via API
  Committing 361 file(s) to branch dev
  ##[error]You have exceeded a secondary rate limit.
```

Because the sync was down for ~a month, the first successful run has a **361-file backlog** (≈180 issues + ≈180 PRs) and `vig-os/commit-action` pushes them in a single burst, which exceeds GitHub's secondary rate limit.

## Why a plain re-run may not clear it

The steps run in order:

1. `Restore sync state` ✓
2. `Sync Issues and PRs` ✓
3. `Commit and push changes via API` ✗ ← fails here
4. `Save sync state` — **never runs**

So the last-synced watermark is never advanced, and the next run rebuilds the same 361-file set and hits the same limit. It is not self-healing.

## Options

1. **Seed the watermark once** — land the backlog in a few manual chunks (or commit the generated `docs/` files directly), then let the daily run handle the small delta it was designed for.
2. **Batch in `vig-os/commit-action`** — chunk the tree/blob calls with backoff. Fixes the class, but is a change in another repo.
3. **Make `Save sync state` run on partial success** so progress is not lost between attempts.

Steady-state daily runs are only a handful of files, so this is purely a backlog-scale problem — but it will not resolve on its own.

Refs: #356
