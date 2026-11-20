---
type: issue
state: open
created: 2026-11-18T21:36:11Z
updated: 2026-11-19T19:29:54Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/910
comments: 4
labels: feature, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-20T09:39:13.351Z
---

# [Issue 910]: [feat(io): read API over committed-but-unsealed fragments of an open WriteSession — live snapshot at watermark W (kit D32)](https://github.com/vig-os/tessera/issues/910)

Context: kit open question **D32** (nerdmachines/kit `docs/open-questions.md`, ADR 0010 §4) asks tessera for a read API over the **committed-but-unsealed** fragments of an open `WriteSession`. #795 lists the same gap as an open question ("the read API for a live puller over an open stage dir"). Filed 2026-11-18 because no issue tracked it. The kit side is still **proposed** (ADR 0010 is not yet accepted), so this is a request to design against, not a commitment.

## Problem

A live query over a running acquisition is a union of three partitions, split at the writer's committed watermark `W`:
- the sealed product;
- the committed-but-unsealed fragments `[sealed_end, W)`;
- the in-memory ring `[W, H)`, which stays kit-side.

tessera reads only **sealed** products (the ADR-0062 provider). The middle partition has no read API. Without it, the live edge is either RAM-only (bounded by the ring) or a whole session in RAM, and a pan from "ten minutes ago" to "now" has a gap at the seal boundary.

## Proposal (to shape, not prescribe)

- **`WriteSession::committed_fragments(range) -> Vec<BlockRef>`,** or a snapshot handle, giving the fragments committed in `[from, W)`. It is safe to read while the writer keeps appending; journal-durable only.
- **A provider over that snapshot** that plugs into ADR-0062's scan machinery (zone maps, `.cidx` pruning), so the same query shape covers sealed and committed-unsealed rows.
- **A snapshot is immutable for the query's lifetime.** Compaction and seal must not delete fragments a live snapshot holds (refcount, or defer the deletion).
- **Integrity:** each fragment verifies against its born digest. With #795, it also verifies against the newest signed checkpoint.

## Acceptance

- [ ] A query over sealed + committed-unsealed, taken at snapshot `W`, returns exactly `[0, W)`, disjoint by row index, while the writer appends and compacts concurrently (property test).
- [ ] Seal or compaction during a live snapshot never removes bytes the snapshot reads.
- [ ] Zone-map pruning works on committed-unsealed fragments as on sealed blocks.

Related: #203, #654 (ADR-0062), #794, #795, #907, #908, #909; kit ADR 0010 §4, D32, D36 (decided with D32).

---

# [Comment #1]() by [gerchowl]()

_Posted on November 18, 2026 at 11:55 PM_

## Extension request from kit #139 (spike round 1, 2026-11-19)

**kit #139** proposes a scan-fragment read-plane protocol (Flight `DoGet` shape) that crosses a **process boundary** between the driver host and kit-query. The acceptance in #139 therefore needs more than the in-process snapshot this issue already names: it needs **fragment handles that keep committed-but-unsealed fragments alive across processes while the kit-query reader holds them**, with a **bounded retention lease measured in bytes**.

### The scenario #139 cannot pass without this

- `kit-query` (process B) opens a live query over a driver host (process A). At plan time the driver host returns a `PartitionSnapshot { epoch, sealed_set, unsealed_set, W, L, H, checkpoint_W }` that names immutable fragment identities for the committed-but-unsealed range.
- Between plan and open, process A may seal (or compact) fragments that the unsealed_set references. On the same host this can be prevented by handing process B **file descriptors** at plan time; across hosts the driver host must serve the fragment bytes over Flight without them disappearing underneath. Either way, the writer must not block, and the fragment bytes referenced by the snapshot must survive until B releases the lease or the lease expires by byte budget.
- The kit side bounds the lease in bytes, not seconds (so a stalled reader cannot pin unbounded disk), and pre-empts it; the tessera side needs a matching contract on its fragment lifetime.

This is strictly an extension of this issue's acceptance item "Seal or compaction during a live snapshot never removes bytes the snapshot reads", lifted from one process to many. All other items (zone-map pruning on committed-unsealed, integrity against the born digest and the newest signed checkpoint via #795, plug-in to the ADR-0062 provider) remain as filed.

### Shape to design against (not prescribed)

