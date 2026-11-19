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

1. **Merge the release PR.** release-plz keeps one PR (`chore: release v<version>`, branch
   `chore/release-plz-<timestamp>`) up to date with the next version and its CHANGELOG. Review it and
   merge it into `dev`. Merging alone tags and publishes nothing.
2. **Tag the merge commit.** On that merge commit, create a signed, annotated tag and push it from the dev
   shell, so the repo's pre-push hooks run:

   ```bash
   git fetch origin
   git tag -s v<version> <merge commit> -m "Tessera v<version> — <one-line summary>"
   nix develop -c git push origin v<version>
   ```

   The tag is the single trigger for everything below.
3. **Watch the Release run.** cargo-dist's `Release` workflow builds the `tessera` binaries and the
   `curl | sh` installer and publishes the GitHub Release (a pre-release for `-alpha.N`). Both
   `nix flake check` legs run on the tag too. A failed Release run publishes nothing and promotes nothing:
   fix the cause and cut the next version. Never move a pushed tag.
4. **Approve and merge the promotion PR.** When the Release run succeeds, `promote-main.yml` opens
   `release/promote-v<version>` → `main` (see below). Check that its body names the new tag and that
   `nix flake check` is green, approve it, and merge it **with a merge commit**. `main` now equals the
   release.

The nix-built Python wheel is not attached to the Release automatically yet: `release-nix.yml` never fires
on its own (#438), and an immutable release rejects uploads after it is published (#440).

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
  - **The head**: the tag commit itself when `main` is its ancestor; otherwise a merge of the tag into
    `main`'s tip, which GitHub makes and signs. The ruleset requires the head to be up to date with `main`,
    and after the first promotion `main` always has a merge commit the next tag lacks. A head that is the
    bare tag commit would need an "Update branch" after approval, which pushes a new head, dismisses the
    approval and restarts `nix flake check`.
  - **Refuses** a merge that conflicts, or whose tree is not exactly the tag's tree. Either means `main`
    holds a change the release lacks, and merging would publish an unreleased tree as the release.
- The PR is opened with a token of the `vigos-devkit-upgrade` GitHub App (org secrets
  `DEVKIT_UPGRADE_APP_CLIENT_ID` / `DEVKIT_UPGRADE_APP_PRIVATE_KEY`, the App `devkit-upgrade.yml` also
  uses), so its CI runs and its bot author is exempt from the branch-name check. The token carries
  contents, pull requests and `workflows: write`: a release usually changes `.github/workflows/*`
  relative to `main`, and GitHub refuses an App such a ref or commit without `workflows`. The release-plz
  App is not used because it has no `workflows` permission and keeps none (owner decision, 2026-11-15):
  the release PR it opens never touches a workflow.
- The head adds either no commit (the bare tag) or GitHub's merge commit, which adds no change. If GitHub
  refuses the merge, the run deletes the branch it had just created at `main`'s tip, so a re-run starts
  clean.

The flake check `release-promotion` runs the script's self-test, which proves each of these rules on
throwaway repositories.

### When the workflow fails

The run's error names the rule that failed. Fix the cause, then re-run it for the release:

```bash
gh workflow run promote-main.yml -f tag=v<version>                 # open the PR
gh workflow run promote-main.yml -f tag=v<version> -f dry_run=true # print the plan only
```

- *`release/promote-v<version>` exists but has no open PR*: an earlier run made the branch and then
  failed after GitHub's merge (a tree or signature mismatch, kept for inspection), or someone else made
  it. Inspect the branch, delete it, and re-run.
- *main holds changes that `v<version>` does not* (or *conflicts*): something reached `main` outside a
  promotion. Bring that change to `dev`, release it, and promote that release.
- *not the latest release*: a newer tag exists; promote that one instead.

When two promotion PRs are open (a release was cut before the previous PR merged), merge only the newest
and close the older: merging the older first leaves the newer one behind `main`, and it would then need
an "Update branch" and a second approval.
