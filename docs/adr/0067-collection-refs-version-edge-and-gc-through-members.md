# ADR-0067 — Collection refs, a collection version edge, and gc through members

Status: **Proposed** (2026-11-25) · Issue #947 · Extends **ADR-0036** (the repository, as amended on 2026-11-24 by
#946) and **ADR-0049** (recursive collections); touches SPEC §4 and §8. Relates ADR-0022 (the version DAG), ADR-0033
and ADR-0050 (collections, level schemas), ADR-0060 §2.2 (forward reading), ADR-0066 (stored bytes). Feeds the #781
ADR (keyed deltas, the `current` collection, compaction), #906 (retention) and #292 (the ownership boundary).

## Why a new ADR and not a dated ADR-0036 amendment

The change has two halves: a **format** change to collections (a sealed edge list, D2), which is ADR-0049's and SPEC
§4's territory, and **repository** semantics (refs, commit, gc, pins), which are ADR-0036's. A dated amendment to
ADR-0036 would bury a seal-affecting format change inside a store ADR that is itself still Proposed, and its two
halves could not be accepted or revised apart. So this is its own ADR, and it **extends ADR-0036's one versioning
story** rather than starting a second one (the owner's ruling of 2026-11-09 on #781): the same objects, the same
refs, the same compare-and-set under the same fenced writer, the same `gc`. On acceptance, ADR-0036 and ADR-0049 get a
one-line "Amended by ADR-0067" pointer and the branch sentence of ADR-0036 is restated as D9 says (step 7).

## Context

kit needs **workspaces**: a person or an agent edits a private copy of a keyed collection and the result is merged
back. The owner decided on 2026-11-21 that branches live in kit: tessera keeps **no branches and no content merge**.
With that split, a workspace is **a second lineage of `current` collections** (#781) whose first version pins the
same base and deltas as the main lineage at the fork; fork, conflict check and merge are kit code, and a merge
writes one new delta on the main lineage.

What blocks that today (read at `d24c1f9a`, the alpha.7 release commit, which contains #946's fix as `ca751291`):

1. **A ref cannot name a collection.** `Repository` stores product manifests; `commit` takes a `Manifest`;
   `get_manifest` parses one. The word "collection" does not occur in `tessera-io/src/repo.rs`.
2. **A collection has no version edge.** `Collection` (`tessera-core/src/collection.rs`) has `id`, `id_inputs`,
   ordered `members` pinned by `manifest_hash`, `content_hash` (the kind-tagged MMR over members, ADR-0049 §2) and
   the seal; nothing names the version it replaces or the lineage it was forked from.
3. **`gc` does not mark through members.** It marks a manifest's `blocks` and follows its `sources` edges. A base
   and deltas that only a `current` collection pins would be reclaimed. Worse, a collection object reached by the
   walk is *manifest-shaped* (it has `tessera_version`), so `walk_read` reads it as a product manifest, which fails
   (a collection has no top-level `product`), and `gc` aborts: the #946 rule turns the gap into a refusal, not a loss.
4. **No durable pin.** A three-way merge in kit needs the common ancestor; for long-lived variants that is an old
   version of either lineage, which must outlive compactions (#947, second comment, item 1).

Two facts about the code that constrain every option below:

- **`all_ref_tips` reads only plain files directly under `refs/`** (`path.is_file()`). An alpha.7 `gc` therefore
  ignores any ref stored in a sub-directory, and would sweep everything only such a ref reaches.
- **`parse_ref` accepts exactly `blake3:<64 hex>`** (trailing whitespace allowed) and reports anything else as
  `Error::CorruptRef`; `gc` turns that into `Error::GcAborted` before it deletes anything. Any new ref *content* is
  therefore refused by an alpha.7 binary, on every verb that reads it, which is the fail-closed behaviour we want.

## Decisions

Each decision lists its options with their honest costs; exactly one is **(Recommended)**: the one that is correct
in the long run, not the cheapest.

### D1 — How a ref names a collection

- **A. One namespace, an untyped address; the object says its kind (Recommended).** `refs/<lineage>` keeps holding
  exactly one address. Whether the tip is a product or a collection is read from the object by one discriminator
  (D1.1). A lineage's kind is its first version's and never changes.
  *Costs:* one discriminator rule to specify and pin. Learning a lineage's kind costs one object read, and every verb
  that uses the tip reads it anyway.
- **B. One namespace, a typed record (`collection blake3:…`).** The ref states the kind.
  *Costs:* a second source of truth for the kind beside the object. The two can disagree and must be cross-checked on
  every read; ADR-0049 §4 rejected exactly this for the member file suffix. It gains nothing for old binaries that A
  does not already give (below).
- **C. A separate namespace (`refs/collections/<lineage>`).** A directory per kind.
  *Costs:* **silent data loss with alpha.7.** Its `gc` skips sub-directories, so it sweeps every object only a
  collection ref reaches, and exits 0. Rejected.

**D1.1 — the object-kind discriminator.** A stored version object is a JSON object with a top-level
`tessera_version`. It is a **product manifest** iff it has a top-level `product` key and no top-level `members` key,
a **collection** iff it has `members` and no `product`; both or neither is `Error::AmbiguousObjectKind`. Every writer
already emits these keys (`Manifest::product` is required; `Collection::members` is serialised even when empty), and
a test pins both. A discriminator is needed, not just a "try each parser": a zero-block product manifest **parses as
a collection** today, since every required collection key is also a manifest key, `members` defaults to empty, the
product's own keys become newer fields (ADR-0060 §2.2), and the empty-MMR `content_hash` matches. Once D2 lands,
a manifest's `sources` (always written) is even a known collection key of the same type, which makes the overlap
wider, not narrower. Edges (D2) carry no kind, so `gc` needs the same rule for objects it reaches through an edge.

**Old binaries under A.** An alpha.7 binary given a repository with a collection lineage: `gc` reads the tip as a
manifest, fails (no `product`), and aborts with nothing deleted; `tessera commit` on that lineage fails in
`get_manifest`; `import` into it is `RefMoved`; `log` reads the cache, which is harmless. Nothing is lost silently.
Step 7's cross-check pins this against the released alpha.7 binary.

**Compare-and-set.** Unchanged from ADR-0036's 2026-11-24 amendment: a collection commit holds the one writer lock
from its first check to its last write and refuses a moved tip with `Error::RefMoved { lineage, expected, found }`.
One lock per repository, not per lineage or per kind, because `gc` spans both kinds.

**`commit` for a collection** (`Repository::commit_collection(lineage, expected, &Collection, members)`), under the
lock, in this order, refusing before anything is stored:

1. `check_ref(lineage, expected)` → `RefMoved`.
2. With a tip, its kind must be collection → `Error::LineageKindMismatch { lineage, tip_kind }`. The existing
   product `commit` gains the mirror check, so a product can never advance a collection lineage either.
3. With `expected = Some(parent)`, exactly one `supersedes` edge, to `parent` → `Error::SupersedesMismatch` (D2),
   and the same `id` as `parent` → `Error::LineageIdMismatch` (model A: `id` is the lineage handle, so a later
   version keeps its first version's `id_inputs`, the `fork` token of D5 included).
4. **Member closure** (D3.1), over what is **stored or supplied in this call**: every member, recursively, is either
   stored and readable as its declared kind, or among the supplied `members`; every block a member product names is
   stored or among its supplied payloads → `Error::MissingMember { path }`. Supplied members are checked in memory
   (sealed, of the kind the member declares, payloads hashing to their digests), so nothing is stored yet.
5. Store the supplied `members` (sealed product manifests with their block payloads, and sealed sub-collections, in
   dependency order: blocks, then products, then sub-collections), then the collection object, durably, in
   ADR-0036's order; advance the ref; append the `log` cache.

Supplying new members **in the same locked call** is deliberate. Storing a delta product in one call and committing
the collection in another leaves a window where a `gc` (which holds the same lock in between) sweeps the
not-yet-reachable delta, and the commit then fails with `MissingMember`. One call makes the delta reachable in the
same critical section that stores it, the property #946 item 4 established for blocks.

**`log` for a collection lineage** is the same: the `log/<lineage>.jsonl` cache (`{manifest_hash, parent}` lines,
kind-neutral), rebuildable by walking `supersedes` from the tip (#971 builds the rebuild for both kinds). `diff` of two
collection versions is D6.

**Lineage names.** As for products, the CLI uses the collection's `id` as its lineage name (model A, ADR-0022). Two
names that differ only in `:`, `/` or `\` versus `_` share a ref file today; that pre-existing defect is #975 and
applies to collection lineages unchanged.

### D2 — A collection version edge

- **A. A sealed `sources: Vec<Source>` on `Collection`, reusing the product edge type (Recommended).** The same
  `Source { role, reference, content_hash }` as a manifest, skipped when empty. Roles: `supersedes` (same lineage,
  the version replaced) and `forked_from` (D5).
  *Costs:* a new sealed key on collections, which is a format addition. In exchange there is one edge vocabulary,
  one `gc` rule and one walk for both kinds.
- **B. A dedicated `supersedes: Option<String>`, plus an `origin` field.** Two purpose-built fields.
  *Costs:* a second edge representation beside `Source`. Every walk (gc, the log rebuild, provenance, RO-Crate) needs
  a collection-specific branch, and the next edge kind needs another field.
- **C. Unsealed: the edge lives only in the ref log or in `aux/`.** No format change.
  *Costs:* not tamper-evident. A collection's lineage could be rewritten without moving any seal, and the `log` is a
  rebuildable cache by decision (ADR-0036 §1), so it can't be the authority. Rejected.

- **It is sealed**, so it is part of the stored bytes and therefore of `manifest_hash`. It is **not** in `id_inputs`:
  `id` is the stable lineage handle across versions (model A); an edge in `id_inputs` would make every version a new
  lineage.
- **It pins the predecessor's `manifest_hash`** (its seal), as SPEC §8 and ADR-0022 do for products: `reference` is
  the predecessor's collection `id`, `content_hash` its `manifest_hash`. Not its `content_hash` (the member MMR):
  that does not change when only the name, description, level or schema changes, so it does not name one version.
- **It is additive.** `#[serde(default, skip_serializing_if = "Vec::is_empty")]`, so every existing collection
  serialises byte-identically: corpus-neutral, no collection golden moves, no `TESSERA_VERSION` bump (the ADR-0060
  precedent). The MMR `content_hash` is untouched: edges are not members.
- **How old readers treat it.** An alpha.7 reader sees `sources` as a newer field (ADR-0060 §2.2, SPEC §4 "Forward
  reading"): it verifies the seal over it, the CLI warns "not interpreted" (`tessera-cli/src/collection.rs`), the
  members are read and verified, and any re-seal or new version from that view is refused with
  `Error::NewerFormatFields`. So an old reader neither drops the edge silently nor writes a version without it; what
  it cannot do is follow it (it does not gc collections at all, D1). Step 1's seal-case vectors pin this.
- **Its own `supersedes` check.** `commit_collection` with a parent requires exactly one `supersedes` edge, to that
  parent, as products do; a first version may carry a `supersedes` to a version held elsewhere, or a `forked_from`.

### D3 — `gc` marks through members, recursively

The walk gains one dispatch: an object reached from a ref, a member pin or an edge is classified by D1.1.

- A **product manifest**: as today, mark its blocks, follow its `sources` edges.
- A **collection**: mark it, push every member as `Reached { address: manifest_hash, expect: member.kind }`, and
  follow its `sources` edges. A sub-collection member recurses through the same loop (ADR-0049).
- An intact non-version object an *edge* pins (a stored source file): kept, no edges, as today.

**D3.1 — a missing or unreadable member.**

- **A. Members are closed: a missing, unreadable or wrong-kind member aborts `gc` (Recommended).**
  *Costs:* `commit_collection` must check the member closure (D1 step 4): one small manifest read per member version,
  with blocks checked for existence only, as #946 decided for products. In exchange the walk never guesses.
- **B. Members are open, like edges: a member not stored is "held elsewhere" and is not followed.**
  *Costs:* a member deleted by hand, or lost to a torn write, looks like an external one, so `gc` proceeds and sweeps
  what only that member reached. That is exactly the #946 item 5 failure. Rejected.
- **C. Closed only for members that are present.**
  *Costs:* a present but unreadable member aborts and a missing one is skipped. That is B's blind spot for the most
  common damage, a deleted file. Rejected.

So, under A: a **member** whose object is absent, does not hash to its address, is refused by its kind's reader, or
whose object kind differs from the member's `kind`, aborts the run with `Error::GcAborted { what, source }` naming the
member path (`<collection hash> → members[3] → members[0]`) and how its root was reached, before anything is deleted.
**Edges stay open** (ADR-0036 2026-11-24): an edge target not stored names a version held elsewhere and is not
followed. The asymmetry is the meaning: a member says "this is part of me", an edge says "I came from that".

A collection with members held elsewhere (a cohort catalog over products in a registry) is therefore not committable
to a repository; it stays a file (`collection.json` beside its members, ADR-0049 §3). The extension point for a
member that is deliberately external is a new `MemberKind` (`ExternalRef`, which ADR-0049 §1 reserved by making the
enum `#[non_exhaustive]`), not a relaxed rule. Not needed by kit; not built here.

**D3.2 — cycles and depth.** A sealed object cannot pin its own seal, or an ancestor's, without a BLAKE3 preimage,
so the member and edge graph is acyclic by construction (ADR-0049 §3). Hand-editing cannot add a cycle either: the walk
reads every object through its address (a version through its seal check, any other object through `check_digest`),
so an edited object is refused before its edges are followed, and a cycle of intact objects would need a preimage.
The walk keeps its `walked` set because it is what makes a shared member (a base pinned by thirty `current`
versions) cost one visit. The walk is an explicit stack, not recursion, so depth costs heap, not call
stack; there is **no depth limit**, because any limit would be a silent "stop marking" and so a sweep of what lies
below it. The cost of a `gc` is the number of distinct version objects reachable plus the object listing (D7).

### D4 — Durable pins

A pin is a root of `gc` that is not a lineage tip: it names one stored version (either kind), it never advances, and
it is set (`set_pin`, which refuses an address that is not stored, as `set_ref` does) and removed (`remove_pin`) only
explicitly, under the writer lock, durably. Lineage verbs (`commit`, `log`, `import`, `forget`) never act on a pin,
and pin verbs never act on a lineage. What has to be chosen is where a pin lives, and that is decided by what an
alpha.7 binary does with each layout. Two alpha.7 facts govern it: its `gc` reads only plain files directly under
`refs/` (`all_ref_tips`), and its `forget` **deletes `refs/<ref_file_name(name)>` without parsing it**
(`repo.rs:451-466`, CLI `version.rs:363`), then exits 0.

- **A. A reserved flat file name, `refs/pin:<name>`, holding a plain address (Recommended).** The file holds
  `blake3:<hex>`, exactly as a lineage ref does, so the ref grammar does not grow. What makes it a pin is the `pin:`
  prefix of the file name, and every tessera build, alpha.7 included, maps `:` (and `/`, `\`) to `_` when it turns
  a lineage name into a file name (`ref_file_name`). So **no lineage verb of any build can name a pin file**: alpha.7
  `forget pin:x` removes `refs/pin_x`, never `refs/pin:x`. A new build reads a `refs/` entry whose file name starts
  with `pin:` as a pin and every other entry as a lineage, so pins and lineages cannot collide either. A new build
  refuses a lineage name that starts with `pin:` (`Error::Invalid`), so `tessera forget pin:x` is an error instead
  of quietly acting on a lineage `pin_x`.
  Under alpha.7 the pin is simply one more root: its `gc` reads the address and **keeps** everything the pin reaches
  (it aborts only if that is a collection, by D1), and its `commit`, `import`, `read_ref` and `forget` cannot reach
  the file.
  *Costs:* `:` is not a legal file-name character on Windows (NTFS). Windows is not a release target today (deferred
  in `dist-workspace.toml`), but before it ships the reserved prefix needs an on-disk spelling that is legal there
  and still unreachable by `ref_file_name`; that is a layout decision owed then, and an object-store repository has
  no such limit. A human `rm` still removes a pin, as it removes any ref.
- **B. A typed record `pin blake3:<hex>` in a flat `refs/<name>`** (this ADR's first draft).
  An alpha.7 `gc` reads the record as `Error::CorruptRef` and aborts, which fails closed.
  *Costs:* a **mixed-version hazard**. An alpha.7 `tessera forget <name>` deletes the pin file without reading it and
  exits 0, and the next `gc` of any build may then delete the fork point or merge base the pin protected. Each way
  to close that hole has a real cost:
  - *A repository version gate* (a `format` file a new build writes on the first pin, and later builds check):
    alpha.7 never reads it, so it does not stop the binary that causes the hazard. It only guards later skew.
  - *A refusal on the new side* (an append-only pin journal, with `gc` refusing to run when a journalled pin is gone
    without a `remove_pin` entry): it notices only after the fact, and only if a new build runs `gc` before an
    alpha.7 `gc` does. It also adds a second piece of state that must stay durable beside the refs.
  - *Documentation only* ("do not run alpha.7 on a repository with pins"): leaves the hazard to the operator.
- **C. A sub-directory: kit's request `refs/pins/<name>`, or a top-level `pins/`.** A visibly separate namespace,
  which is what kit asked for (#947, second comment, item 1).
  *Costs:* **silent data loss with alpha.7**, for both spellings. `all_ref_tips` skips every directory, `refs/pins/`
  included, so an alpha.7 `gc` treats every pinned version as unreachable, sweeps it, and exits 0. Rejected. kit's
  spelling can survive as the pin's *name*: `set_pin("pins/x", …)` gives the file `refs/pin:pins_x` under A.
- **D. No pins; kit fakes one with a lineage it never advances.** Nothing to build.
  *Costs:* a fake pin can be advanced, imported into or forgotten by accident, nothing says it is a pin, and `log`
  shows a lineage that isn't one. kit's comment asked for exactly this not to be needed.

**What the recommendation does about the mixed-version `forget` hazard.** A removes it by construction: no build's
`forget` can produce a path that contains `:`, so there is nothing to gate and nothing to detect. If the owner prefers
B, it needs one of the three mitigations listed under B, and none of them stops an alpha.7 `forget` on its own. That
choice is the owner's (see "Open for the owner").

Under every option, a later ref layout this build does not know (a file under `refs/` that is neither a lineage
address nor a pin) is `Error::CorruptRef` and aborts `gc`, so a newer layout fails closed against this build.

**When a pin matters.** `gc` follows every stored edge, `supersedes` included, and a ref reaches every earlier
version of its lineage (ADR-0036, 2026-11-24). So today an old version of a **live** lineage is never reclaimed, pin
or no pin. A pin changes the outcome in exactly two cases:

1. **The lineage that reached the version is forgotten.** A dropped workspace whose fork point, or a forgotten
   lineage whose release point, must stay readable.
2. **Retention cuts the chain** (#906). Once a retention class lets compaction write an edge that no longer pins a
   superseded generation, that generation is reachable only through a pin. **A pin overrides every retention
   class**: retention decides which *edges* pin, never which *roots* count. #906 must keep that rule, and erasure
   (#591) has to remove or refuse a pin explicitly, never pass under it.

So pins are cheap to build now and become load-bearing with #906; kit's merge bases (D5 step 7) need them from then
on, or as soon as a variant lineage can be forgotten.

### D5 — kit's workspace, mapped onto D1–D4

A workspace is a **second lineage of `current` collections**. Fork, conflict check and merge are **kit code**; each
step below names the tessera call it uses and nothing more.

**The origin edge.** The workspace's first version carries one `forked_from` edge to the main lineage's version at
the fork (`reference` = main's collection `id`, `content_hash` = that version's `manifest_hash`) and no `supersedes`.

The role of the origin edge:

- **A. A new role token `forked_from` (Recommended).**
  *Costs:* one more token in the role vocabulary. RO-Crate projects it as `prov:wasDerivedFrom`, like `derived_from`.
  A walk finds the fork point by role, unambiguously.
- **B. Reuse `derived_from` (#947's proposal).**
  *Costs:* no new token, but `derived_from` means "computed from" (a recon from a raw product), and a copy taken to
  edit is not that. Once collections carry provenance edges too, a merge-base walk would have to guess which
  `derived_from` is the origin.

**The forked lineage's `id`** (#947 "Open"; kit's comment item 2: unique across nodes, not clock-only):

- **A. `id_inputs` gain a caller-supplied `fork` token, required on a version with a `forked_from` edge
  (Recommended).**
  *Costs:* none in the format, since `id_inputs` is an open map; `id` stays a pure function of declared inputs
  (writer determinism). Uniqueness across nodes is kit's to supply (a node id plus a counter, or 128 random bits).
  tessera refuses a forked version without a token (`Error::Invalid`) and detects a local collision for free: the
  lineage name is the `id`, and a first commit on an existing ref is `RefMoved`.
- **B. The default `{product, name, timestamp}`.**
  *Costs:* two workspaces forked from one document in the same second on two nodes get one `id`, so offline work
  collides at sync. Rejected.
- **C. tessera mints a random `id`.**
  *Costs:* breaks "the same inputs give the same bytes" for collection ids, and moves a policy (how unique, from what
  entropy) into the format. Rejected.

**The kit flow, step by step:**

1. **Fork.** kit reads main's tip `M` (`read_ref`), builds `W0` with `M`'s members in `M`'s order, a `forked_from`
   edge to `M` and a `fork` token, and calls `commit_collection(ws, None, W0, [])`. The `forked_from` edge keeps `M`
   (and so the base and deltas it pins) alive for as long as the workspace lineage exists; no pin is needed for a
   short-lived workspace.
2. **Work.** Each flush writes a delta product against `W`'s base and advances the workspace:
   `commit_collection(ws, Some(Wn), Wn+1, [delta])`, the delta supplied in the same call (D1).
3. **Conflict check (kit).** kit reads main's tip `M'`, lists what changed on each side with the member diff (D6):
   `diff(M, M')` and `diff(W0, Wn)`, and compares the key sets of the added deltas with the keyed read (#780, #903).
   tessera returns facts; deciding what is a conflict is kit's.
4. **Merge (kit).** kit writes one new delta on main against main's *current* base (a delta is valid only against its
   collection's base, #781's comment of 2026-11-21; if main compacted since the fork, kit rewrites the rows against
   the new base, never re-points a delta) and calls `commit_collection(main, Some(M'), M'+1, [merged])` with one
   `supersedes` edge to `M'`. A `RefMoved` means main moved during the check: kit repeats step 3 on the new tip. That
   is the compare-and-set protocol, not a retry that hides a fault.
5. **Recording the merge.** If kit wants the merge visible in the sealed history, it adds an edge to the workspace's
   tip. A **pinning** edge (with `content_hash`) keeps the workspace's whole history alive through main for as long as
   main reaches that version, since `gc` follows every stored edge, by design: one liveness rule, no role-specific
   exceptions. A **reference-only** edge (no `content_hash`) records the merge without retaining it; its shape is the
   non-pinning edge #591 and #906 define. kit chooses per merge; tessera gives either no merge meaning.
6. **Drop.** `forget(ws)` then `gc`. Whatever only the workspace reached is reclaimed; the base stays while main (or a
   pin) reaches it. The cost of this is D7.
7. **Long-lived variants merged both ways.** kit pins each merge base (`pin`), so the common ancestor of the next
   three-way merge survives once retention (#906) cuts either lineage's chain, or after a variant is forgotten
   (D4, "When a pin matters"), and removes the pin when the base moves.

**Exactly what tessera must provide** (and nothing else for workspaces):

| # | Provision | Decision | Step |
|---|---|---|---|
| T1 | Sealed `sources` on collections; roles `supersedes`, `forked_from` | D2, D5 | 1 |
| T2 | The object-kind discriminator; `put_collection` / `get_collection`; the `fork` token rule | D1.1, D5 | 2, 3 |
| T3 | `gc` through members, recursively, closed members, open edges | D3 | 3 |
| T4 | `commit_collection`: CAS, lineage kind, `supersedes`, member closure, members in-call | D1 | 4 |
| T5 | `log` and a `supersedes`/`forked_from` walk from any version (`Repository::history`) | D1, D5 | 4 |
| T6 | Pins: `set_pin`, `remove_pin`, `read_pin`, `pins()`; `gc` roots | D4 | 5 |
| T7 | The member diff, a pure function in `tessera-core` | D6 | 2 |
| T8 | CLI: `tessera collection commit/log/diff`, `tessera pin set/rm/ls` | all | 6 |

Not tessera's: the merge base computation (a walk over T5 output), the conflict rule, the merged delta's rows,
workspace naming and the choice of `fork` token, when to pin, and every policy about who may merge.

### D6 — The member diff

`tessera_core::collection::diff(old, new) -> Result<MemberDiff>` with `added`, `removed` and `repinned`
(`reference`, old `manifest_hash`, new `manifest_hash`), each in the order of the collection it comes from, plus
`order_changed: bool` (whether the members both share appear in a different relative order). Members are keyed by
`reference`. Order is reported because it is identity (ADR-0049 §1, `content_hash`) and, by #781's comment, the order
a reader applies deltas in. A collection that lists one `reference` twice has no well-defined diff, so `diff` refuses
it with `Error::DuplicateMemberReference` rather than pick one; whether sealing should refuse duplicates outright is a
format tightening left to the #781 ADR (a `current` collection never needs them). A `kind` change of a member with an
unchanged `reference` is reported as `repinned` (a kind change always changes the pinned object).

### D7 — The cost of dropping a lineage

`forget` plus `gc` costs the mark of every root plus a listing of every object: it grows with the repository, not
with the workspace. kit's request is that discard follow what the workspace wrote (#947, second comment, item 3).
The options (a full `gc`, reclaim-on-forget that skips the sweep's listing, a per-workspace object area that is
dropped whole) trade layout changes against cost that nobody has measured, so this ADR **fixes the invariant and
defers the mechanism to a measurement, #987**: whatever path is built, the objects left after a drop equal what a
full `gc` leaves, under the one writer lock, with retention and WORM holds applied. Correctness does not wait on
#987; only the discard cost does.

### D8 — Retention and WORM across a fork

Reachability is repository-wide, retention is declared per collection (#906). A workspace that pins an old base
keeps it alive past the main lineage's `latest-only` class: intended, since the workspace still reads it. A pin
(D4) does the same and outranks every class: retention decides which edges pin, never which roots count. An upper
bound on how long a workspace may hold a generation is a retention-class question for #906. `gc` still has no WORM
check (ADR-0036 "Still open"); none is added here.

### D9 — Non-goals: no branches, no content merge

Restated for ADR-0036's Consequences, replacing "Branch / merge are explicitly out of scope … `refs` are
single-lineage pointers, not branches":

> **Branches and content merge are not tessera's.** A ref is a single-lineage pointer or a pin, never a branch.
> Two lineages may share objects and may be linked by an origin edge (`forked_from`); that records where a lineage
> began and keeps what it shares alive, and gives no merge meaning. Combining the work of two lineages is an
> application's code (kit's, ADR-0067 D5), and lands as an ordinary commit on one lineage.

Also out of scope here: staging (ADR-0036 keeps it out), multi-writer commits (#288), the object-store form of a
repository (ADR-0036 2026-11-24), the `current` collection's schema, delta semantics and compaction (#781), and the
retention classes themselves (#906).

## Failure modes

Each line: the fault, what happens, and the decision that settles it.

- **Two writers advance one collection lineage.** One wins; the other gets `RefMoved` and has stored nothing (D1,
  the ADR-0036 lock).
- **A product commit aimed at a collection lineage, or the reverse.** `LineageKindMismatch`, nothing stored (D1
  step 2).
- **A `gc` between storing a delta and committing the collection that pins it.** Cannot happen through tessera:
  both are one locked call (D1 step 5).
- **A member object deleted by hand, torn, or of the wrong kind.** `commit_collection` refuses with
  `MissingMember`; `gc` aborts, names the member path and deletes nothing (D3.1).
- **An edge to a version held elsewhere.** Not followed, nothing aborted, as today (D3.1).
- **An object that matches both or neither side of the discriminator.** `AmbiguousObjectKind`; `gc` aborts (D1.1).
- **A very deep or very wide member graph.** A heap-bounded walk, one visit per distinct object, no depth cut-off
  (D3.2).
- **An alpha.7 binary on a repository with collection lineages.** Its `gc` aborts and its `commit` fails; nothing is
  deleted (D1).
- **An alpha.7 binary on a repository with pins.** Under D4 A, its `gc` reads each pin as one more root and keeps
  what it reaches. Under D4 B, it reads the pin as `CorruptRef` and aborts, deleting nothing (D4).
- **Mixed versions: an alpha.7 `tessera forget <pin name>`.** alpha.7's `forget` deletes `refs/<name>` without
  parsing it and exits 0. Under D4 A it cannot name a pin file, since its `ref_file_name` maps the `:` of `pin:` to
  `_`, so the pin stays. Under D4 B it deletes the pin, and the next `gc` of any build may reclaim the fork point or
  merge base the pin protected. That is the hazard D4 B's mitigations address, none of them completely (D4).
- **An alpha.7 reader on a collection with `sources`.** It verifies the seal, warns "not interpreted" and refuses any
  re-seal (D2).
- **A new-build lineage command given a `pin:` name.** Refused (`Error::Invalid`); pin verbs address only
  `refs/pin:<name>` files, never a lineage (D4).
- **Two workspaces forked offline with the same `fork` token.** The same `id`, so the same lineage name: the second
  first commit is `RefMoved` locally, and across nodes kit's sync sees the collision (D5).
- **A workspace keeps an old base past main's retention.** Kept while the workspace lives, by design (D8).
- **`forget` and `gc` of many short lineages in a large repository.** Correct; the cost grows with the repository
  (D7, #987).
- **Lineage names that collide in the ref file name (`a:b`, `a_b`).** Pre-existing and unchanged (#975).

## Test plan

Each test fails on `dev` today and passes after its step; each PR records the test's first failing run. Grouped by
step, with the #947 acceptance item each one proves.

- **Step 1.**
  - `collection_sources_are_sealed_and_skipped_when_empty`: an edge changes `manifest_hash`; a collection without
    one gives the golden bytes (the version edge is sealed and corpus-neutral).
  - Seal-case vectors for a collection with `sources`: it verifies with `sources` as a newer field under a reader
    that does not model it, and is tampered when the key is spliced in under the old seal (old readers).
  - `a_forked_version_without_a_fork_token_is_refused`: a collection with a `forked_from` edge and no `fork` key in
    `id_inputs` is `Error::Invalid` at seal.
- **Step 2.**
  - `object_kind_discriminates_a_zero_block_manifest`: a zero-block manifest is a product, not a collection; both
    keys and neither key are refused.
  - `member_diff_reports_added_removed_repinned_and_order`, plus a property test that applying the diff to `old`
    gives `new`'s members (#947: "`diff` of two collection versions lists exactly the members that differ").
  - `member_diff_refuses_a_duplicate_reference`: a collection listing one `reference` twice, on either side, gives
    `Error::DuplicateMemberReference`.
- **Step 3.**
  - `gc_marks_through_collection_members`: a base and deltas pinned only by a `current` collection survive `gc` and
    read back by hash. Today this aborts with `GcAborted`, the collection read as a manifest (#947 item 2).
  - `gc_marks_through_nested_collections`: three levels, a shared member visited once.
  - `gc_aborts_on_a_missing_member_and_names_its_path`, and the same for a torn and a wrong-kind member: nothing is
    deleted.
- **Step 4.**
  - `commit_collection_advances_by_cas_and_reads_back`, `commit_collection_built_on_a_moved_tip_is_refused`, and the
    N-process race test of #946 run on a collection lineage (#947 item 1).
  - `commit_collection_refuses_a_product_lineage` and `commit_refuses_a_collection_lineage`: each is
    `Error::LineageKindMismatch` and stores nothing (both directions of D1 step 2).
  - `commit_collection_refuses_a_version_with_another_id`: a parent of the lineage and a new version whose `id`
    differs is `Error::LineageIdMismatch`.
  - `commit_collection_refuses_a_wrong_supersedes_edge`: no edge, two edges, or one to another version is
    `Error::SupersedesMismatch` on a collection, as on a product.
  - `commit_collection_stores_new_members_in_the_same_critical_section`: the durable-seam recorder of #946 shows no
    `GcWalk` between storing the delta and advancing the ref.
  - `two_lineages_share_a_base_until_both_are_forgotten`: forget one and `gc`, the base stays; forget both and `gc`,
    it is reclaimed (#947 item 3).
  - `history_from_a_fork_reaches_the_main_version_it_forked_from` (#947 item 4).
- **Step 5.**
  - `a_pin_keeps_a_version_alive_until_removed`: pin a version of a lineage, `forget` the lineage and `gc`: the
    version and everything it reaches read back; remove the pin and `gc`: they are reclaimed (kit's added
    acceptance item; the "after compaction" half of that item becomes testable with #906, whose tests must include
    a pinned generation outside the class).
  - `set_pin_refuses_an_unstored_address`.
  - `lineage_verbs_refuse_a_pin_name`: `commit`, `import`, `log` and `forget` given `pin:x` return `Error::Invalid`
    and leave `refs/pin:x` in place.
  - `remove_pin_never_touches_a_lineage`: a lineage `x` and a pin `x` coexist; removing the pin leaves the lineage.
- **Step 7.** The alpha.7 cross-check job, run against the released alpha.7 binary on two repositories:
  - one with a collection lineage and a collection with `sources`: alpha.7 `gc` exits non-zero and every object
    remains;
  - one with a product lineage and a pin on its first version: alpha.7 `forget pin:x` leaves `refs/pin:x`; after
    alpha.7 `forget` of the lineage itself and alpha.7 `gc`, the pinned version and its blocks remain.
- **#987.** The discard probe (kit's second added acceptance item).

## Implementation plan

Ordered, one PR each, each under 1500 changed lines.

1. **Core: `sources` on `Collection`** (T1). The field, `FIELDS`, a builder method, role constants `supersedes` and
   `forked_from`, the `fork`-token check for a forked version, SPEC §4 (collection record) and §8 (versioning covers
   collections), seal-case vectors. Corpus-neutral by test. About 500 lines.
2. **Core: the object-kind discriminator and the member diff** (T2 core half, T7). `ObjectKind::of(bytes)` and
   `collection::diff`. About 600 lines.
3. **Io: collection objects in the repository and `gc` through members** (T2, T3). `put_collection`,
   `get_collection`, `walk_read` dispatch by kind, closed members, member-path errors. About 900 lines.
4. **Io: `commit_collection`, `log` and `history`** (T4, T5). CAS, lineage-kind checks both ways, member closure,
   new members in the same call, the walk over `supersedes`/`forked_from`. About 1100 lines.
5. **Io: pins** (T6). The `refs/pin:<name>` layout (or D4 B's record and mitigation, if the owner picks it),
   `set_pin` / `remove_pin` / `read_pin` / `pins`, `gc` roots, lineage verbs refusing `pin:` names. About 600
   lines.
6. **CLI** (T8). `tessera collection commit|log|diff`, `tessera pin set|rm|ls`, trycmd walkthroughs as docs (a
   fork, a merge written as one delta, a drop), the book chapter on repositories. About 1000 lines.
7. **Cross-check and amendments.** The alpha.7 fail-closed job; ADR-0036 and ADR-0049 "Amended by ADR-0067"
   pointers and D9's restatement; this ADR to Accepted. About 300 lines.

Steps 1 and 2 are independent; 3 needs 1 and 2; 4 needs 3; 5 needs 3; 6 needs 4 and 5; 7 is last. The #781 ADR can
proceed in parallel: it needs D1–D3 decided, not built.

## Open for the owner

Each has a recommendation above; these are the ones whose answer changes the build:

1. D1: the object decides the kind (A) over a typed ref record (B).
2. D3.1: members are closed in a repository (A), so a catalog over external members stays a file until an
   `ExternalRef` kind exists.
3. D4: pins as a reserved flat file name `refs/pin:<name>` (A), which removes the mixed-version `forget` hazard by
   construction at the cost of a Windows spelling owed later; or a typed record (B) plus one of its mitigations.
4. D5: the new role `forked_from` (A) over reusing `derived_from`, and a caller-supplied `fork` token in `id_inputs`.
5. D7: correctness now, discard cost after the #987 measurement.

## References

#947 (this), #781, #946 (PR #973), #292, #288, #945, #906, #591, #971, #975, #987; nerdmachines/kit#214,
nerdmachines/kit#97. ADR-0022, ADR-0033, ADR-0036, ADR-0049, ADR-0050, ADR-0060, ADR-0066. `tessera-io/src/repo.rs`,
`tessera-core/src/collection.rs`, `tessera-core/src/provenance.rs`.