- A **fragment handle** returned alongside `WriteSession::committed_fragments(range)` that is transferable (an fd on the same host; an opaque identity across hosts) and refcounts the underlying bytes. Closing or expiring the handle releases the refcount; the writer is free to seal/compact once no handle references the fragment.
- A **retention lease** granted to a named reader principal, with a byte budget and an inactivity bound. The lease is pre-empted when the budget is exhausted; pre-empted readers see a typed expiration, not corrupt bytes.
- A **safety rule**: the writer never blocks on a reader. Pre-emption is on the reader, never the writer.

### Owner decision context on the kit side

Owner decision 2 (2026-11-19) names **tessera #908 (ingress tap) as the kit's raw out-of-process path**; there is no second shared-memory mechanism in kit. The read-plane scan-fragment protocol in #139 is the structured path and depends on this extension. Owner decision 3 (2026-11-19) caps the kit-side read lease at ≤ 5 s TTL, which bounds how long a tessera retention lease has to hold.

### Acceptance (to add, not replace)

- [ ] A reader in another process holds a fragment handle across a seal attempt; the writer does not block; the handle still reads the bytes it was promised; releasing the handle lets the next seal proceed.
- [ ] A byte-bounded retention lease is pre-empted when its budget is exhausted; the reader receives a typed expiration (never a torn read).
- [ ] The same guarantees hold across Flight (host A writer, host B reader), subject to the transport's own liveness.

Related: kit #139 (round-1 revision, 2026-11-19), kit ADR 0010 §4, kit D32; this issue and tessera #908, #795. The kit side is still **proposed**.


---

# [Comment #2]() by [gerchowl]()

_Posted on November 19, 2026 at 12:24 AM_

## Correction to the 2026-11-18 extension request (kit #139 spike round 2)

Round 2 of the kit spike (Fable + astra) found the earlier comment conflated two distinct concerns. The
extension request stands in substance, but with these corrections:

1. **Authorization and retention are separate leases, not one.**
   - **Read-authorization lease** (kit #139, owner decision 3): a short TTL (≤ 5 s), renewable, bound per data
     class, revoked by expiry only (no deny list); patient-linked data fails closed on expiry.
   - **Fragment-retention lease** (this issue): a byte budget plus an inactivity bound against which the
     underlying tessera fragments are refcounted. The two leases have independent accounting and independent
     typed expirations.

2. **Seal and compaction always proceed; only reclamation waits.** The writer never blocks on a reader (as
   already stated). Seal and compaction are free to run at any time; what waits on the retention lease is the
   physical reclamation of the fragment's bytes. A reader holding a live fragment handle keeps the bytes
   readable past a seal; releasing or expiring the handle lets reclamation complete. The earlier "releasing
   handles lets the next seal proceed" line read as if a handle blocks seal — it does not; it defers reclamation.

3. **Aggregate byte limits, inactivity reclamation, typed expiry.**
   - The byte budget is aggregate per reader principal (not per capability), so many overlapping handles cannot
     push unbounded disk.
   - An inactivity timer reclaims an idle handle even if it has budget left.
   - Expiration is a **typed error** returned to the reader (never a torn read): the reader sees
     `FragmentLeaseExpired` with cause (`budget_exhausted` | `inactivity` | `writer_eviction`) and re-plans.

4. **Trusted-reader fd behaviour.** On the same host the writer passes fds to a trusted reader process; closing
   the fd at the writer side does not revoke a reader's existing mapping (an Linux fact that applies equally
   here). Revocation of raw access is by **fragment rotation** (the writer stops referencing the old fd in new
   snapshots), not by fd close. A stalled or crashed trusted reader must therefore be reclaimed by the retention
   lease's inactivity bound; the writer-side fd stays closable at any time without affecting the reader.

5. **Tests for stalled and crashed readers.** Add explicit acceptance:
   - [ ] A reader that stops reading and does not renew its retention lease is reclaimed within the inactivity
     bound; its later read returns the typed expiration above, not a torn page.
   - [ ] A reader that crashes while holding a handle is reclaimed by the inactivity bound (or an OS-side fd
     close detection, where cheap); fragments are released; the writer makes forward progress.
   - [ ] A seal landing while a live handle is held does not block the writer; the seal completes, reclamation
     is deferred until the handle releases or expires.

