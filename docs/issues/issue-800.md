---
type: issue
state: open
created: 2026-11-11T01:02:41Z
updated: 2026-11-11T01:10:08Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/800
comments: 1
labels: feature
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-11T08:50:42.491Z
---

# [Issue 800]: [feature(capi): C ABI bridge over the tessera-io writer (Arrow C Data Interface), ADR first](https://github.com/vig-os/tessera/issues/800)

Sub-issue 1 of #799. ADR first, then implementation; cbindgen header, static+shared lib, pkg-config/CMake, nix + cargo-dist packaging, no panics across FFI, ABI version.
---

# [Comment #1]() by [gerchowl]()

_Posted on November 11, 2026 at 01:10 AM_

## Owner design steer (2026-11-11), for tessera-800-adr and tessera-800-spike: fold it in, then continue your round
Owner: "a C interface that takes Arrow batches from many threads and streams them to StreamWriter". Architect refinement,
which the ADR states as the proposed design. It stays Proposed until the owner accepts it.
1. **Wire = Arrow C Data Interface** (in-process, zero-copy). Our table path already encodes Arrow → Vortex, so Arrow at the
   boundary adds no conversion, and Vortex never appears in the C API, so storage can change without breaking C++ callers.
   Arrow IPC = a later option for producers in another process. Per-product C structs are rejected (an ABI per schema).
2. **Concurrency:**
   - One session per product. `push(session, seq, ArrowArray*, ArrowSchema*)` is thread-safe, and any number of threads
     may call it at once.
   - The bridge streams into StreamWriter: parallel encode, **commit in `seq` order**, behind the bounded credit window,
     which back-pressures (blocks) a thread that runs too far ahead. Many products = many independent sessions.
     Multiple processes writing one product = later (exploded layout); out of scope.
3. **Determinism comes from `seq`, not from threads.** Geant4 MT assigns events to threads dynamically, but event seeds are
   per-event, so the producer derives `seq` from the event, e.g. the event-block index.
   - `seq` must be dense: an empty block pushes an empty batch or calls `mark_done(seq)`.
   - Define the behaviour for duplicate, missing-at-seal and too-far-ahead seq (errors, not hangs).
   - The sealed bytes depend only on the (seq → batch) mapping: same mapping, same bytes, for every thread count and scheduling.
4. **Schema:** declared at session open (from the registry, #801). Every batch is validated against it (names, types,
   nullability) at the boundary, with a clear error.
5. **Verify on the real code:** does StreamWriter accept jobs out of order with an explicit sequence today, or only in push
   order? If not, the ADR/prototype states the minimal extension: reuse the in-order commit and credit machinery; never
   write a second scheduler.
   - Spike: the N-thread test pushes in **shuffled seq order** from N ∈ {1, 4, 16} threads and asserts an identical
     content_hash. It also shows the window bound holds (peak in-flight ≤ window) and that a gap at seal is an error.
Do not restart. Fold this into your current round; same receipt, same last-line rule.


