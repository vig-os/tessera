---
type: issue
state: open
created: 2026-06-25T11:39:55Z
updated: 2026-09-28T17:59:43Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/210
comments: 0
labels: epic, python, area:bindings
assignees: none
milestone: backlog / research
projects: none
parent: none
children: none
synced: 2026-09-29T07:53:05.030Z
---

# [Issue 210]: [[P7][epic] Bindings & ops (tessera-py pyo3 · tessera-wasm · ref stack · migration CLI)](https://github.com/vig-os/tessera/issues/210)

**Phase P7 → v0.5.** ROADMAP: tessera/docs/ROADMAP.md

`tessera-py` (**pyo3 wrapping the core** — same engine, not a reimpl) + `tessera-wasm` (**Rust→WASM** for TS/browser readers); reference podman-compose stack (zot+MinIO+InvenioRDM+cosign); migration tooling (`schema diff/validate`); format-spec semver policy.

**Done-gate:** Python/TS parity passes conformance; ref-stack smoke test. *(Note: bindings are reach, not the v1.0 independent-reader gate — see P8.)*
