---
type: issue
state: open
created: 2026-09-24T07:41:46Z
updated: 2026-09-28T17:59:30Z
author: c-vigo
author_url: https://github.com/c-vigo
url: https://github.com/vig-os/tessera/issues/440
comments: 0
labels: bug, priority:medium, area:ci
assignees: none
milestone: devkit release adoption
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:34.474Z
---

# [Issue 440]: [release-nix can never attach the wheel: immutable releases reject uploads after publish (not fixed by #438/#439)](https://github.com/vig-os/tessera/issues/440)

## Summary

`release-nix.yml` can never attach the wheel, and fixing the trigger (#438 / #439) will not change that. The org enforces **immutable releases**; cargo-dist's `announce` job creates the GitHub Release already **published**, so no asset can be appended afterwards — by any trigger, at any time.

## Evidence

The only `release-nix` run to date — `workflow_dispatch`, 2026-09-24T01:00Z, run [35940975248](https://github.com/vig-os/tessera/actions/runs/35940975248) — failed on **both** arches at "Attach the wheel to the release":

```
HTTP 422: Cannot upload assets to an immutable release.
(https://uploads.github.com/repos/vig-os/tessera/releases/395149868/assets?label=&name=tessera-0.1.0a1-cp39-abi3-linux_x86_64.whl)
```

```
HTTP 422: Cannot upload assets to an immutable release.
(https://uploads.github.com/repos/vig-os/tessera/releases/395149868/assets?label=&name=tessera-0.1.0a1-cp39-abi3-linux_aarch64.whl)
```

Same release id (`395149868` = `v0.1.0-alpha.1`), same step, both jobs.

## Why #438/#439 do not fix it

#438 diagnoses the *trigger*: cargo-dist's `announce` creates the Release with `GITHUB_TOKEN`, which cannot fire `release: published`, so `release-nix` never starts on its own. That diagnosis is correct and #439 chains it via `workflow_run`.

But the 422 above came from a run that **did** start — it was dispatched by hand. So the trigger and the upload are two independent failures, and #439 fixes only the first. Once it merges, `release-nix` will start reliably and then fail on the 422 exactly as it did here.

## The shape of the fix

The asset must be uploaded while the Release is still a **draft**, and the draft flipped to published once, afterwards. That is a change to *who creates the Release and when*, not to the trigger:

- cargo-dist's `host` job currently runs `dist host --steps=upload --steps=release` and then `gh release create`, i.e. it creates and publishes in one motion.
- `release.yml` is **generated** and must not be hand-edited (its `plan` job regenerates it and fails on drift — see #433), so the change belongs in `dist-workspace.toml` configuration or in a redesign of which tool owns the Release object.

`vig-os/scitadel`'s `binaries.yml` already solves this exact problem and documents the reasoning: the Release is created as a **draft**, the build matrix uploads into that draft, and a separate `publish` step flips draft → published once. Worth reading before designing tessera's version.

## Scope note

This is independent of the open question of whether tessera moves to the vigOS devkit release train (see the companion issue). It is a live defect under the **current** pipeline: the nix reproducibility channel that #376 built does not deliver, and the v0.1.0-alpha.1 release has no wheel attached. If the release mechanism is redesigned, this constraint is a requirement on whatever replaces it; if it is not, this still needs fixing on its own.

## Acceptance

- [ ] A wheel is attached to a published GitHub Release, produced by CI without a manual dispatch.
- [ ] Re-running the publish path is idempotent (a re-dispatch does not fail on an existing asset).
- [ ] The mechanism is documented where the next reader will hit it — the `release-nix.yml` header currently states the `release: published` trigger as if it worked.

