---
type: issue
state: open
created: 2026-11-15T16:03:59Z
updated: 2026-11-15T16:03:59Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/872
comments: 0
labels: area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-16T08:59:38.112Z
---

# [Issue 872]: [follow-up(#862): parallel verify reopens the path (TOCTOU); SPEC §6 'every zip reader' scope](https://github.com/vig-os/tessera/issues/872)

Nits from the round-3 adversarial review of PR #862 (head 38b6d61, merged as 02b8741).

1. **TOCTOU in parallel verify.** Above 256 MiB, the parallel verify checks the container layout on one file handle, and then each worker reopens the *path*. If the file is swapped in between, the new file is checked only against its own seal and never gets the layout check. Fix: have the workers reuse the checked handle (pread / `try_clone`), or re-check identity (dev+ino+size+mtime) before reading. Add a test.
2. **SPEC §6 scope.** The text says "every zip reader". Spell out that this covers readers that follow the central directory, and seekable or streaming local-header readers. It does not cover recovery tools that scan for signatures (`zip -FF` style), which can find a zip embedded inside a sealed payload (case e1 in the review: every normal reader shows the real manifest).

Untested in the review: Java `ZipInputStream` / `jar`, Node streaming unzip libraries.

Refs: #857
