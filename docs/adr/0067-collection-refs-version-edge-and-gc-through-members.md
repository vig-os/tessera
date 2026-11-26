# ADR-0067 — Collection refs, a collection version edge, and gc through members

Status: **Accepted** (owner, 2026-11-26) · Issue #947 (open until implemented) · Extends **ADR-0036** (the
repository, as amended on 2026-11-24 by #946 and by #975's injective ref names) and **ADR-0049** (recursive
collections); touches SPEC §4 and §8. Relates ADR-0022 (the version DAG), ADR-0033 and ADR-0050 (collections, level
schemas), ADR-0060 §2.2 (forward reading), ADR-0066 (stored bytes; rule 10, no migration). Feeds the #781 ADR (keyed
deltas, the `current` collection, compaction), #906 (retention) and #292 (the ownership boundary).

**The owner's rulings (2026-11-26).** On D1 and D4: "we are alpha .. what would be greenfield correct? its ok to break
backwards compat" and "green field correctness counts here -- we dont want to build baggage in an alpha!?". So the
repository layout is designed greenfield: compatibility with earlier tessera builds is **not** a goal, and no choice
below is made for an older binary's sake. D3.1 A, D5 A and D7 were confirmed as proposed.

## Why a new ADR and not a dated ADR-0036 amendment

