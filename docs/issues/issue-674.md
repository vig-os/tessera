---
type: issue
state: open
created: 2026-10-25T11:19:02Z
updated: 2026-10-25T11:19:02Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/674
comments: 0
labels: question, priority:low, area:workspace
assignees: none
milestone: none
projects: none
parent: 654
children: none
synced: 2026-10-26T07:24:18.258Z
---

# [Issue 674]: [question: declare a workspace MSRV now that DataFusion is public API (ADR-0062)](https://github.com/vig-os/tessera/issues/674)

Open question from ADR-0062 §2 (#654), for the owner.

No crate declares `rust-version`; the toolchain is `stable` from the flake. With DataFusion becoming public API
(`tessera-query`), should the workspace declare an MSRV? Today the effective floor is Vortex 0.75's 1.91.0
(DataFusion 54 requires 1.88.0).

**Recommendation:** declare `rust-version` in `[workspace.package]` at the effective floor, and raise it
deliberately in the PR that bumps the dependency that forces it.

