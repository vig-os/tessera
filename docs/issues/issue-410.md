---
type: issue
state: closed
created: 2026-08-21T20:36:32Z
updated: 2026-09-28T21:37:12Z
author: c-vigo
author_url: https://github.com/c-vigo
url: https://github.com/vig-os/tessera/issues/410
comments: 1
labels: none
assignees: none
milestone: 0.1.0-alpha.2
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:37.894Z
---

# [Issue 410]: [Add devkit-standard CodeQL workflow (actions leg)](https://github.com/vig-os/tessera/issues/410)

## Context

The org is retiring CodeQL **default setup** in favor of the devkit-standard advanced `codeql.yml` everywhere ([vig-os/org-config#189](https://github.com/vig-os/org-config/issues/189)). tessera is one of the three repos still on default setup — currently scanning exactly `["actions"]`, auto-detected by GitHub since the repo's own language (Shell) is not CodeQL-supported.

## Task

Add the devkit-managed `codeql.yml` with an **`actions` leg only**: the four workflows under `.github/workflows/` are the repo's entire CodeQL-analyzable surface. `devkit-smoke-test` is the precedent — same situation (Shell repo, workflows-only scanning) and already carries the standard workflow.

Optionally, once the workflow reports: a follow-up org-config change request to add its check to tessera's `Main protection` required status checks, matching how devkit and org-config gate their CodeQL legs.

## Ordering — blocked by org-config#189

GitHub rejects CodeQL SARIF uploads from workflows while default setup is enabled on the repo. Default setup must be off (org-config#189 steps 1-2 applied) **before** this workflow lands, or its first run fails.

Refs: vig-os/org-config#189
---

# [Comment #1]() by [gerchowl]()

_Posted on September 28, 2026 at 09:37 PM_

Done — landed via the devkit scaffold adoption in #442, and verified green rather than merely present.

**The workflow is in place.** `.github/workflows/codeql.yml` on `dev` is the devkit-managed advanced
configuration with exactly the `actions` leg this issue asked for:

```yaml
strategy:
  fail-fast: false
  matrix:
    language: ['actions']
```

**The ordering blocker is cleared.** This issue was blocked on org-config#189 steps 1-2, because
GitHub rejects SARIF uploads from an advanced configuration while default setup is enabled. Default
setup is now off:

```
$ gh api /repos/vig-os/tessera/code-scanning/default-setup --jq .state
not-configured
```

**And it is actually producing analyses**, which is the part a file-presence check would miss:

```
$ gh api '/repos/vig-os/tessera/code-scanning/analyses' --jq '.[]|"\(.created_at) \(.category) \(.tool.name) \(.ref)"'
2026-09-28T12:16:00Z  /language:actions  CodeQL  refs/pull/445/merge
2026-09-28T08:44:06Z  /language:actions  CodeQL  refs/heads/dev
2026-09-28T06:14:27Z  /language:actions  CodeQL  refs/pull/444/merge
```

So there is a post-merge analysis on `refs/heads/dev` and per-PR analyses, and the
`CodeQL Analysis (actions)` check reports on PRs (59s on #443).

The optional follow-up in this issue is **not** done: adding the `CodeQL Analysis (actions)` check to
tessera's `Main protection` required status checks is an org-config change, matching how devkit and
org-config gate their own CodeQL legs. Worth noting that `dev`'s protection currently requires only
`nix flake check`. Filing that as a request against org-config rather than holding this issue open.

