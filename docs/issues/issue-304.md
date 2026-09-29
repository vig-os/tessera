---
type: issue
state: open
created: 2026-07-02T14:26:30Z
updated: 2026-09-28T17:59:15Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/304
comments: 2
labels: feature, discussion
assignees: none
milestone: 0.1.0-beta
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:51.693Z
---

# [Issue 304]: [feat: assemble a collection from pre-sealed .tsra (or collection-as-query over OCI by exam-id)](https://github.com/vig-os/tessera/issues/304)

## Context — first-user DUPLET FAIR shakedown

Today a collection (`collection.json`) can **only** be minted by `ingest --spec` — there is no verb to
assemble a collection from **already-sealed** `.tsra` members. That forces the whole acquisition
through one atomic spec run (see #302) and couples "which members belong together" to
ingest time.

## Proposal (from the DP01 run)

Decouple membership from ingest. Ingest members individually (resilient), tag each with
`study` / `exam` / `series` metadata, push to the OCI registry, and let **"the collection" be a query**
over the registry catalog / member metadata by **exam-id** — assembled on demand, not pre-baked. This
also fits recursive collections (#293) and the collection-as-query direction.

## Ask

- `tessera collection new/add` (or similar) to assemble a `collection.json` from a set of existing
  `.tsra` (MMR over members, as the spec engine already does internally), **or**
- a documented **collection-as-query** path: catalog/metadata query by `exam`/`study` over OCI (see
  #291 oci-client, #272 collection consumer verbs).

Found during: DUPLET first-user FAIR ingest (DP01).

---

# [Comment #1]() by [gerchowl]()

_Posted on July 2, 2026 at 03:58 PM_

Reviewed — **not superseded; this is exactly the authoring/curation half of collections**, and it's now well-supported by the just-landed building blocks. It was the gap flagged in the collection-architecture discussion ("there's no `tessera collection create/add/from-query`" — the model + consumer verbs shipped, the *producer* verb didn't). Folding it into the landed architecture:

**Part 1 — `tessera collection new/add` from pre-sealed `.tsra` — a thin CLI over primitives that now exist:**
- `CollectionBuilder` + typed `ProductHandle::of(&sealed)` / `CollectionHandle::of(&sealed)` + `add_product` / `add_subcollection` (#306, merged) — carry `(reference, manifest_hash)` as one unit, so you can't mismatch; MMR-over-members exactly as the spec engine does internally.
- `CollectionSchema` (#294) — the assembled collection **declares its level** (`--schema dataset|project|…`), validated at seal (a `dataset` refuses a sub-collection member, etc.).
- Recursive `verify`/`inspect`/`ls` (#309) consume it directly.

So `tessera collection new --schema dataset --member a.tsra --member b.tsra -o collection.json` is a small, well-scoped verb over the landed model. This is the piece I'd build next — it also **structurally fixes #302** (decouple membership from ingest → resilient per-member ingest, assemble after).

**Part 2 — collection-as-query over OCI by exam-id** — the bigger half, ties to **#291** (oci-client: tags/index/referrers as the registry catalog) + **#292** (ownership boundary: the *registry is the SSoT for pointers*, and a "query-materialized cohort" = a persisted collection snapshot, not a live view). That's the "the collection is a query" direction; it lands on the OCI transport work.

Keeping open as the **next collection build** (Part 1 now on the landed model; Part 2 with #291). Not superseded.

---

# [Comment #2]() by [gerchowl]()

_Posted on July 2, 2026 at 06:13 PM_

**Part 1 landed** — `tessera collection new` (PR #321, merged to spike): assemble a `collection.json` from pre-sealed `.tsra` products → a self-contained `<dir>/collection.json` + `<id>.tsra` members that `collection verify` checks in place. Declared `--schema` (dataset/project/…) validates the member_rule at seal. This decouples membership from ingest and structurally defuses #302.

**Part 2 still open:** collection-as-query over OCI by exam-id — rides #291 (oci-client: the registry catalog as the query surface). Also a small follow-up: sub-collection members (assemble a `project` from child collection.jsons) on the same seam.

