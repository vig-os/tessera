---
type: issue
state: closed
created: 2026-08-04T10:33:21Z
updated: 2026-09-28T23:30:52Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/362
comments: 3
labels: none
assignees: none
milestone: 0.1.0-alpha.2
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:45.088Z
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
---

# [Comment #1]() by [gerchowl]()

_Posted on August 4, 2026 at 10:42 AM_

## Correction — the retry went green but did **not** clear the backlog

A re-run after the secondary limit decayed reported `success`, but that is misleading:

```console
$ git show --numstat --format="" origin/dev
62  0  docs/issues/issue-362.md      # ← one file

$ git ls-tree -r --name-only origin/dev docs/issues docs/pull-requests | wc -l
1

$ git log --oneline origin/dev -- docs/issues docs/pull-requests
2a97345 chore: sync issues and PRs   # ← the only commit that has EVER touched these paths
```

**The sync has never actually populated `docs/`.** What happened:

1. Run 1 (full sync, no cache) built the 361-file set → commit failed on the secondary rate limit → `Save sync state` never ran.
2. Run 2 restored a watermark and did an **incremental** sync, so it only saw `issue-362` (created between the two runs) → committed 1 file → `Save sync state: success`.

The watermark has now advanced **past 361 documents that were never written**. So the bot is green, the daily runs will stay green, and `docs/` stays empty — a silent-green failure, which is worse than the honest red it replaced.

## What this changes

Option 3 in the original description ("save state on partial success") is not the fix — the real hazard is the inverse: **state is saved when the payload never landed**. `Save sync state` should be conditional on the commit step actually having committed what the sync produced.

Dispatched a `force-update: true` run (the workflow input exists for exactly this) to regenerate the full set and attempt the backlog commit again. If it trips the rate limit once more, the backlog needs seeding in chunks — a single burst of ~361 files is simply over the limit.

The **credential fix is unaffected and still verified**: `Generate a token: success` in both runs.

---

# [Comment #2]() by [gerchowl]()

_Posted on August 4, 2026 at 10:54 AM_

## Conclusion: the issue half of the sync is broken in `vig-os/sync-issues-action`

Three runs, two of them with `force-update: true` (which is documented as *"Force update all issues and PRs (ignores last sync timestamp)"*), **all reporting `success`**:

| run | mode | files committed | `docs/issues` after | `docs/pull-requests` after |
|---|---|--:|--:|--:|
| 30900653998 | incremental | 361 attempted → **rate-limited** | 0 | 0 |
| 30901622334 | incremental | 1 | 1 | 0 |
| 30901766041 | **force-update** | 180 | 1 | 180 |
| 30902292161 | **force-update** | 0 (no changes) | 1 | 180 |

Expected: **182 issues**, **180 PRs** (`gh issue list --state all` / `gh pr list --state all`).

So: **PRs are complete and correct. Issues are not synced at all** — the single file present (`issue-362.md`) landed only because it was created between two runs and caught by an incremental pass.

The last force-update generated **zero** files, meaning the action believes the issue mirror is already up to date while 181 issue documents are absent from the repo. That is a bug in the action's issue path, not in this repo's workflow, credentials, or rate limits:

- credentials are fine — `Generate a token: success` in every run since the App was re-minted;
- the rate limit was a one-off caused by the 361-file backlog and no longer applies;
- the workflow wiring is unchanged and demonstrably works for the PR half.

### Where the fix belongs

`vig-os/sync-issues-action` — the issue collection is either not being fetched, not being written, or being diffed against the wrong baseline. Worth checking whether it filters `pull_request != null` on the `/issues` endpoint and accidentally discards everything, since GitHub's issues API returns PRs as issues.

### Secondary hazard, still unaddressed

`Save sync state` runs unconditionally after the commit step, so **the watermark advances even when the payload did not land**. That is what turned the original honest 401 into a green-but-empty mirror, and it will mask this bug again after any future partial failure. Gate that step on the commit actually having committed what the sync produced.

---

# [Comment #3]() by [gerchowl]()

_Posted on September 28, 2026 at 09:46 PM_

## Status update: the cause in this issue is fixed; a different one now blocks the same job

**The secondary rate limit is gone.** The backlog drained, and the devkit scaffold adoption (#442)
shipped both mitigations from the Options list above:

- option 3 — `Save sync state` now runs `if: always()`;
- and better than option 1 — `Compute incremental cutoff` takes a bounded **14-day look-back** on a
  cache miss instead of re-syncing from epoch, which is what made the file set unbounded.

The most recent failing run tried to commit **2 files, not 361**.

## What blocks it now

Branch protection, not rate limiting:

```
Committing 2 file(s) to branch dev
##[error]Required status check "nix flake check" is expected.
```

`dev` carries classic protection requiring the `nix flake check` context, and the job's direct API
push has no checks attached, so GitHub refuses it.

**Every "successful" run since then skipped the push**, which is why this hid for weeks — the commit
step is `if: steps.sync.outputs.modified-files != ''`, so a quiet day skips it and the job goes green:

| run | `Sync Issues and PRs` | `Commit and push changes via API` |
|---|---|---|
| 36303464714 | success | **skipped** |
| 36225555271 | success | **skipped** |
| 35831020452 | success | **skipped** |

So sync has never actually pushed since protection was added.

## Fix in flight

PR #471 sets `DEVKIT_SYNC_TARGET=sync/issue-mirror`, devkit's sanctioned route for a protected
target (vig-os/devkit#1227).

**Explicit cost, since it is not free.** Mirror mode normally folds back into the trunk at release
time (devkit#1424), but that fold is rendered into `release-core.yml`, which this repo does not have:
`release` is in `DEVKIT_FEATURES_DISABLED` because cargo-dist owns `release.yml` (#441). So the **181
files** currently under `docs/issues/` and `docs/pull-requests/` freeze as of that commit with no
path back. PR #471 adds `README.md` notices to both directories pointing at the mirror, so a reader
of a stale file is not misled. This is an interim, not a resolution.

## Why this issue stays open

Two upstream bugs found while diagnosing, both of which have to land before the trunk archive can be
live again:

- **vig-os/devkit#1757** — `Save sync state` under `if: always()` advances the watermark even when
  the push **failed**, so the unpushed delta is silently dropped and never retried. In run
  36395811823 the two PR files were lost for good: their `updated_at` is now behind the cutoff, so
  only a `force-update` dispatch recovers them. Data-loss class, and it affects every consumer.
  Worth noting this was presumably added to fix *this* issue's non-self-healing loop — but it trades
  a visible stall for silent loss, and the 14-day look-back already covers the original case.
- **vig-os/devkit#1758** — mirror mode combined with a disabled `release` group renders the retarget
  and the bootstrap step but silently omits the fold (it is `-f`-guarded on `release-core.yml`). The
  scaffold prints no warning, so the permanent divergence is undiscoverable without reading
  `init-workspace.sh`.

## The alternative, recorded

Converting `dev`'s classic protection to a **ruleset with the commit App as a bypass actor** keeps
the archive on `dev` and needs no divergence at all. It is a repo-settings change (classic protection
has no app-bypass for required status checks; rulesets do), so it was not taken here — but it is the
cleaner permanent fix if the frozen trunk copy proves annoying before devkit#1758 lands.

