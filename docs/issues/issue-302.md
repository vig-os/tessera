---
type: issue
state: closed
created: 2026-07-02T14:26:28Z
updated: 2026-09-28T17:54:52Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/302
comments: 2
labels: bug, priority:medium, effort:medium
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:52.406Z
---

# [Issue 302]: [bug(ingest): --spec collection engine is non-atomic — failing member leaves orphan .tsra and no collection.json](https://github.com/vig-os/tessera/issues/302)

## Context — first-user DUPLET FAIR shakedown

`tessera ingest --spec` (the collection engine) is **not atomic**. Running a 2-member DP01 spec where
member 1 (a blob) succeeded and member 2 (a DICOM series) failed left:

- an **orphan** `blake3_….tsra` (the successful member) written to `--out`,
- **no `collection.json`** (the run aborted before minting it),
- **no rollback / cleanup**.

```
error: invalid product: dicom: non-uniform series (shape/modality/rescale differ between slices)
# ... but _min/blake3_2fea….tsra (136 MB) was already on disk
```

## Impact

For real multi-member acquisitions (DP01 = 31 members), a single bad series aborts the whole
collection and leaves partial, unreferenced output. A custodian can't tell a complete collection from
a half-written one.

## Ask

Either (a) **transactional**: stage to a temp dir, mint `collection.json`, then atomically move; roll
back on any member failure; or (b) **resilient**: continue past a failing member, record it in
`collection.json` as skipped-with-reason, exit non-zero. (b) pairs well with the collection-as-query
model (#304).

Found during: DUPLET first-user FAIR ingest (DP01).

---

# [Comment #1]() by [gerchowl]()

_Posted on July 2, 2026 at 03:58 PM_

Reviewed against the just-landed collection work (#282 verbs · #306 recursion mechanism · #309 recursive verify · #294 CollectionSchema). **Not superseded — this is a live bug.** All of that is the *consumer* + *data-model* side; the `--spec` engine's *transactional* behavior (`tessera-ingest`) is untouched. Keeping open.

The landed model does **inform the fix**, though:

- **Detection is now better:** recursive `verify` (#309) fails loudly on a missing/incomplete member, so a consumer *can* tell a half-written collection from a complete one — but the producer still must not leave orphans.
- **The clean fix is (a) transactional:** the engine already builds the collection with `CollectionBuilder` at the end; stage members to a temp dir, `CollectionBuilder::seal()` → write `collection.json`, then atomically rename the whole prefix into `--out`; roll back (delete the temp) on any member failure. Reuses the landed builder; nothing new in core.
- **(b) resilient** (skip-with-reason, exit non-zero) is also viable and pairs with **#304** — which *structurally* dissolves this bug: if membership is decoupled from ingest, you ingest members individually (a bad series doesn't abort the run) and assemble the collection afterward from the survivors. So the best long-term answer is #304 + a transactional writer for the spec path.

Recommend: fix as **(a) transactional** for the `--spec` path now, and land **#304** so the atomic-spec run stops being the *only* way to get a collection.

---

# [Comment #2]() by [gerchowl]()

_Posted on September 28, 2026 at 05:54 PM_

Closing as **done** — verified on `origin/dev` in the 2026-09-28 backlog triage.

Evidence: commit 455af63 'fix(ingest): make the --spec collection engine atomic (#302) (#318)'.

https://claude.ai/code/session_01XdERKMVDAwfMJSKdTytNnK

