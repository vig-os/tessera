---
type: issue
state: open
created: 2026-11-03T02:09:47Z
updated: 2026-11-03T02:09:47Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/743
comments: 0
labels: priority:low, area:testing
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-03T08:59:43.632Z
---

# [Issue 743]: [cli/core: typed NotATable node kind instead of a "node" string; one partitioned-node helper; trailing-slash tests](https://github.com/vig-os/tessera/issues/743)

Follow-up nits from the review of PR #741 (#711, C1), head a60e267.

1. `tessera-cli/src/nav.rs` ~1370 branches on `kind == "node"`, a sentinel string from `LogicalTableError::NotATable { kind: String }` (tessera-core). Make the kind typed (an enum of block kinds plus a node), so the CLI and Python match on a variant, not a string. The Python binding (#717) may depend on the same string; check that.
2. `table_nodes` (CLI) rebuilds the marker-to-parent node discovery that core's `check_partitions` loop uses. Add a core `partitioned_nodes(blocks)` helper and use it in both places, so the node rule lives once.
3. `TableNode::columns`: `serde_json::to_value(s).ok()...unwrap_or_default()` silently prints no columns when serialization fails. Return or propagate the error instead.
4. Missing tests: `ls FILE pairs/` (trailing slash, `nav.rs` ~1185) and `read FILE /` (which trims to an empty name). Each needs a trycmd case.
5. #711 asked for `sql` as a trycmd case; it is a unit test in `sql.rs`. Add the trycmd case, or record why a unit test is the right level.

Refs: #711
