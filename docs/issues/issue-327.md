---
type: issue
state: open
created: 2026-07-03T10:44:36Z
updated: 2026-09-28T17:59:00Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/327
comments: 0
labels: none
assignees: none
milestone: 0.1.0-beta
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:48.844Z
---

# [Issue 327]: [feat(cli): tsra mcp — MCP agent surface follow-ons (structured tools, config, client verification)](https://github.com/vig-os/tessera/issues/327)

First cut landed in 451b0af: a synchronous MCP-over-stdio server (tsra mcp) exposing tsra_tree/tsra_ls/tsra_stats/tsra_verify as tools (hand-rolled JSON-RPC 2.0, no async SDK, matches ADR-0034). Reuses the CLI/view-model. Compile + unit tested (initialize/tools-list/error dispatch).

Follow-ons:
- [ ] SMOKE-TEST against a real MCP client (the protocol is not yet client-verified — the one open risk).
- [ ] Structured-data tools returning JSON not just text: read (columns/rows/SQL) and histogram over RecordBatch (via page_cells / serde), slice/project regions, inspect (manifest/schema/provenance/trust/governance).
- [ ] More verbs as tools (schema, export, log/diff, keygen/sign left OUT of a read surface by policy).
- [ ] Optional: config (which tools enabled), read-only vs edit capability gating, and a cohort/collection tool.

Enables human-TUI + agent-MCP co-driving via the CoW repo (agent commits a new sealed version, human opens it). See docs/spikes/tsra-explorer-wireframes.md Surfaces.

Refs: #286
