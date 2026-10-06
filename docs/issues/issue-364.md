---
type: issue
state: closed
created: 2026-08-07T13:06:59Z
updated: 2026-09-24T11:48:58Z
author: c-vigo
author_url: https://github.com/c-vigo
url: https://github.com/vig-os/tessera/issues/364
comments: 0
labels: chore, priority:medium, area:ci, effort:large
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-06T08:33:34.156Z
---

# [Issue 364]: [chore(ci): adopt devkit scaffold; migrate sync to standard org Apps and retire dedicated App](https://github.com/vig-os/tessera/issues/364)

> **Re-scoped 2026-08-07.** This issue previously tracked migrating the
> dedicated `tessera-sync-issues-bot` App to client-ID credentials. That scope is
> **superseded**: rather than modernise the dedicated App's credentials, `tessera`
> adopts the **devkit scaffold** and moves sync onto the **standard org Apps**,
> and the dedicated App is **decommissioned**. Migrating credentials for an App we
> are about to delete is wasted work, and it would have been a double migration
> (numeric → client-ID on the dedicated App, then dedicated → org App later).

## Why the dedicated App goes away

`tessera-sync-issues-bot` (`app_id` 4483218, installation `151164735`) was created
on **2026-08-04** as a debugging artifact while sync was failing. The run history
shows it did not solve the problem it was created for:

- **2026-07-23 → 2026-08-04 05:12** — every scheduled run failed at the
  *Generate a token* step with `A JSON web token could not be decoded`. This was a
  **credential** failure, not a throughput failure. The dedicated App did fix
  *this* symptom.
- **2026-08-04 10:26** (run `30900653998`, first run to get past token generation)
  — failed at *Commit and push changes via API* with
  `You have exceeded a secondary rate limit`.
- **2026-08-05 05:11** (run `30977383250`) — same failure again, on **187 files**.
- **2026-08-06 / 2026-08-07** — green only because `modified-files` was empty and
  the commit step was skipped entirely. Sync is not working; it is idle.

So the dedicated App traded one failure mode for another, and the second failure
mode has nothing to do with App identity. **Quota isolation was never the
problem** — a secondary rate limit is per-installation, but the trigger here is
the *request pattern* of the pinned `commit-action` version, which any App
identity would hit identically. There is no remaining reason for `tessera` to
carry its own App.

## Rate-limit context: the current action versions already fix this

