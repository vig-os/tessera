---
type: issue
state: closed
created: 2026-10-18T12:01:46Z
updated: 2026-10-18T13:30:46Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/606
comments: 1
labels: bug, priority:blocking, area:ci
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-19T07:17:50.693Z
---

# [Issue 606]: [ci(release): release-plz never proposes the next release because the crates are not on crates.io](https://github.com/vig-os/tessera/issues/606)

## Summary

`release-plz release-pr` on `dev` never proposes the release after `0.1.0-alpha.1`. Every run since the alpha.1 tag ends with
`the repository is already up-to-date` and `{"prs":[]}`, so the release PR for `0.1.0-alpha.2` never opens, although `dev`
holds many unreleased `feat`/`fix` commits (e.g. #585).

## Root cause (reproduced)

release-plz decides "last released version" from the **cargo registry** unless told otherwise. Tessera's crates are
deliberately not on crates.io (owner decision 2026-08-12, "hold crates.io"), so every lookup is
`WARN Package tessera-core@*.*.* not found` (run on eb12822, 2026-10-18), release-plz treats each crate as never released, and the
"next" version equals the current one, `0.1.0-alpha.1` → nothing to do. #436 assumed that tagging `v0.1.0-alpha.1` would let
release-plz compute the next version; release-plz does not read git tags in its default mode.

Probes with release-plz 0.3.169 (the action's default) on eb12822:

| Mode | Result |
|---|---|
| default (registry) | `next version is 0.1.0-alpha.1`, no PR (the CI behaviour) |
| `git_only = true` + `git_tag_name = "v{{ version }}"` | `failed to determine next versions: create unreleased repo for spinning worktrees: could not find repository at <repo>/tessera` — git_only opens the repo at the **manifest's directory**, and our workspace lives in `tessera/`. Same in a plain clone, so it is an upstream limitation, not a worktree artefact. |
| `--registry-manifest-path <checkout of v0.1.0-alpha.1>/tessera/Cargo.toml` | all six crates `next version is 0.1.0-alpha.2`; Cargo.toml/Cargo.lock bumped and five CHANGELOGs updated (`tessera-wasm: already up to date`) |

## Decided fix

Compare against the last **release tag** instead of the registry, which is the documented use of `--registry-manifest-path`:

1. In `.github/workflows/release-plz.yml`, resolve the newest release tag (`v*`, semver order incl. pre-releases, never a
   lexical sort), `git worktree add` it outside the checkout, and run `release-plz release-pr --manifest-path tessera/Cargo.toml
   --registry-manifest-path <worktree>/tessera/Cargo.toml`. The pinned `release-plz/action` has no input for that flag, so install
   the pinned release-plz version and call the CLI (keeping the App-token step and SHA pins). Fail loudly when no release tag
   exists, never fall back silently to the registry.
2. Update the header comments of `release-plz.yml` and `release-plz.toml`, which describe the flow wrongly.
3. Leave `git_only` out until upstream supports a workspace in a subdirectory (to be reported upstream).

Acceptance: a workflow run on the fix branch (`workflow_dispatch`) or on `dev` after merge opens the release PR proposing
`0.1.0-alpha.2` for the workspace crates.

---

# [Comment #1]() by [gerchowl]()

_Posted on October 18, 2026 at 01:30 PM_

Fixed by #610 (merge e771a9c): release-plz now baselines on the newest v* release tag via --registry-manifest-path. Follow-ups: #608, #625.

