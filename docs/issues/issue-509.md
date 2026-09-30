---
type: issue
state: open
created: 2026-09-29T10:48:07Z
updated: 2026-09-29T10:48:07Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/509
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-30T07:57:08.473Z
---

# [Issue 509]: [The arrow lane cannot be built without the parquet lane](https://github.com/vig-os/tessera/issues/509)

## The break

`cargo check -p tessera-ingest --no-default-features --features arrow` does not compile:

```
error[E0433]: cannot find `parquet_table` in `crate`
   --> crates/tessera-ingest/src/engine.rs:774:28
    |
774 |                     crate::parquet_table::read_arrow_table(input, exclude)?,
```

The **Arrow IPC** arm reads through `crate::parquet_table::read_arrow_table`, but `parquet_table` is
`#[cfg(feature = "parquet")]`. So the `arrow` lane cannot be built without the `parquet` lane, even though
`arrow` is a capability feature in its own right and `parquet = ["arrow", "dep:parquet"]` declares the
dependency in the other direction.

## Not a regression

Pre-existing on `dev` (verified: same call at `engine.rs:709`, same module gate at `lib.rs:44`). It has
simply never been built — every gate that touches the ingest lanes enables `parquet`, so the one
configuration that would expose it is the one nobody runs. Found while adding
`ingest-gate-a-npy-only` for #386, which compiles reduced configurations on purpose.

## Why it matters beyond tidiness

ADR-0057 §5 wants *which decoders could have produced this artifact* to be a build-time fact, and the
feature set is how that is expressed. A lane that cannot actually be selected alone weakens that: the
Arrow IPC reader is not independently gateable, so "this build has the arrow lane and not the parquet lane"
is currently unrepresentable. It also means the arrow lane's `ingest_decoder` digest describes a
configuration no one can build.

## Fix options

1. **Move `read_arrow_table` into `arrow_table`** (which is `#[cfg(feature = "arrow")]`) and leave the
   Parquet-specific readers in `parquet_table`. The function reads an Arrow IPC/Feather file; it is in the
   Parquet module by history, not by dependency.
2. Widen the module gate to `any(feature = "arrow", feature = "parquet")` and gate the Parquet-only items
   inside it. Smaller diff, muddier module boundary.
3. Declare the coupling honestly — `arrow = ["dep:parquet", …]` — which is the wrong direction: it would
   put a Parquet reader in a build that asked for Arrow IPC, and the ADR-0056 §6a digest would then pin
   `parquet` on the IPC lane, re-creating the cross-lane over-claim #477 removed.

(1) looks right. Whichever is chosen, add `--features arrow` to a gate so it stays compiled — the reason
this survived is that no configuration exercised it.

Refs: #386