Everything else from the 2026-11-18 extension (shape of `WriteSession::committed_fragments(range)`, zone-map
pruning on committed-unsealed, integrity against born digest + newest signed checkpoint via #795, plug-in to
the ADR-0062 provider) stands.

Links: kit #139 (round-2 revision 2026-11-19), kit ADR 0010 §4, kit D32; this issue and tessera #908, #795.
The kit side is still **proposed**.


---

# [Comment #3]() by [gerchowl]()

_Posted on November 19, 2026 at 12:44 AM_

**2026-11-19 (kit spike round 3). Corrects the 2026-11-19 round-2 correction above.**

Three substantive corrections (fragment retention and the decision-2 sentence) and one date fix.

1. **Fragment-retention lease, physical reclamation.** Round-2 said a stalled reader is "reclaimed by the inactivity bound". Bytes behind a passed fd (same-host) or mapped regions cannot be freed while the reader holds them open. The correct statement is:

   - **Lease expiry stops new handles and accounting** (no new fragment handles are issued and no new bytes count against the budget);
   - **Physical reclamation completes when the reader closes its fds or exits**; **retained bytes stay charged to the reader principal until then**;
   - **Closure is cooperative** — a trusted reader is expected to close on `FragmentLeaseExpired`; a trusted reader that has not closed by the inactivity bound is terminated by the supervisor (process termination) so the writer can make forward progress.

   Acceptance: "a reader that stops reading and does not renew its retention lease is reclaimed within the inactivity bound" is kept; its mechanism is "the lease stops issuing new handles and accounting at expiry; the fds / mappings held by the reader are released when the reader closes them or is terminated at the inactivity bound".

2. **"No second shared-memory mechanism" sentence withdrawn.** The 2026-11-19 extension comment above says "there is no second shared-memory mechanism in kit". Round 2 of the kit spike corrected this on kit's side without retracting the sentence here. **The sentence is withdrawn.** The correct statement is: **memfd is for trusted kit processes only (owner decision 2, 2026-11-19); tessera #908 is the raw out-of-process path; every other reader goes through kit-query in the core.** A raw memfd ring is therefore still a shared-memory mechanism on the kit side, but restricted to trusted processes and measured against the Unix-socket scan-fragment baseline on kit #139 before promotion.

3. **Dates.** The 2026-11-19 round-2 correction above is correctly dated. The 2026-11-19 extension comment just above is also 2026-11-19 (no 2026-11-18 marker). The cross-references in kit #78, kit #139 Related and the docs PR body that say "2026-11-18 extension / comment" are being corrected on kit's side — the originals are **2026-11-19**.

Everything else from the 2026-11-19 extension and the 2026-11-19 round-2 correction (shape of `WriteSession::committed_fragments(range)`, zone-map pruning on committed-unsealed, integrity against born digest + newest signed checkpoint via #795, plug-in to the ADR-0062 provider, aggregate byte limits per reader principal, typed `FragmentLeaseExpired` with cause, seal and compaction always proceed, fragment rotation revokes raw access on new snapshots) stands.

Links: kit #139 (round-3 revision 2026-11-19), kit ADR 0010 §4, kit D32; this issue and tessera #908, #795. The kit side is still **proposed**.


---

# [Comment #4]() by [gerchowl]()

_Posted on November 19, 2026 at 07:29 PM_

**Correction 2026-11-19 (nerdmachines/kit#116, closing check of kit #139):** this corrects two phrases in [the round-3 follow-up](https://github.com/vig-os/tessera/issues/910#issuecomment-6996072010). The earlier text stays as it was, as evidence.

1. **"Fragment rotation revokes raw access on new snapshots"** is withdrawn. Segment or fragment rotation **isolates future bytes only**: the writer stops publishing to the old segment, so a reader holding it sees no new rows there. Rotation revokes nothing. The only revocation of the right to receive bytes is expiry of kit's read-authorization lease, and bytes already disclosed or mapped cannot be recalled.
2. **"Lease expiry stops new handles and accounting"** should read **"stops new handles and new allocations"**. After expiry, no new fragment handles are issued and no new bytes are allocated against the reader's budget. Bytes already retained **stay charged** to the reader principal until it closes its fds or exits, as the next bullet in that comment already says.

Nothing else in the request changes.


