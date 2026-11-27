# Repositories: collections, lineages and pins

A repository stores immutable, content-addressed objects. A lineage names its latest version; a pin
names an explicitly retained version. Product and collection lineages use the same repository and
writer lock. Collections pin ordered members by their exact manifest seals, including nested
collections. Changing a collection writes a new collection object and reuses unchanged members.

## Layout and initialization

`tessera init REPO` creates repository layout 2. The `layout` marker contains
`tessera-repository 2`; objects live under `objects/`, lineage tips under `refs/lineages/`, pins under
`refs/pins/`, and rebuildable lineage logs under `log/lineages/`. Each ref records a kind and seal.
Names are percent-encoded injectively; empty names and names exceeding 249 encoded bytes are refused.
A lineage and pin may have the same name independently.

Other layouts are refused. There is no in-place migration: export products with the binary that wrote
them, then import into a newly initialized repository. Never run older repository tools on layout 2.

## Committing collections

First import sealed products with `tessera import REPO FILE.tsra`. Build a sealed collection using
`collection new`, an ingest spec, or the library's `CollectionBuilder`. Commit it with:

```sh
tessera collection commit REPO catalog/collection.json
```

The collection's `id` names its lineage. Omitted `--expected` asserts that no tip exists. For the next
version, retain the identity inputs and add exactly one `supersedes` source referencing the parent's
id and pinning its manifest seal. Supply the observed parent seal explicitly:

```sh
tessera collection commit REPO next/collection.json \
  --expected blake3:PARENT --member delta.tsra
```

Here `blake3:PARENT` stands for the complete observed seal. Repeat `--member` for new product files,
and `--subcollection` for sealed collection descriptors. Supply every unstored member and its closure;
there is no implicit sibling-file discovery. Members may be supplied in any dependency order.
Products, their blocks, sub-collections and the new tip are stored in one fenced call, preventing GC
from reclaiming a new delta before it becomes reachable. Existing reachable members need no flags.

The CLI accepts already sealed collection versions; it does not choose fork identities, manufacture
version edges, or decide how to merge. The library validates these before storing the collection.
Loading supplied files can fail before the repository begins its locked validation. Memory use includes
all supplied encoded product payloads until the atomic call completes.

## Logs, ancestry and diffs

`collection log REPO ID` prints the lineage cache **oldest first**, marking the tip. With
`collection log REPO SEAL --history`, it prints distinct reachable versions in depth-first preorder:
the requested version first, following `supersedes` before `forked_from`, preserving source order
within each role. Shared ancestors appear once. Each record includes kind, lineage id and labelled
predecessor seals. Members and derivation sources are outside this ancestry walk. A missing or
unreadable predecessor refuses the complete history request; no partial history is printed.

`collection diff REPO OLD_SEAL NEW_SEAL` compares immediate members by reference. Additions follow
the new order, removals the old order, and repins the new order. `order_changed` reports a change in
relative order of shared references. A repin includes a seal or kind change. Duplicate references are
refused. This diff excludes metadata, member roles, derivation edges and nested member contents;
an empty diff does not mean the collection seals are equal.

Plain lineage log takes no reader lock: the tip and log cache are separate observations. A concurrent
commit may add entries newer than the observed tip; `(tip)` marks the initial observation, and a cache
without that entry has no marker. Concurrent forget/GC can cause a read error or an empty log. Do not use
this display as an atomic concurrency check; supply an observed seal to a CAS write instead.

## Forks, merges and dropping work

A fork is a second lineage. The caller retains the main version's ordered members, adds a
`forked_from` source pinning its seal, and declares a non-empty caller-chosen `fork` token in
`id_inputs`. Token uniqueness belongs to the caller. Different tokens yield distinct identities;
reuse can collide locally and is refused by CAS. Later versions retain that token and identity.

Merge policy belongs to the application. It writes its merged delta and commits an ordinary new
version on the main lineage, with that delta supplied atomically. Tessera implements neither branches
nor content merge. `forget REPO ID` drops a lineage's tip and log; `gc REPO` reclaims unreachable objects.
The fork's origin edge keeps the main ancestor alive while the fork remains reachable.

