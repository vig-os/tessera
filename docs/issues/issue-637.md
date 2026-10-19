---
type: issue
state: open
created: 2026-10-19T00:00:04Z
updated: 2026-10-19T00:00:04Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/637
comments: 0
labels: feature, priority:medium, area:testing, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-19T07:17:40.497Z
---

# [Issue 637]: [test(io): table-block size ratchet so an encoder change cannot grow sealed products unnoticed](https://github.com/vig-os/tessera/issues/637)

Part of #635 (vortex upgrade path). **Gap:** encoder changes can grow sealed table blocks unnoticed. The only size assertion is
`f64_column_is_compressed_by_pco_not_stored_raw` (`table.rs:2550`, < 0.7× raw); `perf-budgets.toml` has every budget commented out and
no check reads it; `grid_decode_perf_ratchet_narrow_table` (`table.rs:2772`) skips below 8 cores, so it is off in CI.

**Ask:** a deterministic **size ratchet** per corpus table fixture (encoded block bytes per column against a committed baseline, fail on a
regression beyond a stated tolerance, print the delta on any change, re-baseline deliberately), runnable in the flake. Speed stays
out of CI (runner-dependent), but give it an explicit, documented way to run the decode/encode benches before and after a bump.
Also note: `table.rs:1640` hardcodes encoded lengths 2772/2916/2444, which a vortex bump will move although the comment says it
shouldn't fire on a dependency update. Make its intent and its re-baseline path explicit.

