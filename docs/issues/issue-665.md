---
type: issue
state: open
created: 2026-10-25T11:12:51Z
updated: 2026-10-25T11:58:28Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/665
comments: 1
labels: dependencies, chore, priority:low
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-26T07:24:22.708Z
---

# [Issue 665]: [chore(deps): drop basic-toml (RUSTSEC-2026-0374) once i18n-config releases the toml swap](https://github.com/vig-os/tessera/issues/665)

## What

`tessera/deny.toml` ignores **RUSTSEC-2026-0374** (`basic-toml` 0.1.10, unmaintained, no safe upgrade). The ignore
was added by #664. This issue tracks removing it.

## Why it's in the graph

```text
basic-toml 0.1.10
└── i18n-config 0.4.8
    ├── i18n-embed-fl 0.9.4 (proc-macro)  ← age 0.11.5
    └── i18n-embed-impl 0.8.4 (proc-macro) ← i18n-embed 0.15.4 ← age 0.11.5
age 0.11.5 ← tessera-ingest (ADR-0047 crypto-shred encryption)
```

`i18n-config` 0.4.8 is the latest release, and it still depends on `basic-toml`. Bumping `age` to 0.12.x doesn't
help: it moves to `i18n-embed-fl` 0.10 → `i18n-config ^0.4.7` → the same 0.4.8.

Reachability: `basic-toml` is only reached through proc-macro crates. They parse `age`'s own bundled `i18n.toml`
at compile time. It isn't linked into any tessera binary and never sees runtime or user input.

## Removal path

Upstream already dropped the crate: kellpossible/cargo-i18n#176 (merged 2026-09-26) moves `i18n-config` back to
`toml`, but it's unreleased.

1. Wait for the next `i18n-config` release that carries #176.
2. If it's semver-compatible with `^0.4.7`, bump `i18n-config` in `tessera/Cargo.lock`. If not, wait for an `age`
   release that accepts it.
3. Delete the `RUSTSEC-2026-0374` entry from `tessera/deny.toml` and confirm `cargo deny check` stays clean.

## Acceptance

`basic-toml` is gone from `tessera/Cargo.lock`, and `RUSTSEC-2026-0374` is removed from `tessera/deny.toml`.

---

# [Comment #1]() by [gerchowl]()

_Posted on October 25, 2026 at 11:58 AM_

Nit from the #676 review: the deny.toml comment on the basic-toml ignore says RUSTSEC-2026-0374 is "dated 2026-09-26". RustSec says it was **issued 2026-10-20**; 2026-09-26 is the date it was reported. Correct the date when this ignore is removed or touched.

