---
type: issue
state: open
created: 2026-11-19T05:25:27Z
updated: 2026-11-19T05:25:27Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/916
comments: 0
labels: chore, priority:high, area:testing
assignees: none
milestone: none
projects: none
parent: 897
children: none
synced: 2026-11-19T09:52:02.720Z
---

# [Issue 916]: [chore(corpus): final corpus + conformance cross-check after the format break (ADR-0066 step 8)](https://github.com/vig-os/tessera/issues/916)

Part of the ADR-0066 implementation plan (docs/adr/0066-seal-encoding.md, merged in #900; epic #897). **Step 8 of 9: regenerate the corpus and conformance data once more, as the final cross-check.**

- **Crates:** `corpus/`, the `tessera-cli` snapshots, the reference reader, `tessera-wasm`.
- **Depends on:** #895 (step 1), #720 (step 5) and #828 (step 7).
- **Scope:** steps 1, 5 and 7 each regenerate the goldens they move, in their own PRs. This step re-runs the whole corpus as a final check:
  - any remaining difference is a defect;
  - every moved hash (`content_hash`, `manifest_hash`, block digests) is traced to step 1, 5 or 7;
  - `id` moves only where #895 touched a field in `id_inputs`.
- **Acceptance:** a per-product table of which hash moved and why; Rust, the reference reader and `tessera-wasm` agree on the new corpus.

