---
type: issue
state: closed
created: 2026-08-19T09:20:20Z
updated: 2026-09-28T17:55:19Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/390
comments: 1
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:41.488Z
---

# [Issue 390]: [AX/onboarding audit — 3 fresh-agent runs; quick wins + follow-ups](https://github.com/vig-os/tessera/issues/390)

Ran three fresh-context agents (CLI + published docs only, no source) against the built `tessera` on
sample `.tsra` files, as first-time users.

## Scores
- **Read / inspect / query UX: 4.5/5** — grouped `--help`, every task first-try, `stats`/`slice`/
  `project`/`read` discoverable.
- **Write your own data: 2/5** — ingest is vendor-only; no generic table/array ingest; Python write
  claimed but unshown.
- **Advantage vs Parquet/HDF5: 2/5** — the pitch is never made; "Parquet" appears nowhere in the docs.

## Quick wins (this PR)
- Fix the **SIGPIPE bug**: `tessera slice/project/read … | head` printed `Broken pipe (os error 32)`
  and exited non-zero — now terminates quietly like a standard Unix tool.
- **`docs/book/src/why-tessera.md`** — capability comparison table (Tessera vs Parquet/HDF5/Zarr,
  native/bolt-on/no) + "why not just Parquet + a hash file?"; linked from the mdBook nav + README.
- Expanded the README quickstart (`read --all`/`--rows`, `stats`/`slice`/`project`, Python pointer).

## Follow-ups (filed)
- #386 generic `ingest table/array` (the #1 write-path gap)
- #387 non-CSV array output (`--format npy|png|json`) + size guard
- #388 `tessera bench compare` vs Parquet/HDF5 (numbers to back the pitch)
- #389 ingest cookbook + product-schema reference + Python write example + fix `cli.md` include-stubs
---

# [Comment #1]() by [gerchowl]()

_Posted on September 28, 2026 at 05:55 PM_

Closing as **done** — verified on `origin/dev` in the 2026-09-28 backlog triage.

Evidence: Quick wins landed (commit dd345aa 'AX onboarding quick wins: SIGPIPE fix + why-tessera comparison + quickstart (#391)'); follow-ups filed as separate open issues (#386/#387/#388/#389).

https://claude.ai/code/session_01XdERKMVDAwfMJSKdTytNnK