## Pins and garbage collection

```sh
tessera pin set REPO merge-base blake3:VERSION --kind collection
tessera pin ls REPO
tessera pin rm REPO merge-base --expected blake3:VERSION
```

Use a complete stored seal in place of `blake3:VERSION`. `--kind product` pins a product instead.
Pin creation requires absence unless `--expected` supplies the old target seal. Replacement may
change kind; the declared kind is checked against the stored object. Removal also uses CAS: omitting
`--expected` only matches an absent pin. Pins are sorted by name when listed and survive `forget`.
Each `pin ls` row is a JSON string naming the pin, a space, the kind, a space, and the full seal.
Names are sorted by their logical values; quotes, backslashes and ASCII control characters are escaped.
Keep a lineage or another pin until a new pin has landed, so concurrent GC cannot reclaim its target.

GC roots include every lineage and pin. GC follows all stored source edges, including `supersedes`
and `forked_from`, and recursively follows every collection member. An unavailable source edge may
refer to an external version; an unavailable member aborts GC before deletion. A pin keeps its target
and everything reachable from it alive. Removing a pin permits reclamation only when no remaining
root reaches those objects. Removing an old pin while its lineage still reaches it reclaims nothing.
GC scans all roots and lists the repository's objects; discard cost grows with repository size.
Retention policy and WORM enforcement are outside these commands (ADR-0067 D7–D8).

## Refusals and exit codes

One stable CLI map classifies the outer library error, so scripts can choose an action without parsing
stderr. Codes 4–6 extend the existing 0–3 range without collisions:

| Code | Class | Action and errors |
|---|---|---|
| 0 | Success | The operation succeeded. |
| 1 | Other failure | Other errors, including ordinary invalid input and I/O failures. |
| 2 | Usage | Correct the command syntax rejected by clap. |
| 3 | Backend unavailable | Use a build containing the requested backend. |
| 4 | Conflict | `RefMoved`: reread the tip or pin, rebuild or reconsider the change, then retry with CAS. |
| 5 | Rule refusal | Correct kind, member closure, fork token, lineage id, supersedes edge, or ref name. |
| 6 | Store damaged or unsupported | Repair corrupt refs/targets or aborted walks; use a supported layout. |

Rule refusals cover requested lineage/object/member kind mismatches, unstored new lineage or pin targets, missing
product blocks or collection members, missing/empty fork tokens, lineage id or supersedes mismatches,
and invalid ref names. Stored ref kind disagreements and absent/torn ref targets are damage (6), including
when a write discovers them while reading the existing ref under its lock. Write-time pin kind checks use
`ObjectKindMismatch` (5); read-time disagreements use `RefKindMismatch` (6). `RefTargetUnreadable` retains
read context; `UnstoredLineageTarget`, `UnstoredPinTarget` and `MissingProductBlock` retain write context, without guessing from
I/O messages or making a separate preflight read. The exhaustive classifier lives beside `Error` in
`tessera-core`: a new error variant cannot compile until assigned a class. The CLI assigns numeric codes
to that closed class set. `MissingForkToken` classifies absent, empty and whitespace-only fork tokens
as rule refusals without inspecting diagnostic text. `Invalid` errors stay 1.
Classification is by the outer error: a torn member
refuses a commit with 5, while damage aborting a GC or history walk returns 6. Never blindly retry a rule
refusal or store damage. A lineage's kind and id cannot change; pin targets must be stored with the declared kind.
A log-cache append failure can be reported after the tip has advanced; inspect the ref before retrying.

## Tested walkthroughs

The test harness builds sealed fixtures through public APIs and replaces `@...@` tokens with their
exact identities. `main`, `fork`, `next` and `merged` are those sealed catalog directories; `base.tsra`
and `delta.tsra` are their product files. The transcripts execute real commands and assert exit codes.

{{#include ../../../tessera/crates/tessera-cli/tests/cmd/repositories/lifecycle.trycmd}}

{{#include ../../../tessera/crates/tessera-cli/tests/cmd/repositories/refusals.trycmd}}
