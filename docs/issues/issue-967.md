---
type: issue
state: open
created: 2026-11-24T18:22:22Z
updated: 2026-11-24T18:22:22Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/967
comments: 0
labels: chore, area:testing
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-25T10:51:50.213Z
---

# [Issue 967]: [nits from the #960 review (ksb test job)](https://github.com/vig-os/tessera/issues/967)

From the fresh review of PR #960 (head c4604386):
1. CONTRIBUTING.md:53 and CLAUDE.md say the wrapper takes "any commit … pushed or not". It doesn't: `ksbTest` is read from the tested commit's own flake.nix, so a commit or branch older than #960 exits 3 (`attribute 'ksbTest' missing`). Say so in the docs.
2. ksb records any exit 124 as `reason: deadline`, so `ksb jobs` shows a test `--timeout` as if ksb's own time limit had killed the job. Either tell the two apart (a distinct exit code for the test timeout) or document how they overlap.
3. In the exit-3 output, the log tail has no trailing newline, so the last log line runs into the `ksb-test:` hint (`ksb-test.sh:484`).
Resource sizing (spec 4 CPU / 8–16 GiB vs `--cpu 1 --mem 1Gi` on the builder) is already tracked in #961.

Refs: #957
