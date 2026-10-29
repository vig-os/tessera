---
type: issue
state: open
created: 2026-10-29T07:47:53Z
updated: 2026-10-29T07:47:53Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/701
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-29T08:03:58.800Z
---

# [Issue 701]: [ci: re-derive nix-check's timeout and --max-jobs/--cores from ksb x86_64 timings](https://github.com/vig-os/tessera/issues/701)

Follow-up to #688 (PR #700). The x86_64 `nix flake check` leg now runs on `ksb-vig-os-large` (8 CPU / 32 GiB) instead of a 16 GB GitHub-hosted runner. Two settings in `.github/workflows/nix-check.yml` were derived only from GitHub-hosted measurements:

- `timeout-minutes: 120`: worst hosted x86 draw (89m) plus ~35%.
- `--max-jobs 2 --cores 4`: the #495 memory fix for a 16 GB runner.

Both are kept unchanged on ksb for now. That's safe: 32 GiB is twice the memory these flags were tuned for, and the cap only bounds a hung leg.

**Do:** once at least 5 x86_64 legs have run on ksb, collect their durations and peak `[res]` memory from the sampler. Then re-derive both settings with the same method the comment documents: worst draw plus margin, and memory headroom. Record the table in the comment and close this issue. Don't widen either setting without the measurements.

Refs: #688
