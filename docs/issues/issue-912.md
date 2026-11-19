---
type: issue
state: open
created: 2026-11-19T05:25:21Z
updated: 2026-11-19T05:25:21Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/912
comments: 0
labels: feature, priority:high, semver:minor, area:io
assignees: none
milestone: none
projects: none
parent: 897
children: none
synced: 2026-11-19T09:52:04.860Z
---

# [Issue 912]: [feat(io): writers emit the JCS form of every sealed JSON document (ADR-0066 step 2)](https://github.com/vig-os/tessera/issues/912)

Part of the ADR-0066 implementation plan (docs/adr/0066-seal-encoding.md, merged in #900; epic #897). **Step 2 of 9.**

**Goal:** every door that writes `manifest.json` or `collection.json` writes exactly `JCS(object)`, still with the in-band `manifest_hash` key. Readers do not change.

- **Crates:** `tessera-io` (`pack`, `pack_dir`, the exploded form, `WriteSession`, repository `objects/`), `tessera-cli` (`commit`/`publish`/`seal` and the collection writer), `tessera-ingest` (`collection.json`), `tessera-py`.
- **Depends on:** nothing.
- **Acceptance:**
  - a test per door asserts `bytes == JCS(parse(bytes))`;
  - the 13 tracked `.tsra` (12 distinct manifests) are re-packed with `corpus.json` unchanged;
  - the reference reader's `run.py` passes unchanged;
  - docs and snapshots that print `manifest.json` show the single-line form, with `jq .` as the readable view.

