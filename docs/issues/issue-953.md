---
type: issue
state: closed
created: 2026-11-22T17:22:54Z
updated: 2026-11-22T20:46:24Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/953
comments: 0
labels: dependencies, chore
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-23T10:16:28.179Z
---

# [Issue 953]: [chore(deps): url 2.5.8 is flagged by RUSTSEC-2026-0592](https://github.com/vig-os/tessera/issues/953)

cargo-deny (Lint & Format) fails on every PR: advisory RUSTSEC-2026-0592 (stack overflow in `Url::origin()` on nested `blob:` URLs) hits `url` 2.5.8 in `tessera/Cargo.lock`, reached via object_store, reqwest, tower-http and vortex-file. Patched in url >= 2.5.9.

Acceptance: `cargo deny check` exits 0 with no ignore added; `cargo metadata --locked` exits 0; feature-snapshots unchanged or regenerated.
