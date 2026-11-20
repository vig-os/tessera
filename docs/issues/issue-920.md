---
type: issue
state: open
created: 2026-11-19T11:33:54Z
updated: 2026-11-19T11:33:54Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/920
comments: 0
labels: bug, priority:medium, area:core
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-20T09:39:10.994Z
---

# [Issue 920]: [fix(cli): --set/--meta text that looks like JSON but doesn't parse is stored as a string ('[1,NaN]')](https://github.com/vig-os/tessera/issues/920)

Found by the fresh re-review of PR #894 (round 2, head 23ce61c).

The CLI `--set`/`--meta` rule is: JSON first; otherwise numeric intent if `str::parse::<f64>` accepts the text; otherwise a bare string. Under that rule `--meta 'k=[1,NaN]'` seals the **string** `"[1,NaN]"` with rc=0. The user meant an array, and a non-finite element inside it gets silently stored as text. This is the same class of bug as the `Infinity`→string case that #894 fixes.

**Proposal:** treat text that starts with `[` or `{` as JSON intent. If it does not parse as JSON, refuse it with the parse error (naming the flag and the key) and with the quoted-string escape (`k='"[1,NaN]"'`). Bare text that starts with any other character stays a string.

**Acceptance:**
- `[1,NaN]` and `{"a":` are refused, rc≠0, with nothing written;
- `'"[1,NaN]"'` is stored as a string;
- `[1,2]` is stored as an array;
- the `--set` and `--meta` help text and the docs state the rule.

Refs #886.

