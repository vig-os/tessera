---
type: issue
state: closed
created: 2026-09-08T09:43:34Z
updated: 2026-09-28T21:11:49Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/417
comments: 0
labels: none
assignees: none
milestone: 0.1.0-alpha.2
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:36.867Z
---

# [Issue 417]: [tessera inspect never prints `generation` — ADR-0058's sealed recipe has no operator surface](https://github.com/vig-os/tessera/issues/417)

## Motivation

ADR-0058 (landing on `dev` via #409 / PR #415) seals a `generation{config | config_ref}` record —
the *recipe* for how a product was made. It is the ADR's headline artifact: the answer to "how was
this made?", sealed into `manifest_hash` and therefore tamper-evident.

**No CLI verb prints it.** `tessera inspect` renders `producer` (`main.rs:924-925`) and stops
there; `generation` appears nowhere in the CLI.

Found while re-verifying the DP01 acceptance archive for #409. To confirm that the raw product
carried its 10-key DAQ `.ini` recipe, the only route was to bypass the tool entirely:

```python
import zipfile, json
json.loads(zipfile.ZipFile(p).read('manifest.json'))['generation']
```

That is the shape of the gap the #390 onboarding audit already flagged — mechanism present,
operator surface absent. A sealed provenance record a scientist cannot read without a Python
one-liner does not deliver the FAIR **R**(eusable) property the ADR is justified by.

## Scope

**P1**
- [ ] `tessera inspect` prints `generation` when present — the `config_ref` digest, and the
      `config` keys (count + values, or a summary line when large, consistent with how `sources`
      collapses under `--full`).
- [ ] The structured producer's extra fields (`git_commit`, `git_repo`, `dirty`) are reachable —
      `display()` currently folds them to `tool/version (commit)`.

**P2**
- [ ] A machine-readable manifest dump (`inspect --json` or `tessera manifest <file>`), so
      provenance is scriptable without reaching into the container format. This is the general
      form of the gap and probably subsumes most of the above.
- [ ] Consider a `tessera provenance <file>` verb that walks the `derived_from` DAG and shows
      producer + generation + inherited identity per hop — the operator view of ADR-0058 §5.

## Pitfalls

- `config` is a deliberately **non-opinionated** bag (ADR-0058 §2): the keys are the generator's
  business and the CLI must not interpret, validate, or re-order them beyond stable display.
- `config_ref` points at a carried Blob block; resolving/printing that payload is a separate,
  larger job than printing the digest — don't conflate them.
- Whatever `inspect` prints is snapshotted by the `.trycmd` docs-as-tests, so a display change
  means a golden regen.

## References

- ADR-0058 §1/§2 · #409 · PR #415 · #324, #342
- #390 (AX onboarding audit — the operator-surface gap) · #386
- `tessera/crates/tessera-cli/src/main.rs:924`

Refs: #409