The change has two halves: a **format** change to collections (a sealed edge list, D2), which is ADR-0049's and SPEC
§4's territory, and **repository** semantics (the ref layout, commit, gc, pins), which are ADR-0036's. A dated
amendment to ADR-0036 would bury a seal-affecting format change inside a store ADR, and its two halves could not be
decided or revised apart. So this is its own ADR, and it **extends ADR-0036's one versioning story** rather than
starting a second one (the owner's ruling of 2026-11-09 on #781): the same objects, the same compare-and-set under the
same fenced writer, the same `gc`. ADR-0036 and ADR-0049 get an "Amended by ADR-0067" pointer, and ADR-0036's branch
sentence is restated as D9 says, in the last implementation step.

## Context

kit needs **workspaces**: a person or an agent edits a private copy of a keyed collection and the result is merged
back. The owner decided on 2026-11-21 that branches live in kit: tessera keeps **no branches and no content merge**.
With that split, a workspace is **a second lineage of `current` collections** (#781) whose first version pins the
same base and deltas as the main lineage at the fork; fork, conflict check and merge are kit code, and a merge
writes one new delta on the main lineage.

What blocks that today (read at `d24c1f9a`, which contains #946's fix as `ca751291`):

1. **A ref cannot name a collection.** `Repository` stores product manifests; `commit` takes a `Manifest`;
   `get_manifest` parses one. No collection type or verb appears in `tessera-io/src/repo.rs`.
2. **A collection has no version edge.** `Collection` (`tessera-core/src/collection.rs`) has `id`, `id_inputs`,
   ordered `members` pinned by `manifest_hash`, `content_hash` (the kind-tagged MMR over members, ADR-0049 §2) and
   the seal; nothing names the version it replaces or the lineage it was forked from.
3. **`gc` does not mark through members.** It marks a manifest's `blocks` and follows its `sources` edges. A base
   and deltas that only a `current` collection pins would be reclaimed. A collection object reached by the walk is
   *manifest-shaped* (it has `tessera_version`), so `walk_read` reads it as a product manifest, which fails (a
   collection has no top-level `product`), and `gc` aborts.
4. **No durable pin.** A three-way merge in kit needs the common ancestor; for long-lived variants that is an old
   version of either lineage, which must stay readable (#947, second comment, item 1).
5. **Refs are untyped files in one flat directory.** `refs/<name>` holds a bare address, so a ref cannot say what it
   is (a lineage tip or a pin) or what it names (a product or a collection). #975 (PR #990, merged as `93fc6716`)
   made the name-to-file mapping injective by percent-encoding and added a `layout` marker (`tessera-repository 1`).

## Decisions

Each decision states what was decided. The options it was chosen over follow, one line each with the reason.

### D1 — The ref layout: typed records in separate namespaces (repository layout 2)

A repository at **layout 2** is laid out as:

```text
layout                           tessera-repository 2
objects/<alg>/<aa>/<rest-of-hex> write-once content-addressed objects (unchanged)
refs/lineages/<enc>              a lineage tip:  "<kind> <address>\n"
refs/pins/<enc>                  a pin:          "<kind> <address>\n"
log/lineages/<enc>.jsonl         a lineage's rebuildable log cache (ADR-0036 §1)
lock, tmp/                       the writer lock and temp files (ADR-0036, 2026-11-24)
```

- **A namespace per ref role, a directory each.** What a ref *is* (a lineage tip that advances by compare-and-set, or
  a pin that never advances) is the namespace it lives in. Listing one role is reading one directory; a lineage and a
  pin may share a name without colliding; no name is reserved and no prefix carries meaning. A later role (a signed
  tag, say) is a new directory under a layout bump. A directory per *target kind* (`refs/collections/`) was not
  chosen: the kind is a property of what the ref names, and belongs in the record.
- **One record grammar for both namespaces:** exactly `<kind> SP <address> LF`, where `<kind>` is `product` or
  `collection` and `<address>` is `blake3:<64 lower-case hex>`. One spelling per value: no other whitespace, no
  upper-case hex, nothing after the line feed. Anything else is `Error::CorruptRef` naming the file and what it holds.
  A line grammar rather than JSON because the record has two fields of closed alphabets: a line has one spelling by
  construction, where JSON would need JCS to have one.
- **What a ref holds, and nothing more.** The kind and the target. Not its name (the encoding is reversible), not a
  time or an author (the log cache and the sealed versions carry those), not the previous tip (the `supersedes` edge
  is the authority, ADR-0036 §1).
- **The kind in the record is checked against the object.** A version object says its own kind (D1.1); every read of
  a ref's target compares the two and refuses a disagreement with `Error::RefKindMismatch { path, record, object }`.
  The record makes a lineage's kind a declared, fixed fact that `commit`, `log` and `gc` know without reading the
  object; the object stays the authority on what its bytes are (ADR-0049 §4).
- **One injective name encoding for both namespaces:** #975's percent-encoding (PR #990): every UTF-8 byte outside
  `a-z`, `0-9`, `-` and `_` is written `%xx` in lower-case hex, `%` included, so `a:b`, `a_b`, `A_b` and `a.b` are four
  files on any file system, case-insensitive ones included. The bound is #990's: an encoded name of at most 249 bytes
  (255 less `.jsonl`), for pins too so there is one rule; an empty or longer name is refused before the writer lock
  is taken (#990's `Error::InvalidLineage`, generalised to `Error::InvalidRefName { namespace, name }`). The `:` rule
  an earlier draft reserved `refs/pin:<name>` on is moot: pins have their own directory.
- **The layout marker.** `layout` at the repository root holds exactly `tessera-repository 2\n`. `init` writes it.
  Every entry point (`open`, `init` on an existing directory, and through them every verb) refuses any other state
  with `Error::UnsupportedRepositoryLayout { found, required }`, naming the layout it found (`tessera-repository 1`,
  "no marker, refs at the top of `refs/`", or the unknown marker's text) and the one it requires. A newer marker is
  refused the same way. A directory without a marker is never a repository to `open`, even an empty one: only `init`
  makes one, and `init` writes the marker into a directory that holds no `refs/`, `log/` or object entries, and
  refuses any other unmarked directory. (#990 marks an empty unmarked repository on its first write; layout 2 drops
  that, since no unmarked repository of this layout can exist.) Entries under `refs/` that are not the two namespace
  directories, and entries inside them that are not regular files, are `Error::CorruptRef`, and `gc` aborts on them
  before deleting anything.
- **No migration verb** (the policy of ADR-0066 rule 10). A layout-1 or pre-#975 repository is refused, never
  converted in place. Its data moves the way every other format break moved: with the build that wrote it, `tessera
  seal` each lineage tip into a self-contained `.tsra`, then `import` those into a new repository. History edges
  travel inside the sealed versions; the old repository stays as it was.
- **Ordering with #975.** PR #990 landed first (`93fc6716`): it is the encoding this layout uses. Step 1 below then
  moves refs into the namespaces, makes the records typed, moves the log cache, and bumps the marker from 1 to 2.
  #990 has no migration verb (it refuses a pre-#975 repository with `Error::UnsupportedRepositoryLayout`, never
  converts it), so nothing of it has to be retired. Two of its parts point at the `refs/pin:` reservation an earlier
  draft of this ADR made: the text of its ADR-0036 amendment and the test
  `no_lineage_maps_to_a_file_name_holding_a_colon`. Step 1 removes or retargets both under namespaced refs.
- **Old tools: no coexistence.** Builds from #990 on refuse layout 2 (they read only `tessera-repository 1`). Builds
  before #990 check no marker and are **not** refused: their `gc` finds no files directly under `refs/`, so it sees no
  roots and would empty the store. They must never be run against a layout-2 repository.

Rejected (greenfield ruling, 2026-11-26), one line each:

- *A bare address in `refs/<name>`, the object saying its kind:* chosen in earlier drafts only so that alpha.7 tools
  failed closed; it leaves a ref unable to say what it is.
- *A typed record in one flat `refs/` namespace:* lineages and pins would share names, so one of them needs a reserved
  spelling.
- *A directory per target kind (`refs/collections/<name>`):* the kind belongs to the target, and is checked against it.
- *Keeping layout 1 readable, or a migration verb:* compatibility is not a goal in the alpha, and ADR-0066 rule 10
  already set the policy.

**D1.1 — the object-kind discriminator.** A stored version object is a JSON object with a top-level
`tessera_version`. It is a **product manifest** iff it has a top-level `product` key and no top-level `members` key,
a **collection** iff it has `members` and no `product`; both or neither is `Error::AmbiguousObjectKind`. Every writer
already emits these keys (`Manifest::product` is required; `Collection::members` is serialised even when empty), and
a test pins both. A discriminator is needed, not just a "try each parser": a zero-block product manifest **parses as
a collection** today, since every required collection key is also a manifest key, `members` defaults to empty, the
product's own keys become newer fields (ADR-0060 §2.2), and the empty-MMR `content_hash` matches. Once D2 lands, a
manifest's `sources` (always written) is even a known collection key of the same type. Refs carry their target's kind
(D1), but edges (D2) and members carry none or only a claim, so `gc` and the ref check need this rule.

**Compare-and-set.** Unchanged from ADR-0036's 2026-11-24 amendment: a commit holds the one writer lock from its
first check to its last write and refuses a moved tip with `Error::RefMoved { lineage, expected, found }`. One lock
per repository, not per lineage, per kind or per namespace, because `gc` spans all of them.

**`commit` for a collection** (`Repository::commit_collection(lineage, expected, &Collection, members)`), under the
lock, in this order, refusing before anything is stored:

1. `check_ref(lineage, expected)` → `RefMoved`.
2. With a tip, its record's kind must be `collection` → `Error::LineageKindMismatch { lineage, tip_kind }`. The
   product `commit` makes the mirror check, so a product can never advance a collection lineage either.
3. With `expected = Some(parent)`, exactly one `supersedes` edge, to `parent` → `Error::SupersedesMismatch` (D2),
   and the same `id` as `parent` → `Error::LineageIdMismatch` (model A: `id` is the lineage handle, so a later
   version keeps its first version's `id_inputs`, the `fork` token of D5 included).
4. **Member closure** (D3.1), over what is **stored or supplied in this call**: every member, recursively, is either
   stored and readable as its declared kind, or among the supplied `members`; every block a member product names is
   stored or among its supplied payloads → `Error::MissingMember { path }`. Supplied members are checked in memory
   (sealed, of the kind the member declares, payloads hashing to their digests), so nothing is stored yet.
5. Store the supplied `members` (sealed product manifests with their block payloads, and sealed sub-collections, in
   dependency order: blocks, then products, then sub-collections), then the collection object, durably, in
   ADR-0036's order; write the ref record `collection <address>`; append the `log` cache.

Supplying new members **in the same locked call** is deliberate. Storing a delta product in one call and committing
the collection in another leaves a window where a `gc` (which holds the same lock in between) sweeps the
not-yet-reachable delta, and the commit then fails with `MissingMember`. One call makes the delta reachable in the
same critical section that stores it, the property #946 item 4 established for blocks.

**`log` for a collection lineage** is the same as for a product: the `log/lineages/<enc>.jsonl` cache
(`{manifest_hash, parent}` lines), rebuildable by walking `supersedes` from the tip (#971 builds the rebuild for both
kinds). `diff` of two collection versions is D6. As for products, the CLI names a collection lineage by the
collection's `id` (model A, ADR-0022).

### D2 — A collection version edge

**Decided: a sealed `sources: Vec<Source>` on `Collection`, reusing the product edge type.** The same
`Source { role, reference, content_hash }` as a manifest, skipped when empty. Roles: `supersedes` (same lineage, the
version replaced) and `forked_from` (D5). One edge vocabulary, one `gc` rule and one walk for both kinds.

- **It is sealed**, so it is part of the stored bytes and therefore of `manifest_hash`. It is **not** in `id_inputs`:
  `id` is the stable lineage handle across versions (model A); an edge in `id_inputs` would make every version a new
  lineage.
- **It pins the predecessor's `manifest_hash`** (its seal), as SPEC §8 and ADR-0022 do for products: `reference` is
  the predecessor's collection `id`, `content_hash` its `manifest_hash`. Not its `content_hash` (the member MMR):
  that does not change when only the name, description, level or schema changes, so it does not name one version.
- **It is additive.** `#[serde(default, skip_serializing_if = "Vec::is_empty")]`, so every existing collection
  serialises byte-identically: corpus-neutral, no collection golden moves, no `TESSERA_VERSION` bump (the ADR-0060
  precedent). The MMR `content_hash` is untouched: edges are not members.
- **Readers that predate it** follow SPEC §4 "Forward reading": they verify the seal over `sources`, warn that it is
  not interpreted, and refuse to build a new seal from that view (`Error::NewerFormatFields`).
- **Its own `supersedes` check.** `commit_collection` with a parent requires exactly one `supersedes` edge, to that
  parent, as products do; a first version may carry a `supersedes` to a version held elsewhere, or a `forked_from`.

Rejected, one line each:

- *A dedicated `supersedes: Option<String>` plus an `origin` field:* a second edge representation that every walk
  would have to special-case.
- *An unsealed edge (ref log or `aux/`):* not tamper-evident, and the log is a rebuildable cache by decision.

### D3 — `gc` marks through members, recursively

The walk has three kinds of root and reach: the targets of `refs/lineages/` and `refs/pins/` (each checked against
its record's kind), member pins, and edges. Each object reached is classified by D1.1.

- A **product manifest**: as today, mark its blocks, follow its `sources` edges.
- A **collection**: mark it, push every member as `Reached { address: manifest_hash, expect: member.kind }`, and
  follow its `sources` edges. A sub-collection member recurses through the same loop (ADR-0049).
- An intact non-version object an *edge* pins (a stored source file): kept, no edges, as today.

**D3.1 — a missing or unreadable member. Decided (owner, 2026-11-26): members are closed inside the repository.** A
**member** whose object is absent, does not hash to its address, is refused by its kind's reader, or whose object kind
differs from the member's `kind`, aborts the run with `Error::GcAborted { what, source }` naming the member path
(`<collection hash> → members[3] → members[0]`) and how its root was reached, before anything is deleted. The cost
is `commit_collection`'s closure check (D1 step 4): one small manifest read per member version, blocks checked for
existence only, as #946 decided for products. **Edges stay open** (ADR-0036 2026-11-24): an edge target not stored
names a version held elsewhere and is not followed. The asymmetry is the meaning: a member says "this is part of me",
an edge says "I came from that".

A collection with members held elsewhere (a cohort catalog over products in a registry) is therefore not committable
to a repository; it stays a file (`collection.json` beside its members, ADR-0049 §3). The extension point for a
member that is deliberately external is a new `MemberKind` (`ExternalRef`, which ADR-0049 §1 reserved by making the
enum `#[non_exhaustive]`), not a relaxed rule. Not needed by kit; not built here.

Rejected, one line each:

- *Open members (a missing member is "held elsewhere"):* a member deleted by hand looks external, and `gc` sweeps what
  only it reached, the #946 item 5 failure.
- *Closed only for members that are present:* the same blind spot for the commonest damage, a deleted file.

**D3.2 — cycles and depth.** A sealed object cannot pin its own seal, or an ancestor's, without a BLAKE3 preimage,
so the member and edge graph is acyclic by construction (ADR-0049 §3). Hand-editing cannot add a cycle either: the
walk reads every object through its address (a version through its seal check, any other object through
`check_digest`), so an edited object is refused before its edges are followed. The walk keeps its `walked` set
because it is what makes a shared member (a base pinned by thirty `current` versions) cost one visit. The walk is an
explicit stack, not recursion, so depth costs heap, not call stack; there is **no depth limit**, because any limit
would be a silent "stop marking" and so a sweep of what lies below it. The cost of a `gc` is the number of distinct
version objects reachable plus the object listing (D7).

### D4 — Durable pins

**Decided: a pin is a record in `refs/pins/<enc>`** (D1's layout and grammar): a root of `gc` that is not a lineage
tip. It names one stored version of either kind and never advances. It is set (`set_pin`, which refuses an address
that is not stored, as `set_ref` does, and a kind that disagrees with the object) and removed (`remove_pin`) only
explicitly, under the writer lock, durably. Lineage verbs (`commit`, `log`, `import`, `forget`) address
`refs/lineages/` only and pin verbs `refs/pins/` only, so neither can touch the other's refs, by construction.

**When a pin matters.** `gc` follows every stored edge, `supersedes` included, and a ref reaches every earlier
version of its lineage (ADR-0036, 2026-11-24). So an old version of a **live** lineage is never reclaimed, pin or no
pin. A pin changes the outcome in exactly two cases:

1. **The lineage that reached the version is forgotten.** A dropped workspace whose fork point, or a forgotten
   lineage whose release point, must stay readable.
2. **Retention cuts the chain** (#906). Once a retention class lets compaction write an edge that no longer pins a
   superseded generation, that generation is reachable only through a pin. **A pin overrides every retention
   class**: retention decides which *edges* pin, never which *roots* count. #906 must keep that rule, and erasure
   (#591) has to remove or refuse a pin explicitly, never pass under it.

So pins are cheap to build now and become load-bearing with #906; kit's merge bases (D5 step 7) need them from then
on, or as soon as a variant lineage can be forgotten.

Rejected (greenfield ruling, 2026-11-26), one line each:

- *A reserved flat file `refs/pin:<name>`:* existed only so that no alpha.7 verb could reach a pin, and owed a Windows
  spelling.
- *A `pin` record type in the one flat `refs/` namespace:* mixes the ref's role into its record and shares one name
  space; a directory per role is cleaner.
- *No pins, kit faking one with a lineage it never advances:* it can be advanced or forgotten by accident, and nothing
  says it is a pin.

### D5 — kit's workspace, mapped onto D1–D4

A workspace is a **second lineage of `current` collections**. Fork, conflict check and merge are **kit code**; each
step below names the tessera call it uses and nothing more.

**The origin edge. Decided (owner, 2026-11-26): a new role `forked_from`.** The workspace's first version carries one
`forked_from` edge to the main lineage's version at the fork (`reference` = main's collection `id`, `content_hash` =
that version's `manifest_hash`) and no `supersedes`. RO-Crate projects it as `prov:wasDerivedFrom`, like
`derived_from`, and a walk finds the fork point by role, unambiguously.

**The forked lineage's `id`. Decided (owner, 2026-11-26): a required, caller-supplied `fork` token in `id_inputs`.**
`id_inputs` is an open map, so this is no format change, and `id` stays a pure function of declared inputs (writer
determinism). Uniqueness across nodes is kit's to supply (a node id plus a counter, or 128 random bits). tessera
refuses a version with a `forked_from` edge and no `fork` key (`Error::Invalid`, at seal), and detects a local
collision for free: the lineage name is the `id`, and a first commit on an existing ref is `RefMoved`.

Rejected, one line each:

- *Reusing `derived_from` for the origin:* it means "computed from", and a merge-base walk would have to guess which
  `derived_from` is the origin.
- *The default `{product, name, timestamp}` id inputs:* two workspaces forked in one second on two nodes collide.
- *tessera minting a random `id`:* breaks writer determinism and moves a uniqueness policy into the format.

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
7. **Long-lived variants merged both ways.** kit pins each merge base (`set_pin`), so the common ancestor of the next
   three-way merge survives once retention (#906) cuts either lineage's chain, or after a variant is forgotten
   (D4, "When a pin matters"), and removes the pin when the base moves.

**Exactly what tessera must provide** (and nothing else for workspaces):

| # | Provision | Decision | Step |
|---|---|---|---|
| T0 | Layout 2: typed ref records, `refs/lineages/` and `refs/pins/`, the marker, refusal of other layouts | D1 | 1 |
| T1 | Sealed `sources` on collections; roles `supersedes`, `forked_from`; the `fork` token rule | D2, D5 | 2 |
| T2 | The object-kind discriminator; `put_collection` / `get_collection`; the record-object kind check | D1.1 | 3, 4 |
| T3 | `gc` through members, recursively, closed members, open edges | D3 | 4 |
| T4 | `commit_collection`: CAS, lineage kind and id, `supersedes`, member closure, members in-call | D1 | 5 |
| T5 | `log` and a `supersedes`/`forked_from` walk from any version (`Repository::history`) | D1, D5 | 5 |
| T6 | Pins: `set_pin`, `remove_pin`, `read_pin`, `pins()`; `gc` roots | D4 | 6 |
| T7 | The member diff, a pure function in `tessera-core` | D6 | 3 |
| T8 | CLI: `tessera collection commit/log/diff`, `tessera pin set/rm/ls` | all | 7 |

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

**Decided (owner, 2026-11-26): correctness now, the discard cost after the #987 measurement.** `forget` plus `gc`
costs the mark of every root plus a listing of every object: it grows with the repository, not with the workspace.
kit's request is that discard follow what the workspace wrote (#947, second comment, item 3). The options (a full
`gc`, reclaim-on-forget that skips the sweep's listing, a per-workspace object area that is dropped whole) trade layout
changes against cost nobody has measured, so this ADR fixes the invariant and defers the mechanism to #987: whatever
path is built, the objects left after a drop equal what a full `gc` leaves, under the one writer lock, with retention
and WORM holds applied.

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

- **A repository at layout 1, without a marker, or at an unknown layout.** Every entry point refuses it with
  `UnsupportedRepositoryLayout`, naming the layout found and the one required; nothing is read or written (D1).
- **A ref record with another spelling** (trailing space, upper-case hex, an unknown kind). `CorruptRef` naming the
  file; `gc` aborts before deleting anything (D1).
- **A stray file directly under `refs/`, or a directory inside a namespace.** `CorruptRef`; `gc` aborts (D1).
- **A ref record whose kind disagrees with its object.** `RefKindMismatch`; `gc` aborts (D1).
- **Two writers advance one collection lineage.** One wins; the other gets `RefMoved` and has stored nothing (D1,
  the ADR-0036 lock).
- **A product commit aimed at a collection lineage, or the reverse.** `LineageKindMismatch`, nothing stored (D1
  step 2).
- **A version on a lineage with another `id`.** `LineageIdMismatch`, nothing stored (D1 step 3).
- **A `gc` between storing a delta and committing the collection that pins it.** Cannot happen through tessera:
  both are one locked call (D1 step 5).
- **A member object deleted by hand, torn, or of the wrong kind.** `commit_collection` refuses with
  `MissingMember`; `gc` aborts, names the member path and deletes nothing (D3.1).
- **An edge to a version held elsewhere.** Not followed, nothing aborted, as today (D3.1).
- **An object that matches both or neither side of the discriminator.** `AmbiguousObjectKind`; `gc` aborts (D1.1).
- **A very deep or very wide member graph.** A heap-bounded walk, one visit per distinct object, no depth cut-off
  (D3.2).
- **A lineage and a pin with the same name.** Two files in two directories; each verb touches only its own (D4).
- **Two workspaces forked offline with the same `fork` token.** The same `id`, so the same lineage name: the second
  first commit is `RefMoved` locally, and across nodes kit's sync sees the collision (D5).
- **A workspace keeps an old base past main's retention.** Kept while the workspace lives, by design (D8).
- **`forget` and `gc` of many short lineages in a large repository.** Correct; the cost grows with the repository
  (D7, #987).
- **Old tools.** No coexistence: builds from #990 on refuse layout 2; earlier builds are not refused, and their
  `gc` would empty the store, so they must not be run against it (D1).

## Test plan

Each test fails on `dev` today and passes after its step; each PR records the test's first failing run. Grouped by
step, with the #947 acceptance item each one proves.

- **Step 1.**
  - `a_repository_at_another_layout_is_refused_naming_both`: layout 1, no marker with flat refs, and
    `tessera-repository 3` are each `UnsupportedRepositoryLayout` with `found` and `required` set, from `open` and
    from every write. An unmarked empty directory is refused by `open` and marked by `init`; an unmarked directory
    with entries is refused by both.
  - `ref_records_have_one_spelling`: trailing whitespace, upper-case hex, a missing line feed and an unknown kind are
    each `CorruptRef`; `gc` aborts on each and deletes nothing.
  - `a_stray_entry_under_refs_aborts_gc`: a file directly under `refs/`, a directory inside `refs/lineages/`.
  - `ref_names_encode_injectively_in_both_namespaces`: #990's name list, in both directories.
- **Step 2.**
  - `collection_sources_are_sealed_and_skipped_when_empty`: an edge changes `manifest_hash`; a collection without
    one gives the golden bytes (the version edge is sealed and corpus-neutral).
  - Seal-case vectors for a collection with `sources`: it verifies with `sources` as a newer field under a reader
    that does not model it, and is tampered when the key is spliced in under the old seal.
  - `a_forked_version_without_a_fork_token_is_refused`: a collection with a `forked_from` edge and no `fork` key in
    `id_inputs` is `Error::Invalid` at seal.
- **Step 3.**
  - `object_kind_discriminates_a_zero_block_manifest`: a zero-block manifest is a product, not a collection; both
    keys and neither key are refused.
  - `member_diff_reports_added_removed_repinned_and_order`, plus a property test that applying the diff to `old`
    gives `new`'s members (#947: "`diff` of two collection versions lists exactly the members that differ").
  - `member_diff_refuses_a_duplicate_reference`: a collection listing one `reference` twice, on either side, gives
    `Error::DuplicateMemberReference`.
- **Step 4.**
  - `gc_marks_through_collection_members`: a base and deltas pinned only by a `current` collection survive `gc` and
    read back by hash. Today this aborts with `GcAborted`, the collection read as a manifest (#947 item 2).
  - `gc_marks_through_nested_collections`: three levels, a shared member visited once.
  - `gc_aborts_on_a_missing_member_and_names_its_path`, and the same for a torn and a wrong-kind member: nothing is
    deleted.
  - `a_ref_whose_kind_disagrees_with_its_object_is_refused`: a `collection` record naming a product, and the
    reverse, are `RefKindMismatch` on read and abort `gc`.
- **Step 5.**
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
- **Step 6.**
  - `a_pin_keeps_a_version_alive_until_removed`: pin a version of a lineage, `forget` the lineage and `gc`: the
    version and everything it reaches read back; remove the pin and `gc`: they are reclaimed (kit's added
    acceptance item; the "after compaction" half of that item becomes testable with #906, whose tests must include
    a pinned generation outside the class).
  - `set_pin_refuses_an_unstored_address_and_a_wrong_kind`.
  - `a_pin_and_a_lineage_with_one_name_are_independent`: with a pin `x` set, `commit` and `forget` on lineage `x`,
    and `import` of a product whose `id` is `x` (which names its lineage `x`), leave `refs/pins/x` as it was; removing
    the pin leaves the lineage.
- **Step 7.** trycmd walkthroughs for the CLI verbs (a fork, a merge written as one delta, a drop, a pin), run as
  tests.
- **#987.** The discard probe (kit's second added acceptance item).

## Implementation plan

Ordered, one PR each, each under 1500 changed lines. Step 1 builds on PR #990 (#975), merged as `93fc6716`.

1. **Io: repository layout 2** (T0). The marker bump to `tessera-repository 2` and the refusal of every other layout
   (#990's `Error::UnsupportedRepositoryLayout { found, required }`, reused, with `required` bumped to layout 2),
   `refs/lineages/` and `refs/pins/` with the typed record grammar (product records only at this step),
   `log/lineages/`, and `InvalidRefName` generalising #990's `InvalidLineage`. It removes or retargets #990's
   `refs/pin:` text in the ADR-0036 amendment and its `no_lineage_maps_to_a_file_name_holding_a_colon` test. ADR-0036
   and the book's repository chapter describe the layout. About 900 lines.
2. **Core: `sources` on `Collection`** (T1). The field, `FIELDS`, a builder method, role constants `supersedes` and
   `forked_from`, the `fork`-token check for a forked version, SPEC §4 (collection record) and §8 (versioning covers
   collections), seal-case vectors. Corpus-neutral by test. About 500 lines.
3. **Core: the object-kind discriminator and the member diff** (T2 core half, T7). `ObjectKind::of(bytes)` and
   `collection::diff`. About 600 lines.
4. **Io: collection objects in the repository and `gc` through members** (T2, T3). `put_collection`,
   `get_collection`, `collection` records, the record-object kind check, `walk_read` dispatch by kind, closed
   members, member-path errors. About 1000 lines.
5. **Io: `commit_collection`, `log` and `history`** (T4, T5). CAS, lineage kind checks both ways, the lineage id
   check, member closure, new members in the same call, the walk over `supersedes`/`forked_from`. About 1100 lines.
6. **Io: pins** (T6). `set_pin` / `remove_pin` / `read_pin` / `pins`, `gc` roots over `refs/pins/`. About 500 lines.
7. **CLI** (T8). `tessera collection commit|log|diff`, `tessera pin set|rm|ls`, trycmd walkthroughs as docs, the book
   chapter on repositories. About 1000 lines.
8. **Amendments.** ADR-0036 and ADR-0049 "Amended by ADR-0067" pointers and D9's restatement in ADR-0036. About 100
   lines.

Steps 1, 2 and 3 are independent of each other; 4 needs all three; 5 and 6 need 4; 7 needs 5 and 6; 8 is last. The
#781 ADR can proceed in parallel: it needs D1 to D3 decided, not built.

## References

#947 (this), #781, #946 (PR #973), #975 (PR #990), #292, #288, #945, #906, #591, #971, #987; nerdmachines/kit#214,
nerdmachines/kit#97. ADR-0022, ADR-0033, ADR-0036, ADR-0049, ADR-0050, ADR-0060, ADR-0066. `tessera-io/src/repo.rs`,
`tessera-core/src/collection.rs`, `tessera-core/src/provenance.rs`.
