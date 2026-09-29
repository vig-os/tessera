---
type: issue
state: closed
created: 2026-09-29T01:10:57Z
updated: 2026-09-29T05:01:56Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/483
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:25.866Z
---

# [Issue 483]: [ci(nix-check): one unrelated derivation's substituter failure cancels all 17 flake checks — no verdict at all](https://github.com/vig-os/tessera/issues/483)

## Symptom

A transient `cache.nixos.org` download failure for **one** derivation aborts the entire
`nix flake check`, so **every** check is cancelled and the leg reports failure without any check having
produced a verdict.

Observed on #481's aarch64 leg (run
[36506225691](https://github.com/vig-os/tessera/actions/runs/36506225691)):

```
warning: unable to download '…/14wzm9dp….nar.xz': HTTP error 200 (curl error: Failed sending data to
         the peer); retrying from offset 19940032 …
error:   unable to download '…/14wzm9dp….nar.xz': HTTP error 416
error:   path '/nix/store/…-minio-2025-10-15T17-29-55Z-go-modules' is required, but there is no
         substituter that can build it
error:   some substitutes … failed (usually happens due to networking issues);
         try '--fallback' to build derivation from source
error:   Cannot build '/nix/store/…-minio-2025-10-15T17-29-55Z.drv'.
```

All 17 checks then show `(cancelled)`:

```
❓ checks.aarch64-linux.seal-profile-determinism (cancelled)
❓ checks.aarch64-linux.feature-snapshots        (cancelled)
❓ checks.aarch64-linux.workspace-test           (cancelled)
… 14 more, all cancelled
```

## Why this is worth fixing rather than re-running

Three separate problems, in increasing order of importance:

1. **The failure is unrelated to what it kills.** `minio` is pulled in only by `minio-range-read`, the
   cloud-feature range-read check. Its CDN download failing took down the determinism gates
   (`seal-profile-determinism`, `feature-snapshots`), `workspace-test`, clippy, the wasm checks — none
   of which depend on it.

2. **The red is actively misleading.** On #481 — a PR whose entire purpose is fixing an
   *architecture-dependent* determinism bug — a red aarch64 leg reads as "the fix does not work on
   aarch64". It is not: nothing ran. Two reviewers independently had to dig into the log to establish
   that, which is exactly the cost a misleading signal imposes. A cancelled check and a failed check
   should not look the same from the PR list.

3. **`nix flake check` is all-or-nothing by default.** One substituter hiccup anywhere in the closure
   discards the work of every other check, including ones that had already been built.

## Suggested fix

In `.github/workflows/nix-check.yml` (this repo's own file, not devkit-managed):

- add **`--fallback`**, which is the hint nix itself prints: build from source when substitution fails,
  instead of aborting. That converts this class from a failure into a slower run.
- add **`--keep-going`** so a genuine failure in one check still lets the other 16 report. Today a
  single failure hides every other result, which also makes triage slower than it needs to be.

Worth measuring the `--fallback` cost on a hosted aarch64 runner before committing to it — building
`minio` from source there may be slow enough to hit the 90-minute timeout, in which case `--keep-going`
alone plus a retry is the better trade. `--keep-going` is cheap and useful regardless.

## Related: a second infrastructure failure in the same batch

#461's x86_64 leg died with **exit code 143** (SIGTERM — runner eviction) mid-`cargo nextest`, after the
tests had passed. Different cause, same trap: it reads as a code failure from the status alone.

No evidence the two are related beyond timing, and I am not claiming a common root cause. But two
independent infra failures inside an hour, both of which required log archaeology to distinguish from
real failures, is enough to justify making the distinction visible rather than leaving it to whoever
reads the PR next.

Not folded into #481, which is deliberately scoped to the vortex pin (13 files, confirmed with the
reviewer). Happy to take this as a follow-up.

Refs #481, #463
