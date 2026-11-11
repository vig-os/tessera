---
type: issue
state: open
created: 2026-11-11T04:18:25Z
updated: 2026-11-11T04:18:25Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/831
comments: 0
labels: feature, area:io
assignees: none
milestone: none
projects: none
parent: 799
children: none
synced: 2026-11-11T08:50:27.433Z
---

# [Issue 831]: [feature(io): StreamWriter finish_with (header patch at seal) and abort (ADR-0065 §3)](https://github.com/vig-os/tessera/issues/831)

Part of epic #799. Design: ADR-0065 (docs/adr/0065-c-abi-bridge.md, PR #819). **Blocked until the owner accepts ADR-0065**; do not start before. Toward #800 (split from #810).

## What
- `StreamWriter::finish_with(out, header)`: once `StreamWriter::new` holds the session (`stream.rs:746`, `:757`; owned by `commit_loop`, `:1144`) no header setter is reachable, so `finish_with` applies a header patch (study, fields, extras, sources, producer, generation, schema, ingest transforms) after the last commit and before the validated seal (#810).
- `StreamWriter::abort()`: close the input, join every worker and the committer, return without sealing (today only `finish` joins the threads, `stream.rs:1112-1133`; there is no `Drop`).

## Acceptance
- Metadata applied through `finish_with` after the last push seals identically to the same metadata set before the first push.
- `abort` returns with every thread joined while a job is mid-encode, and leaves no output.

Size: ≤ ~250 lines. Depends on: #810.

