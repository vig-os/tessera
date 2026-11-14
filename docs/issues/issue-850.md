---
type: issue
state: open
created: 2026-11-14T01:17:55Z
updated: 2026-11-14T01:17:55Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/850
comments: 0
labels: bug
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-14T08:34:33.785Z
---

# [Issue 850]: [cli: `tessera read` prints a NULL in a nullable f8 column as nan, and `tree` hides nullability](https://github.com/vig-os/tessera/issues/850)

From the 2026-11-14 review, verified on the alpha.5 binary: a table declared with a nullable `f8?` column reads back NULLs as `nan` in `tessera read`, so a missing value and a stored NaN look identical. `tree` doesn't show which columns are nullable.

Related: #804 (the Python nullable read), the same class of bug on the Python side.

Acceptance: `read` renders NULL distinctly from NaN, with tests for both on f8, i8 and a string column, and `tree`/`ls` show nullability.
