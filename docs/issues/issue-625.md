---
type: issue
state: open
created: 2026-10-18T13:30:39Z
updated: 2026-10-18T23:46:37Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/625
comments: 1
labels: chore, priority:low, area:ci
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-19T07:17:43.796Z
---

# [Issue 625]: [ci(release): follow-ups from the #610 review (tag tree check, pin comment, root scripts in the coverage registry)](https://github.com/vig-os/tessera/issues/625)

Follow-ups from the review of #610 (#606), none blocking:

1. `scripts/last-release-tag.sh` (`resolve`, ~L62-75): assert that the selected tag's tree contains `tessera/Cargo.toml`
   and fail with a message naming the format-milestone tag clash (#608). Today, once the workspace reaches 0.1.0, the
   ceiling selects the format tag `v0.1.0` (no `tessera/` dir) and release-plz fails later with a bare missing-manifest error.
2. `.github/workflows/release-plz.yml`: the `release-plz/git-config@5914485…` pin lacks the `# vX`/provenance comment every
   other pin carries.
3. `flake.nix` `test-coverage` script registry scans only `./tessera`, so repo-root `scripts/*.sh`
   (`last-release-tag.sh`, `feature-snapshots.sh`) escape the "every script is declared" rule (pre-existing gap).

---

# [Comment #1]() by [gerchowl]()

_Posted on October 18, 2026 at 11:46 PM_

4. From the #630 review: the bot exemption on the branch-name gate (`ci.yml` comment + `docs/COMMIT_MESSAGE_STANDARD.md:105`) applies to **any** GitHub App / Bot account, not only the repo's configured bots. Say "any GitHub App account" explicitly. This is acceptable as is (the gate is cosmetic and `user.type` cannot be claimed by a human), but the wording invites the misreading.

