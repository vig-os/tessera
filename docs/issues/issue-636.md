---
type: issue
state: open
created: 2026-10-19T00:00:03Z
updated: 2026-10-19T00:00:03Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/636
comments: 0
labels: feature, priority:high, area:testing, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-19T07:17:40.937Z
---

# [Issue 636]: [test(io): backward-read value goldens for committed .tsra files (catch a dependency that mis-decodes old products)](https://github.com/vig-os/tessera/issues/636)

Part of #635 (vortex upgrade path): prerequisite for a meaningful bump canary.

**Gap (mapped 2026-10-19):** no test decodes committed `.tsra` files and compares **values**. `conformance.rs:66`
(`committed_corpus_files_reproduce_goldens`) checks the golden triple and block digests only; `tessera-py/tests/smoke.py:58-71`
checks types, row counts and projection == full read, with no expected values. So a dependency bump that mis-decodes OLD files passes CI.
The committed files hold only two simple tables (`listmode_events` u64/f32, `multiblock_study` u32/f32); ingest goldens
(`corpus/ingest-corpus.json`) are hashes with no committed files, so "a hash moved" can't be split into "encoding moved" vs "values moved".

**Ask:**
1. Commit a backward-read corpus of `.tsra` files written by today's pinned Vortex, covering: f32 AND f64 NaN with a custom payload, ±inf, -0,
   subnormals; nullable columns with nulls (start/middle/end, all-null); plain strings (FSST) and dictionary; bool; several row groups;
   several table blocks (multi-block); the existing array blocks.
2. A test that decodes every committed file with the CURRENT build and compares each column to an expected-values digest
   (e.g. blake3 over the canonical Arrow IPC or the `to_bits` LE bytes plus the validity bitmap), committed next to the file. A value change fails
   it even when the bytes on disk are unchanged.
3. Run it in the flake on both arches. A new Vortex must read every old file to identical values; that is the read-compatibility promise.

