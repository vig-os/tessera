---
type: issue
state: closed
created: 2026-02-12T16:39:35Z
updated: 2026-09-28T17:53:14Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/1
comments: 3
labels: question
assignees: irenecortinovis, c-vigo
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:53:14.623Z
---

# [Issue 1]: [Review fd5](https://github.com/vig-os/tessera/issues/1)

Could you review the whitepaper, and, e.g. comments from datalad perspective, and the point of device_data / metrics that would get ingested from prometheus?

Creation of an ro-crate or other stuff should be 'easy' as the hdf5 attr should hold all relevant data.

This came out of frustration with the dicom mess from the scanners.
Want sth. that i can ingest the scanner-crap to that is actually usable and then store the sh*t in a 'junk' folder, push it to backup & forget it ever existed.
---

# [Comment #1]() by [c-vigo]()

_Posted on February 16, 2026 at 09:57 AM_

Add support to sign files (both optionally and mandatory)

---

# [Comment #2]() by [c-vigo]()

_Posted on March 2, 2026 at 04:21 PM_

### Comments

- [Additive schema](https://github.com/vig-os/fd5/blob/0901bb16b7c9ff7335974a0c7fa6d5bd74d24272/white-paper.md#L126-L128). Add a `deprecated` flag to any field so that users get a warning.
- [Products without lifetime](https://github.com/vig-os/fd5/blob/0901bb16b7c9ff7335974a0c7fa6d5bd74d24272/white-paper.md#L220). Should this be allowed? How can a product not have at least a creation date, i.e. the moment you execute the software that generates the `fd5`?
- [What manifest?](https://github.com/vig-os/fd5/blob/0901bb16b7c9ff7335974a0c7fa6d5bd74d24272/white-paper.md#L428)  I see now that manifest is explained later, but at this point it is not clear what this means.

---

# [Comment #3]() by [gerchowl]()

_Posted on September 28, 2026 at 05:53 PM_

Closing as **obsolete** (2026-09-28 backlog triage).

Evidence: fd5 archived at tag fd5-archive; Tessera superseded fd5 (commit e805d56 'Tessera supersedes fd5 — graduate spike/tessera-core to dev' #322).

https://claude.ai/code/session_01XdERKMVDAwfMJSKdTytNnK

