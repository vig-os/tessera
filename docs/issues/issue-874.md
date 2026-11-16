---
type: issue
state: open
created: 2026-11-15T16:24:55Z
updated: 2026-11-15T16:24:55Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/874
comments: 0
labels: bug
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-16T08:59:36.912Z
---

# [Issue 874]: [fix(cli): inspect warns about newer manifest fields and --json prints the stored object](https://github.com/vig-os/tessera/issues/874)

Follow-up to #573 (ADR-0060 §2.2 / §11 P0a, the split the ADR allows). The #573 fix makes `tessera verify` warn
`warning: fields from a newer format revision are not interpreted: <paths>` (from `Manifest::newer_fields`).
Still missing:

- `tessera inspect` prints the same warning.
- `tessera inspect --json` prints the **parsed manifest object** (`Reader::manifest_json`), not the typed view's
  re-serialisation, so it is exactly the manifest as stored (the promise at `tessera-cli/src/main.rs` ~225-234):
  today it drops newer fields and normalises `"study": null` / an omitted `sources`.

Acceptance: a trycmd case over a product with a newer field (e.g. built from `corpus/seal-cases.json`
`empty_no_blocks`-style resealed manifest) shows the warning, and `inspect --json` round-trips the stored bytes.

