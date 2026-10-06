---
type: issue
state: closed
created: 2026-09-23T16:21:36Z
updated: 2026-09-23T18:28:58Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/430
comments: 2
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-06T08:33:23.454Z
---

# [Issue 430]: [ci(release): release-plz has failed since 2026-07-03 — `.envrc` is both tracked and gitignored, so it aborts on "working directory has uncommitted changes"](https://github.com/vig-os/tessera/issues/430)

## Problem

The `release-plz` workflow has failed on every run since 2026-07-03 (both runs that day red; no successful run since). Surfaced while fixing #427 → #429. The failing step reports:

```
working directory has uncommitted changes … [".envrc"]
```

`.envrc` is committed to the repo **and** matched by `.gitignore`. release-plz (via `cargo` / `git` cleanliness checks) sees that as a dirty tree and refuses to plan a release. This is a hard blocker for #337 (cut `0.1.0-alpha.1`) — the release cut cannot run until it's resolved.

## Decision needed (owner call — it changes repo semantics)

Two clean fixes; pick one:

1. **Stop tracking `.envrc`** (`git rm --cached .envrc`, keep the ignore rule) — `.envrc` becomes per-developer (direnv is a local convenience; the devShell is the SSoT anyway). Contributors copy it from docs.
2. **Un-ignore `.envrc`** (remove the `.gitignore` rule) — keep it tracked as the canonical `use flake` one-liner.

Either way, re-run `release-plz` (workflow_dispatch) to confirm green before attempting #337.

## Not in scope

This is unrelated to #427/#429 (checkout v7 vs cargo-dist's regenerated `release.yml`), which is fixed separately.

Refs: #337
---

# [Comment #1]() by [gerchowl]()

_Posted on September 23, 2026 at 05:05 PM_

**Correction after a live baseline run** (`workflow_dispatch` on current dev, run 35892910085, before #431 merged):

- The `.envrc` / "working directory has uncommitted changes" abort **does not reproduce today** — the full log has no mention of it. That was the 2026-07-03 failure mode; it is not the current blocker.
- release-plz gets all the way to **`next version is 0.1.0-alpha.1` for all six crates**, so the "no tagged baseline" limit noted in the workflow isn't blocking either.
- It then fails at **`ERROR Failed to open PR`** — i.e. the workflow's `GITHUB_TOKEN` cannot create the release PR. The workflow declares `pull-requests: write`, so this is the repo/org Actions setting *"Allow GitHub Actions to create and approve pull requests"* being off, and/or the well-known limitation that `GITHUB_TOKEN`-created PRs don't trigger CI anyway.

**So #431 (untrack `.envrc`) is still worth landing** — a tracked+ignored file is a latent hazard and the `.envrc.example` docs fix is real — but it will **not** make release-plz green, so it's `Refs`, not `Fixes`. The durable fix is a **GitHub App token** for release-plz (can open PRs regardless of that setting, never expires, and its PRs trigger CI) — or, as a quick unblock, flipping the Actions setting. Owner call; leaving this open.

---

# [Comment #2]() by [gerchowl]()

_Posted on September 23, 2026 at 06:28 PM_

**Proven end-to-end (post-close audit trail).** Corrected root cause: `GITHUB_TOKEN` could not create the release PR (`can_approve_pull_request_reviews=false`) → `Failed to open PR` after correctly computing `0.1.0-alpha.1`. Fix: GitHub App **`tessera-release-plz`** (app id 5050387; secrets `RP_APP_ID`/`RP_APP_PRIVATE_KEY`) wired by #432. Proof: `workflow_dispatch` run [35902435972](https://github.com/vig-os/tessera/actions/runs/35902435972) → success → **opened #435 `chore: release v0.1.0-alpha.1`** as the App, and #435's CI triggers (`plan` ✓, nix legs running). Also landed: #431 (untrack `.envrc`), #429 (checkout v7 + cargo-dist override); #434 (SHA-pin generated `release.yml`) pending. **#435 is the alpha cut — merging it is #337's decision.**

