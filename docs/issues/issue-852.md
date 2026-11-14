---
type: issue
state: open
created: 2026-11-14T01:17:58Z
updated: 2026-11-14T01:17:58Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/852
comments: 0
labels: documentation
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-14T08:34:32.882Z
---

# [Issue 852]: [docs: FEATURE-MATRIX honesty pass — every ✓ must name a CI gate that enforces it](https://github.com/vig-os/tessera/issues/852)

The 2026-11-14 review found ✓ rows that no gate enforces:
- §D says a perf-floor breach fails the build, but every budget in `tessera/perf-budgets.toml` is commented out (#637 is related).
- 'Independent reader ✓': `corpus/reference_reader/reader.py` checks hashes but decodes no payload, while ROADMAP's gate required decoding (#636 is related).
- 'PS3.15 de-id ✓': #579 shows it is a 12-tag list, not a PS3.15 profile.
- 'Writer determinism ✓': two identical `ingest table` runs gave different file sha256 (#824).

Do: add a 'gate' column that names the CI check or test for every ✓, and downgrade every row that has none to ◑ with its issue. 15 ADR index rows still say Proposed for built ADRs (e.g. 0026/0035/0036/0037), and the D7 'encryption non-goal' row contradicts Accepted ADR-0047: fix those statuses in the same pass, or put them to the owner.