Investigated 2026-08-07. **Verdict: solved by the versions the devkit scaffold
already pins**, for tessera's payload shape. `commit-action` **v0.1.3** (the pin
here) creates one `POST /git/blobs` per file, sequentially, with no retry —
187 files becomes ~190 content-creating requests. Run `30977383250` issued them in
**52 s** (≈215/min) and tripped GitHub's documented ceiling of 80 content-creating
requests per minute; run `30901766041` pushed 180 files in **86 s** (≈127/min) and
survived only because secondary limits are heuristic. `tessera` therefore sits
permanently over the cliff for any run touching more than ~80 files.
`commit-action` **v0.2.0** (issue
[vig-os/commit-action#19](https://github.com/vig-os/commit-action/issues/19))
changed this: text files under 1 MiB are inlined as `content` inside the
`createTree` payload, so **zero blob POSTs are issued for markdown**; trees are
chunked at 100 entries / 6 MiB. The same 187 markdown files become
**2 `createTree` + 1 `createCommit` + 1 `updateRef` = 4 content-creating
requests**, a ~48× reduction and two orders of magnitude below the ceiling.
v0.2.0 additionally added bounded retry with exponential backoff whose
`isTransientError` explicitly classifies `403 … secondary rate limit / abuse` as
retryable, and v0.3.0 moved that retry to each individual call site. Retry is
**opt-in** (`DEFAULT_MAX_ATTEMPTS = 1`), and the devkit scaffold sets
`MAX_ATTEMPTS: "3"`, so adopting the scaffold turns it on. Note that *incremental
sync is not the fix*: `updated-since` + `state-file` shipped in
`sync-issues-action` v0.1.0 and this repo **already uses it** — the 08-05 run was
incremental and still produced 187 files. The scaffold does add a further bound
(a 14-day look-back on cache miss instead of a from-epoch re-sync), but the
decisive change is on the commit side. **No new issue is warranted against
`commit-action` or `sync-issues-action`; adopting the scaffold is the fix.**

## Precondition check: does devkit support tessera's dev-based flow?

**Yes — and it is devkit's default.** Verified against the devkit tree on
2026-08-07.

- `DEVKIT_WORKFLOW` accepts exactly `gitflow | trunk`; empty resolves to
  **`gitflow`** (`assets/init-workspace.sh:207-214`, `:400-406`, `:667-676`;
  `install.sh:594-600`, `:677-706`). `gitflow` *is* the long-lived-`dev` model —
  `.vig-os:17-25` describes it as "long-lived `dev` + `main` with
  `sync-main-to-dev.yml`". There is no `dev` value; the trunk render is the
  opt-in deviation (`render_workflow_model()` returns early unless
  `model == trunk`, `assets/init-workspace.sh:1377`).
- The scaffolded `sync-issues.yml` already defaults to `dev`:
  `target-branch` input `default: 'dev'` (`:25-29`), `ref: … || 'dev'` (`:103`),
  `TARGET_BRANCH: refs/heads/… || 'dev'` (`:194`). The trunk variant is produced by
  rewriting those to `main`.
- `docs/RELEASE_CYCLE.md:38-46` documents `dev` as the integration branch with a
  require-PR protection rule.
- `org-config` uses `DEVKIT_WORKFLOW=trunk`; `tessera` simply leaves the key empty
  (or sets `gitflow`).

**This is not a blocking dependency.** No devkit work is required.

Two secondary preconditions, both already satisfied:

- `commit-action-bot` (`app_id` 2433383, installation `98585378`) is installed
  org-wide on **all** `vig-os` repos, so it already has access to `tessera`.
- Org secrets `COMMIT_APP_CLIENT_ID`, `COMMIT_APP_ID`, `COMMIT_APP_PRIVATE_KEY`
  all exist with `visibility: all`.
- `tessera`'s `dev` uses a **classic branch-protection rule**, not a ruleset, and
  it carries no push restrictions and `requires_pull_request: false`. There is no
  App-specific bypass list to reproduce — the org App can push to `dev` exactly as
  the dedicated App does today. (The `docs/MIGRATION.md:303-334` warning about
  needing `DEVKIT_SYNC_TARGET=sync/issue-mirror` applies to repos whose `dev`
  ruleset requires a PR; it does not apply here as configured.)

## Sequencing — after devkit ships the client-ID scaffold

**Do not start until [vig-os/devkit#1365](https://github.com/vig-os/devkit/issues/1365)
has shipped *and been released*.** #1365 switches the stamped `sync-issues.yml`
from `app-id: ${{ secrets.COMMIT_APP_ID }}` to
`client-id: ${{ secrets.COMMIT_APP_CLIENT_ID }}` and bumps the
`sync-issues-action` pin; it is itself blocked on
[vig-os/sync-issues-action#168](https://github.com/vig-os/sync-issues-action/issues/168)
landing a `client-id` input. Scaffolding `tessera` before that release would stamp
numeric `COMMIT_APP_ID` and force a second migration here later. Waiting means
`tessera` lands directly on client-ID stamps.

Latest devkit release at time of writing is **1.6.0** (2026-08-04); #1365 targets a
later release. Fleet-wide client-ID consolidation is tracked in
[vig-os/org-config#112](https://github.com/vig-os/org-config/issues/112).

## Scope note: this is a first-time adoption, not a bump

`tessera` has **no `.vig-os` manifest** and only four workflows
(`ci.yml`, `release.yml`, `scorecard.yml`, `sync-issues.yml`). It has never been
scaffolded. This is a greenfield devkit adoption, not a version bump — hence
`effort:large`, not `effort:small`.

Current pins in `.github/workflows/sync-issues.yml` (verified 2026-08-07):

| Line | Pin | Version |
|------|-----|---------|
| `:55` | `actions/create-github-app-token@29824e69f54612133e76f7eaac726eef6c875baf` | v2.2.1 |
| `:96` | `vig-os/sync-issues-action@b4cdf371bb708230ce410a8203e6463e9e6caf2d` | v0.1.1 |
| `:110` | `vig-os/commit-action@b70c2d87acd0f146c40e8d88a9bda40b76c084b5` | v0.1.3 |

Credentials at `:57-58` and `:98-99` are the repo secrets `APP_SYNC_ISSUES_ID` /
`APP_SYNC_ISSUES_PRIVATE_KEY`.

## Plan

1. **Wait** for devkit #1365 to ship in a devkit release.
2. **Adopt the devkit scaffold** in `tessera` with `DEVKIT_WORKFLOW` left empty
   (= `gitflow`), pinned to that release. The generated `sync-issues.yml` targets
   `dev`, authenticates with the org `COMMIT_APP_*` secrets, sets
   `MAX_ATTEMPTS: "3"`, and carries the bounded-look-back cutoff step.
3. **Run the workflow** (a `workflow_dispatch` with `force-update: true` is the
   honest test — it forces the largest possible file set). Verify: the run is
   green, files are actually committed, the commit identity is
   `commit-action-bot`, and the commit step does **not** hit a secondary rate
   limit.
4. **Only then decommission** — steps 5-7 below, and not before a green run.
5. **Delete the repo secrets** `APP_SYNC_ISSUES_ID` and
   `APP_SYNC_ISSUES_PRIVATE_KEY` from `vig-os/tessera`.
6. **Uninstall the App** `tessera-sync-issues-bot` (installation `151164735`,
   `app_id` 4483218) from the `vig-os` org, and delete the App itself.
7. **Revert the paired declarations** in `vig-os/org-config`
   `otterdog/vig-os/vig-os.jsonnet` (~lines 655-663 — the two
   `orgs.newRepoSecret(...)` entries and the explanatory comment above them,
   declared 2026-08-07 via `vig-os/org-config#102`). Per the house invariant, the
   config edit ships in the **same change** as the live deletions, so otterdog
   never observes drift in either direction.

**Migration principle, unchanged:** no secret is deleted while any pinned
workflow still references it, and the *silent* failure mode is the one to guard
against — `exo-pet/playground-carlos`'s sync job reported success while syncing
nothing for a week when `COMMIT_APP_ID` was unavailable. Step 3 must check the
committed content and the committer, not just the green check.

## Acceptance criteria

- [ ] devkit #1365 has shipped in a devkit release, and `tessera` is scaffolded
      from that release (or later).
- [ ] `.vig-os` exists with `DEVKIT_WORKFLOW` empty/`gitflow`, and the generated
      `sync-issues.yml` targets `dev`.
- [ ] `sync-issues.yml` authenticates via the org `COMMIT_APP_CLIENT_ID` /
      `COMMIT_APP_PRIVATE_KEY` secrets; no `APP_SYNC_ISSUES_*` reference remains
      anywhere in the repo.
- [ ] A `force-update` dispatch run is green, commits a large file set **without**
      a secondary-rate-limit error, and the committer is `commit-action-bot` —
      run URL recorded in this issue.
- [ ] Repo secrets `APP_SYNC_ISSUES_ID` and `APP_SYNC_ISSUES_PRIVATE_KEY` deleted.
- [ ] App `tessera-sync-issues-bot` uninstalled and deleted.
- [ ] The `vig-os.jsonnet` declarations removed in the same change as the live
      deletions; `otterdog plan` clean afterwards.
- [ ] The old warning about "re-scaffolding would silently switch the App
      identity" is retired — after this issue, switching to the org App **is** the
      intended state.

Refs: vig-os/devkit#1365, vig-os/sync-issues-action#168, vig-os/org-config#112,
vig-os/org-config#102. Supersedes the original credential-migration scope of this
issue. See also #362 for unrelated sync-issues throughput work.

