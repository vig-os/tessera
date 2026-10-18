---
type: issue
state: open
created: 2026-10-18T00:55:10Z
updated: 2026-10-18T00:55:10Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/594
comments: 0
labels: documentation
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-18T06:56:49.204Z
---

# [Issue 594]: [docs(py): Builder config keys are sorted by UTF-8 bytes, not JCS (UTF-16) order as documented](https://github.com/vig-os/tessera/issues/594)

Follow-up from the fresh review of PR #585 (head c7faab5), a nit. `config_to_json`'s docs, the sort_by comment, the #562 commit message and bindings.md say nested config keys come back in 'canonical (JCS) order'. The code sorts by UTF-8 bytes (and the top-level BTreeMap already did before #585). RFC 8785 JCS sorts by UTF-16 code units, so the two orders differ for keys mixing characters ≥ U+E000 with characters beyond the BMP. The seal isn't affected (serde_jcs canonicalises independently); only the documented order is wrong. Fix the wording (say 'sorted by UTF-8 byte order; the seal uses JCS independently'), and note that the 125-level nesting cap applies to tuples too (they're treated as lists).

Refs: #562
