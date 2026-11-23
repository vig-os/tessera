---
type: issue
state: open
created: 2026-11-22T22:14:02Z
updated: 2026-11-22T22:14:02Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/959
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-23T10:16:26.954Z
---

# [Issue 959]: [ci(lint): the pymarkdown hook runs in fix mode and never fails on line length or MD018](https://github.com/vig-os/tessera/issues/959)

## Defect

The `pymarkdown` hook in `.pre-commit-config.yaml` runs `pymarkdown -c .pymarkdown fix`, not `scan`. Fix mode rewrites
what it can and exits 0 when the only violations left are ones it cannot fix: MD013 (line length, including headings
and code blocks) and MD018 among them. So no commit, push or CI run (`Lint & Format`) ever fails on them, and the
120-column limit the repo's config sets (`md013.line_length: 120`) is not enforced.

Found while reviewing PR #958. The hook does cover `docs/` (`types: [markdown]`, excluding only the three root files),
but the violations below passed CI's `Lint & Format`.

Evidence (`dev` 342bf645, from the repo root inside `nix develop`):

```sh
pymarkdown -c .pymarkdown scan docs/adr/0066-seal-encoding.md   # rc 1: 4 x MD018
pymarkdown -c .pymarkdown scan docs/spikes/915-tessera-table-v2.md   # rc 1: 34 violations (MD013, MD018)
cp docs/adr/0066-seal-encoding.md /tmp/x.md
pymarkdown -c .pymarkdown fix /tmp/x.md   # rc 0 on a file with a 175-char line (PR #958's first head)
```

`.pymarkdown.config.md` also documents a 210-character MD013 limit while `.pymarkdown` sets 120.

## Ask

- Run the lint in `scan` mode (or `fix` and then `scan`) so that a violation fails.
- Decide what to do with the violations already on `dev` before the gate turns red: fix them, or baseline them
  explicitly. A full `pymarkdown scan` over `docs/` shows how many.
- Bring `.pymarkdown.config.md` in line with `.pymarkdown`.

Not changed in #958 (owner instruction: no hook config change there).

## Changelog Category

Fixed

