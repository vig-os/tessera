---
type: issue
state: open
created: 2026-11-03T18:07:31Z
updated: 2026-11-03T18:07:31Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/758
comments: 0
labels: bug
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-04T08:35:52.353Z
---

# [Issue 758]: [fix(build): tessera's vortex fork pin is a workspace [patch], so dependents silently build with crates.io vortex 0.75.0 and lose both determinism fixes](https://github.com/vig-os/tessera/issues/758)

Found by the ADR-0064 PR G proof (#715). The consumer reproducer (MorePET/visionx-rs, `visionx-tsra`) resolves vortex **0.75.0 from crates.io**.

Tessera pins vortex to the gerchowl/vortex fork (PR #481) through a workspace-level `[patch]`. Cargo applies `[patch]` only in the root workspace; dependents don't inherit it. So every downstream crate that depends on tessera, by git or registry, builds without the two determinism fixes the fork carries:
- **#468:** container bytes differ under `debug_assertions`.
- **#472:** cross-arch `Stat::Sum` NaN divergence.

A downstream product can therefore seal a different `content_hash` than tessera's own CLI for the same input, on another arch or profile. This breaks S15 writer-determinism for every consumer, and nothing reports it.

**Scope:** a design decision (record it as an ADR or a decided issue), then a fix plus a test.
- Options: a published fork crate under its own name; a documented, required `[patch]` for consumers that is checked at runtime; or blocking on upstream releases (#480). Avoid any option that relies on consumers remembering to add a patch silently.
- Minimum guard: tessera detects at build or run time that it is linked against a vortex without the fixes, and fails or warns loudly.

Related: #480 (removing the pin once upstream releases both fixes), #677 consumer.

Refs: #715
