---
type: issue
state: open
created: 2026-09-29T11:21:26Z
updated: 2026-09-29T11:21:26Z
author: c-vigo
author_url: https://github.com/c-vigo
url: https://github.com/vig-os/tessera/issues/513
comments: 0
labels: chore
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-30T07:57:07.609Z
---

# [Issue 513]: [[CHORE] bump dicom-rs and parquet/arrow to clear 4 out-of-range Cargo security alerts](https://github.com/vig-os/tessera/issues/513)

### Chore Type

Dependency update

### Description

Four open Dependabot alerts on `tessera/Cargo.lock` cannot be cleared by either dependency bot, because the fixed versions sit outside the semver range our direct dependencies allow. Each needs a deliberate direct-dependency bump.

| Alert | Crate (locked) | Severity | Advisory | Fixed in | Pulled in by |
|---|---|---|---|---|---|
| #44 | `jxl-grid` 0.5.3 | high | GHSA-5pmv-rx8r-wmv5 | 0.6.2 | `jxl-oxide` |
| #43 | `jxl-oxide` 0.10.2 | medium | GHSA-66m8-c62j-h6v5 | 0.12.6 | `dicom-transfer-syntax-registry` 0.9.1 |
| #42 | `jxl-modular` 0.9.1 | medium | GHSA-2v8p-fqpx-2q3w | 0.11.3 | `jxl-oxide` |
| #45 | `thrift` 0.17.0 | medium | GHSA-2f9f-gq7v-9h6m | 0.23.0 | `parquet` 58.3.0 |

**jxl-\* (via dicom-rs).** `dicom-transfer-syntax-registry` 0.9.1 declares `jxl-oxide = "^0.10.2"` (optional, behind its `jpegxl` feature). The fixed `jxl-oxide` 0.12.6 is out of that range. `dicom-transfer-syntax-registry` 0.10.0 requires `jxl-oxide ^0.12.6`, so this needs the dicom-rs family bumped 0.9 → 0.10 (`dicom` and `dicom-transfer-syntax-registry` in `tessera/Cargo.toml`).

Note: we do not enable `jpegxl`, so the jxl crates are **lockfile-only**. `cargo tree --target all --all-features -i jxl-oxide` finds nothing in the build graph. They are still in `Cargo.lock`, so the alerts stay open and the lockfile carries the vulnerable versions.

**thrift (via parquet).** `cargo tree -i thrift`: `thrift 0.17.0 ← parquet 58.3.0 ← tessera-ingest ← tessera-cli`. `parquet` 58.3.0 requires `thrift ^0.17`. `parquet` 59.0.0 and later no longer depend on `thrift` at all. `parquet` is exact-pinned (`=58.3.0`, `tessera/Cargo.toml` ~L103) together with the `arrow-*` crates (`=58.3.0`, ~L90–93), so the bump has to move the arrow family in lockstep (latest: 60.0.0).

### Acceptance Criteria

- [ ] dicom-rs bumped to 0.10.x; `jxl-grid`, `jxl-oxide` and `jxl-modular` are at or above their fixed versions in `tessera/Cargo.lock`, or gone from it
- [ ] `parquet` and the `arrow-*` pins moved to a release ≥ 59.0.0; `thrift` no longer appears in `tessera/Cargo.lock`
- [ ] The DICOM and Parquet/Arrow ingest lanes pass their tests, and any content-hash or decoder-digest effects of the bump are accounted for
- [ ] Dependabot alerts #42–#45 close

### Implementation Notes

- The two bumps are independent and can land as separate PRs.
- Both are majors under 0.x semver and may carry API breaks; the `=` pins on parquet/arrow are deliberate (see the comment block above them), so re-read that rationale before moving them.

### Related Issues

Refs #467 (dependency-bot standardisation; neither bot can fix these in range)

### Priority

High

### Changelog Category

Security

### Additional Context

Found while retiring `dependabot.yml` in favour of Renovate. Verified with `cargo tree -i` and `Cargo.lock` inspection, and the crates.io dependency metadata of the candidate versions.

