---
type: issue
state: open
created: 2026-11-17T21:16:04Z
updated: 2026-11-18T00:24:16Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/899
comments: 1
labels: dependencies, chore
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-18T09:43:21.191Z
---

# [Issue 899]: [chore(deps): drop the chrono/humantime unmaintained ignores once upstream moves off chrono](https://github.com/vig-os/tessera/issues/899)

`tessera/deny.toml` ignores two **unmaintained** advisories, added in #898:

| ID | Crate | Why it's ignored |
|---|---|---|
| RUSTSEC-2026-0543 | chrono 0.4.45 | unmaintained, all versions affected, no safe upgrade (chronotope/chrono#1768) |
| RUSTSEC-2026-0542 | humantime 2.3.0 | unmaintained because it wraps chrono, all versions affected |

Neither advisory is a vulnerability. **No tessera crate uses chrono or humantime directly**: neither appears in any
`crates/*/Cargo.toml`, and nothing in `crates/` names `chrono::` or `humantime::`. So there's no code of ours to
migrate to `jiff`. Every path is transitive.

## Paths that reach chrono (`cargo tree -i chrono -e normal,build,dev --workspace --all-features`)
- arrow-array 58.4.1 / parquet 58.4.1: the arrow-58 ingest pins (tessera-cli, tessera-ingest dev)
- arrow-array / arrow-cast / arrow-ord / arrow-select 59.3.2 → vortex-arrow / vortex-file / vortex-layout 0.93 → tessera-io
  (so every binary, tessera-py included)
- dicom-core 0.10 → dicom-* → tessera-ingest
- object_store 0.14 → the tessera-io `cloud` feature and a tessera-cli dev-dependency
- datafusion 54 / arrow-* 58.3 / object_store 0.13.2 / chrono-tz → the tessera-cli `sql` feature

## Paths that reach humantime
- object_store 0.14 → the tessera-io `cloud` feature and a tessera-cli dev-dependency
- object_store 0.13.2 → datafusion 54 → the tessera-cli `sql` feature
- trycmd 0.15 (and humantime-serde) → a tessera-cli dev-dependency

## Removal condition
Drop each ignore once `cargo tree -i <crate> -e normal,build,dev --workspace --all-features` comes back empty. That means
arrow-rs (58 and 59 lines, via the vortex fork), dicom-rs, object_store, datafusion and trycmd have all moved off
chrono/humantime (most likely to `jiff`), or chrono is maintained again and the advisory is withdrawn. Re-check on every
arrow, parquet, vortex, dicom, object_store, datafusion or trycmd bump.

If a tessera crate ever needs a date/time type, use `jiff`, not chrono.

---

# [Comment #1]() by [gerchowl]()

_Posted on November 18, 2026 at 12:24 AM_

Nit from the #902 review: the deny.toml chrono comment lists arrow-ord/-select 59.3.2 as depending on chrono. Only arrow-array and arrow-cast 59 depend on it directly; the others get it through arrow-array. Fix the wording when this ignore is revisited.

