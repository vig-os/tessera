# Releasing Tessera

How a release is cut, in order. The automation is described where it lives: `release-plz.toml` and
`.github/workflows/release-plz.yml` (version bump and CHANGELOG, ADR-0052), `dist-workspace.toml` and the
generated `.github/workflows/release.yml` (cargo-dist binaries), and `.github/workflows/promote-main.yml`
(the `main` promotion, #854). This page is the checklist that connects them.

## Branches

- `dev` is the default branch. All work lands there.
- `main` is the latest release: after every release it equals the newest `v<version>` tag's tree. Anything
  that tracks or reads `main` sees the newest published release, never unreleased work.
- A release is the `v<version>` tag (for example `v0.1.0-alpha.5`), pushed on a commit of `dev`.

## Checklist

1. **Version the root CHANGELOG on the release PR.** release-plz keeps one PR (`chore: release v<version>`,
   branch `chore/release-plz-<timestamp>`) up to date with the next version and the crate changelogs
   (`tessera/crates/<crate>/CHANGELOG.md`). It does not touch the root `CHANGELOG.md`, which is the release
   notes (see [Release notes](#release-notes)). Its `nix flake check` fails at `changelog-sections` until
   the root file has a section for the new version. On the PR's branch, from the dev shell:

   ```bash
   # <version> without the leading v, <YYYY-MM-DD> today's date
   scripts/changelog.py release CHANGELOG.md <version> <YYYY-MM-DD>
   scripts/changelog.py check CHANGELOG.md tessera/Cargo.toml
   git commit -S -am "docs(changelog): release <version>" \
     -m "Refs: #<release issue>"
   git push origin HEAD:chore/release-plz-<timestamp>
   ```

   `release` turns `## Unreleased` into `## [<version>] - <date>` (dropping its empty subheads), opens a
   new, empty `## Unreleased` above it, and adds the release's compare link. Do this last, just before
   merging: when `dev` moves, release-plz closes a release PR that has a commit not made by a bot and
   opens a new one, without this commit. Then do it again on the new PR.
2. **Merge the release PR.** Review it and merge it into `dev`. Merging alone tags and publishes nothing.
3. **Tag the merge commit.** On that merge commit, create a signed, annotated tag and push it from the dev
   shell, so the repo's pre-push hooks run:

   ```bash
   git fetch origin
   git tag -s v<version> <merge commit> -m "Tessera v<version> — <one-line summary>"
   nix develop -c git push origin v<version>
   ```

   The tag is the single trigger for everything below.
4. **Watch the Release run.** cargo-dist's `Release` workflow builds the `tessera` binaries and the
   `curl | sh` installer and publishes the GitHub Release (a pre-release for `-alpha.N`), whose notes are
   the version's section of the root `CHANGELOG.md`; check that they are only this release's entries. Both
   `nix flake check` legs run on the tag too. A failed Release run publishes nothing and promotes nothing:
   fix the cause and cut the next version. Never move a pushed tag.
5. **Approve and merge the promotion PR.** When the Release run succeeds, `promote-main.yml` opens
   `release/promote-v<version>` → `main` (see below). Check that its body names the new tag and that
   `nix flake check` is green, approve it, and merge it **with a merge commit**. `main` now equals the
   release.

The nix-built Python wheel is not attached to the Release automatically yet: `release-nix.yml` never fires
on its own (#438), and an immutable release rejects uploads after it is published (#440).

## Release notes

The root `CHANGELOG.md` holds one section per release, newest first, under a `## Unreleased` section that
collects what has landed since the last release (#982). Every pull request with a user-facing change adds its
entry under `## Unreleased`, never under a released version.

cargo-dist builds the GitHub Release body from that file, at the tagged commit. It picks the section in this
order (axoproject's `changelog_for_version`, cargo-dist 0.32.0, which parses with `parse-changelog` 0.6.16):

1. the section titled with the exact version, `## [0.1.0-alpha.7] - <date>`; the release is titled after it,
   without the brackets (`0.1.0-alpha.7 - <date>`);
2. otherwise, the section of the version without its prerelease suffix (`## [0.1.0] - …`), retitled with the
   full version;
3. otherwise, for a prerelease only, `## Unreleased`, titled `Version <version>`.

So a release takes exactly its own section once step 1 of the checklist has run. Until #982, step 1 did not
exist and the file had only `## Unreleased` at every tag, so each release fell through to case 3 and its notes
listed every entry since the first alpha.

`scripts/changelog.py` keeps the file in that shape, and the `changelog-sections` flake check runs its
`check` mode on every pull request and its `--self-test`. `check` requires `## Unreleased` first, every other
section `## [<semver>] - <YYYY-MM-DD>` and newest first, a section for the workspace version in
`tessera/Cargo.toml` (which is why the release PR fails until step 1), every entry in exactly one section, and
exactly the compare links at the end. The sections for `v0.1.0-alpha.1` to `v0.1.0-alpha.6` were rebuilt
from the file at each tag with its `split` mode:

```bash
scripts/changelog.py split CHANGELOG.md \
  v0.1.0-alpha.1 v0.1.0-alpha.2 v0.1.0-alpha.3 \
  v0.1.0-alpha.4 v0.1.0-alpha.5 v0.1.0-alpha.6
```

A release's entries are the bullets in the file at its tag that are absent at the previous tag. `split`
refuses, instead of guessing, when a bullet at a tag is no longer in the file word for word.

## The promotion PR

`main`'s ruleset requires a pull request, one approving review and a green `nix flake check`, and forbids
force-pushes; only an organization admin can bypass it. So no workflow merges or pushes `main`. The
`promote-main` workflow does everything up to the approval:

- It runs after every successful `Release` run on a tag (`workflow_run`). A `release: published`
  trigger would never fire here, because cargo-dist publishes the Release with `GITHUB_TOKEN`, and events
  caused by `GITHUB_TOKEN` start no workflows (#438).
- `scripts/promotion-plan.sh` decides what to open, and fails the run rather than guess:
  - **Nothing**, when `main` already contains the release (a re-run, or a promotion already merged).
  - **Refuses** a tag that is not `v<semver>`, a tag that moved since the Release run built it, and a
    release older than the newest `v<semver>` tag: `main` never moves to anything but the latest release.
  - **The head**: the tag commit itself when `main` is its ancestor; otherwise a new commit whose tree is
    exactly the tag's tree and whose parents are `main`'s tip and the tag commit. The ruleset requires the
    head to be up to date with `main`, and after the first promotion `main` always has a merge commit the
    next tag lacks. A head that is the bare tag commit would need an "Update branch" after approval, which
    pushes a new head, dismisses the approval and restarts `nix flake check`.
  - **Refuses** a `main` whose merge with the tag conflicts, or whose tree is not exactly the tag's tree.
    Either means `main` holds a change the release lacks. The promotion commit has the tag's tree, so it
    would drop that change from `main`, and merging would publish an unreleased tree as the release.
- `scripts/promotion-head.sh` builds that head and creates the branch (#925). It makes the two-parent
  commit with GitHub's Git Data API (`POST /repos/{owner}/{repo}/git/commits`, only message, tree and
  parents), not by merging, so nothing can conflict. With no custom author, committer or signature,
  GitHub signs a commit an App creates this way, which the `Signed commits` ruleset (every ref, no bypass
  actors) requires. GitHub's merges API, used before, produced an unsigned merge commit that the ruleset
  refused (HTTP 409). Before it creates the branch, the script reads the head back from GitHub and
  requires the tag's tree, the parents `[main, tag]` and a verified signature
  (`.verification.verified == true`). A head that fails any of these fails the run with no branch and no
  PR. There is no fallback.
- The PR is opened with a token of the `vigos-devkit-upgrade` GitHub App (org secrets
  `DEVKIT_UPGRADE_APP_CLIENT_ID` / `DEVKIT_UPGRADE_APP_PRIVATE_KEY`, the App `devkit-upgrade.yml` also
  uses), so its CI runs and its bot author is exempt from the branch-name check. The token carries
  contents, pull requests and `workflows: write`: a release usually changes `.github/workflows/*`
  relative to `main`, and GitHub refuses an App such a ref or commit without `workflows`. The release-plz
  App is not used because it has no `workflows` permission and keeps none (owner decision, 2026-11-15):
  the release PR it opens never touches a workflow.
- The head adds either no commit (the bare tag) or the App's two-parent promotion commit, which changes
  nothing relative to the tag. A head that fails a rule gets no branch, so a re-run starts clean.

The flake check `release-promotion` runs both scripts' self-tests, which prove each of these rules on
throwaway repositories (`promotion-head.sh` against a fake `gh` that answers from a local repository).

### When the workflow fails

The run's error names the rule that failed. Fix the cause, then re-run it for the release:

```bash
gh workflow run promote-main.yml -f tag=v<version>                 # open the PR
gh workflow run promote-main.yml -f tag=v<version> -f dry_run=true # print the plan only
```

A manual run uses the workflow and scripts on `--ref`, which defaults to the default branch (`dev`), so a
fix to the promotion takes effect for re-runs once it is merged into `dev`.

- *`release/promote-v<version>` exists but has no open PR*: an earlier run created the branch and then
  failed to open the PR, or someone else made it. Inspect the branch, delete it, and re-run.
- *the head … is not signature-verified* (or *has tree …* / *has parents …*): GitHub stored a commit
  that breaks a rule, and no branch was created. Nothing needs cleaning up; find out why (for a signature,
  the request must carry no author or committer) before re-running.
- *main holds changes that `v<version>` does not* (or *conflicts*): something reached `main` outside a
  promotion. Bring that change to `dev`, release it, and promote that release.
- *not the latest release*: a newer tag exists; promote that one instead.

When two promotion PRs are open (a release was cut before the previous PR merged), merge only the newest
and close the older: merging the older first leaves the newer one behind `main`, and it would then need
an "Update branch" and a second approval.
