---
type: issue
state: closed
created: 2026-11-17T21:14:30Z
updated: 2026-11-18T00:24:13Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/898
comments: 0
labels: dependencies, security, priority:blocking
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-18T09:43:21.734Z
---

# [Issue 898]: [chore(deps): cargo-deny red on dev: RUSTSEC-2026-0544 (libz-sys vuln), chrono/humantime unmaintained, yanked async-executor](https://github.com/vig-os/tessera/issues/898)

`cargo-deny` (Lint & Format) now fails on **dev b290237** and on every PR (first seen on PR #894). Three new advisories plus one yanked crate:

| ID | Crate | Kind | Path | Fix |
|---|---|---|---|---|
| RUSTSEC-2026-0544 | libz-sys 1.1.31 (zlib `gz_vacate` heap overflow) | **vulnerability** | build dep of zarrs → tessera-io | `cargo update -p libz-sys` to ≥ 1.1.33 |
| RUSTSEC-2026-0543 | chrono 0.4.45 | unmaintained, all versions, no safe upgrade (chronotope/chrono#1768) | arrow-array/cast/… (58 and 59), dicom-core/…, others | documented `ignore`, same as the existing unmaintained entries; track a migration to `jiff` |
| RUSTSEC-2026-0542 | humantime 2.3.0 | unmaintained (wraps chrono) | transitive | documented `ignore`; track it the same way |
| yanked | async-executor 1.14.0 | yanked | transitive | `cargo update -p async-executor` |

## Acceptance
- **Vulnerability:** fixed by upgrading, not ignored.
- **Unmaintained advisories:** ignored only with a deny.toml comment in the existing style: what the advisory is, why there is no upgrade, every path that reaches the crate, whether tessera calls it directly, and the condition for removing the ignore.
- **Direct use:** if any tessera crate uses chrono directly, file a follow-up to migrate that code to `jiff`.
- **Feature snapshots:** regenerate them if the update moves them, and prove the delta is inert.
- **No hash changes:** no golden or corpus hash moves.
- **CI:** Lint & Format is green.

