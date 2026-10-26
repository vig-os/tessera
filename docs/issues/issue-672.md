---
type: issue
state: open
created: 2026-10-25T11:18:59Z
updated: 2026-10-25T11:18:59Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/672
comments: 0
labels: question, priority:medium, area:integrity
assignees: none
milestone: none
projects: none
parent: 654
children: none
synced: 2026-10-26T07:24:19.540Z
---

# [Issue 672]: [question: integrity default for object-store query sources, VerifyBlocks vs SealOnly (ADR-0062)](https://github.com/vig-os/tessera/issues/672)

Open question from ADR-0062 §7 (#654), for the owner.

Should object-store sources default to `Integrity::VerifyBlocks` (download each touched block whole into a
spool, hash it, then scan) or to `SealOnly` (ranged reads of only the needed segments, payload bytes not
hashed)?

**Recommendation:** keep `VerifyBlocks` everywhere. A query never returns a row from bytes that don't match
the seal, and tier-1 pruning still avoids fetching blocks that can't match. **Cost:** a touched block is
fetched whole even for a narrow projection.

