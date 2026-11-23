---
type: issue
state: open
created: 2026-11-21T11:09:22Z
updated: 2026-11-22T22:57:10Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/946
comments: 1
labels: bug, area:io, area:integrity
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-23T10:16:29.945Z
---

# [Issue 946]: [fix(io): the repository's ref advance is not an atomic compare-and-set, refs and objects are written without rename or sync, and gc can race a commit (ADR-0036 §2)](https://github.com/vig-os/tessera/issues/946)

Filed by an agent for the owner, 2026-11-21, from the kit's storage review (nerdmachines/kit#214, nerdmachines/kit#97). Found by reading `dev` at `4863a78`. **Nothing was built or run**, so every finding below is a reading of the code and wants a failing test first.

## Motivation

`refs/<lineage>` is the only mutable state in a repository (ADR-0036 §1). ADR-0036 §2 promises that the ref advances by "compare-and-set on the prior value, so concurrent commits to one lineage are rejected, not silently lost", and `repo.rs` repeats it in its doc comments.

kit plans to advance a `current` collection once per flush (#781, #905), from several writer processes on one node (people and agents). The reference case for what goes wrong here is cx: its worst storage incident was a lost compare-and-swap, where 20 of 120 writes reported success and were not in the graph (nerdmachines/kit#214). So before kit's plan leans on this ref, the kit review read the module that implements it.

## What already exists

- `tessera/crates/tessera-io/src/repo.rs`: `Repository` with `write_raw` / `write_object`, `read_ref`, `set_ref(lineage, expected, new)`, `commit`, `forget`, `gc`. Seven tests, among them `set_ref_rejects_a_moved_tip` and `forget_then_gc_reclaims_exclusive_keeps_shared`; all are single-threaded and none kills the process.
- `tessera/crates/tessera-cli/src/version.rs`: the `commit`, `import`, `forget` and `gc` verbs.
- #288 already says "tessera's CoW versioning (ADR-0036) is single-writer" and proposes to study Icechunk's conditional write. Nothing in the code enforces single-writer.
- The same class of defect was fixed for `WriteSession` in #695 (directory entries not synced) and #686 (non-atomic journal rewrite).

## The gap

1. **`set_ref` is not a compare-and-set.** It reads the ref (`repo.rs:192`), compares, and then writes (`repo.rs:202`). The check and the write are two steps with no lock between them. Two processes can both read the same tip, both pass the check and both write; one commit is lost without an error.

2. **`commit` compares the tip against itself.** `Repository::commit` reads the tip (`repo.rs:298`) and passes that same value as `expected` one line later (`repo.rs:299`). The caller's view of the parent never reaches the check. The CLI reads the tip at `version.rs:68`, builds a manifest whose `supersedes` edge names that tip (`version.rs:118`) and then calls `repo.commit` (`version.rs:121`). If another commit lands in between, the ref moves from the other commit to this one, whose `supersedes` edge still names the older tip. The other commit is no longer reachable from the ref, and the next `gc` deletes it. This needs no race in the narrow sense, only two overlapping invocations.

3. **Refs and objects are written in place and never synced.** `write_raw` (`repo.rs:98`) and `set_ref` (`repo.rs:202`) call `std::fs::write` on the final path: no temporary file and rename, no file sync, no directory sync. After a crash or a power cut:
   - a ref can be empty or partial. `all_ref_tips` skips an empty ref (`repo.rs:331`), so a following `gc` treats that lineage as having no tip and reclaims every object only it reaches;
   - an object can be truncated at its own address. `write_raw` returns early when the path exists (`repo.rs:92`) and `has_object` is an existence test (`repo.rs:107`), so committing the same bytes again does not repair it. `read_object` does catch it by hash, but the block stays unreadable until someone deletes the file by hand;
   - the ref can survive while the objects it names do not, because nothing orders their durability.

4. **`gc` and `commit` do not exclude each other.** `commit` stores objects (`repo.rs:278`) and checks that every block exists (`repo.rs:288`) before it advances the ref (`repo.rs:299`). A `gc` that runs in between sees those objects as unreachable and deletes them (`repo.rs:414`); the ref then advances to a manifest whose blocks are gone. The same holds for a commit that reuses a block which is present but currently unreachable.

5. **`gc` treats "cannot read" as "has nothing to keep".** When a manifest on the walk fails to load, the walk continues (`repo.rs:393`, "leave it for the sweep"). If that manifest is a ref tip or an ancestor, its blocks and everything behind it are not marked and are deleted. A transient read error, or an object the reader refuses (ADR-0066 rule 10), therefore turns into data loss.

6. **For #781 (a requirement, not a defect):** the repository stores product manifests only. A `current` collection that pins a base and its deltas by `manifest_hash` cannot be the target of a ref today, and `gc` marks only `blocks` and `sources` of product manifests (`repo.rs:395-406`), so collection members would not be kept alive.

## Proposal

1. `commit` takes the parent the caller built on (`expected: Option<&str>`) and refuses when the ref differs, with a typed error (for example `Error::RefMoved { lineage, expected, found }`) in place of `Error::Invalid`. It also refuses a manifest whose `supersedes` edge does not name `expected`.
2. A real compare-and-set. On a local filesystem: one exclusive writer lock for the repository (or one per lineage) held across check and write; the ref written to a temporary file, synced, renamed, and the directory synced. On an object store: a conditional put, which is the mechanism #288 names. Say in the ADR which of "one fenced writer per repository" and "one per lineage" is meant.
3. Objects are written to a temporary file in the same directory, synced, renamed, and the directory synced, all before the ref names them. One sync per commit batch is enough; the per-commit cost for small commits stays with the commit log of #905.
4. `gc` takes the writer lock, or runs in two phases (mark, then sweep only objects older than the mark). A ref or manifest that cannot be read aborts the run with an error. Nothing is swept on a partial walk.
5. An empty or unparsable ref is an error on open, never "no commits yet".
6. Refs to collections and marking through collection members, scheduled with #781.

## Acceptance

- [ ] A test with N processes committing to one lineage: every commit that returned `Ok` is reachable from the ref by `supersedes`, and every other one returned the typed error.
- [ ] A test for finding 2: a commit built on tip A, attempted after the ref moved to B, is refused.
- [ ] A kill-point test (or the existing fault harness) at each step of `commit`: after reopening, the ref names either the old or the new version, and every object the ref reaches reads back by hash.
- [ ] A truncated object at its address is detected and replaced on the next write of the same bytes.
- [ ] `gc` running concurrently with `commit` never removes an object that a returned commit references.
- [ ] `gc` over a repository with one unreadable manifest deletes nothing and reports the manifest.
- [ ] ADR-0036 states the writer model (one fenced writer, or serializable multi-writer per #288) and the durability order.

## Not checked

- Nothing was compiled or run; the line numbers are from `dev` at `4863a78`.
- `publish`, `seal`, `diff` and the redacting commit (#591) were not read.
- A search for file locking in `tessera-io/src` and `tessera-cli/src` found none; other crates were not searched.
- The object-store form of the repository ("or, later, an object-store prefix") is not in the code.

## Related

ADR-0036 §1, §2 and "Still open"; #288, #781, #905, #906, #591, #695, #686, #945. nerdmachines/kit#214 (what cx learned about the same path), nerdmachines/kit#97, nerdmachines/kit#219 (filesystem check when a store opens).

---

# [Comment #1]() by [gerchowl]()

_Posted on November 22, 2026 at 10:57 PM_

Item 5 is now confirmed by a probe, not only by reading. The fresh review of PR #942 (merged as dde0517f) planted an orphan-index tip on a lineage with history. `gc` then reported `scanned 5 · kept 1 · reclaimed 4` and exited 0 with no warning, and it deleted the older versions and the data block. Since #942, a manifest holding a dangling chunk index is refused on read, which widens what reaches the `Err(_) => continue` at `repo.rs:393`. Filed separately as #955, now closed as a duplicate of this issue.

