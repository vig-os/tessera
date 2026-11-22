---
type: issue
state: open
created: 2026-07-02T13:07:09Z
updated: 2026-11-21T20:39:35Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/292
comments: 1
labels: discussion, docs
assignees: none
milestone: 0.1.0-beta
projects: none
parent: none
children: none
synced: 2026-11-22T09:19:47.728Z
---

# [Issue 292]: [ADR: tsra ownership boundary — own the format + proofs, ride OCI+S3 for transport (not DataLad/git-annex as a dependency)](https://github.com/vig-os/tessera/issues/292)

Parent architecture-decision record for the tsra ↔ storage/distribution boundary. Governs #291 (transport crate choice) and the collection ref-model / custodian verbs. Prompted by a direct challenge: *"this sounds a lot like reinventing git-lfs & datalad"* — plus the constraint *"prefer not pulling another dep."* This ADR draws the line so the NIH risk is answerable per-capability.

## Context

Collections, versioning, refs, pull-on-demand, origin tracking, and nested collections **are** git-annex + git-lfs + DataLad patterns (subdatasets = nested collections; `datalad get` = pull-on-demand; annex `whereis` = origin breadcrumb; lfs pointer = reference-not-embed). The question is not *"is it similar"* (it is) but *"what does tsra legitimately own vs. delegate,"* so the versioning/distribution layer doesn't quietly grow into a worse git-annex.

## Decision — the "thin waist"

**Test:** *would delegating this lose a format guarantee or a cryptographic proof? If no → delegate.* Everything that produces verifiable truth or is a function of the format stays in tsra; everything that moves/stores/serves/enforces on bytes goes to existing tooling.

```
OWN (tsra)                INTERFACE (emitted)      DELEGATE (existing tooling)
format · seal · Merkle    the .tsra file           MinIO / S3        (store)
verify · proofs           OCI manifest/index JSON  zot / Harbor      (registry + cache)
prune brain (which bytes) S3 prefix layout         oras / mc / aws   (push/pull/copy)
descriptor builders       ro-crate / DataCite      S3 Object-Lock    (WORM enforce)
range-read adapter        the .sig.json envelope   cosign / sigstore (keyless + log)
                                                   InvenioRDM        (DOI / landing)
```

Everything tsra owns is a **pure function or a proof** — no long-lived server, no storage, no network state. That's the tell the line is right.

### Per-capability
| Capability | Own / delegate | Why |
|---|---|---|
| Build OCI manifest/index for a `.tsra` | own (pure fn) | format contract — delegating loses the spec |
| Compute which byte ranges to fetch (prune) | own | format-intrinsic; nobody else knows the central dir + stats |
| Verify a pulled artifact | own | the proof — only tsra can do it |
| Emit RO-Crate / DataCite / prefix layout | own (pure fn) | descriptors are format-level |
| HTTP PUT/GET, auth, retries, chunked upload | delegate | `oci-client` / `object_store` do it; lose nothing |
| Run the registry / object store | delegate, always | tsra must never *be* a registry or a store |
| Enforce WORM retention | delegate | tsra emits the policy; S3 Object-Lock enforces |
| Pull-through cache on a node | delegate | pure infra (zot) |
| Mint DOIs / landing pages | delegate | InvenioRDM |

## Ride OCI + S3 — not DataLad, not a new runtime dep

- **Ride OCI (registry) + S3 (object store)** as the substrate. In-process via **`oci-client`** + **`object_store`** (see #291) — both Rust libraries, single binary, no external tool required, no reimplementation. `oras`/`mc` shell-out kept as an *optional* minimal-footprint escape hatch.
- **Reject DataLad/git-annex as a runtime dependency** — it's an external runtime (git + git-annex + Python), git-centric, and duplicates the OCI-digest / cosign / InvenioRDM stack tsra already commits to (RFC §10). It stays an *optional bridge* at most, never the backbone.

### "GitHub for tsra" — the OCI mapping
| git / GitHub | OCI | tsra |
|---|---|---|
| object (blob/tree/commit) | content-addressed blob | product / block / collection |
| ref (branch/tag → commit) | tag (`repo:tag` → digest) | lineage-id → latest `manifest_hash` |
| tree / monorepo | image **index** | collection |
| PR checks / attestations | Referrers API | signatures / attestations |
| push/pull | oras push/pull | distribution |

## Trust & freshness rules (load-bearing)

1. **The registry is SSoT for *pointers*, not *truth*.** Trust a tag as a pointer, never as content — every pulled object is still hash-verified against the pinned `manifest_hash`. No parallel location DB; the registry's tags/manifests *are* the ref store.
2. **Verify is offline + mandatory; freshness/origin is online + opt-in.** Open + `verify` must work air-gapped with no network (the archival "verify anywhere" promise). Reachability/"is my copy stale?" is a **separate, deliberate, online** verb — never phone home on open.
3. **`aux/origin.json` breadcrumb** — where *this copy* came from lives in non-sealed `aux/` (ADR-0042), never the seal (sealing a URL breaks content-addressing; same bytes from mirror A/B are the same product). It's a *hint* (where to look), not a *proof* (unsealed → not trusted for integrity). It's the input to the online freshness check.

## Prior-art honesty

- **Own (genuinely not git-annex/DataLad):** the *format* (typed, bit-deterministic, sub-file-addressable, self-verifying, SPEC'd, clinical-FAIR) — annex/lfs/DataLad are format-agnostic (opaque blobs, no determinism, no conformance); and **prune-before-fetch** from intrinsic stats.
- **Borrow, don't rebuild:** version + ref + fetch + provenance + nesting patterns.
- **When to just use DataLad:** whole-file versioned datasets *without* sub-file range-read, stats-pruning, determinism, or cloud-WORM/DOI needs → DataLad is the right tool and tsra is over-engineering. The differentiator only pays off on the query-scale / archival / regulatory axes.

## Consequences / child work

- [ ] #291 — adopt `oci-client` + `object_store`, retire hand-rolled `registry.rs`.
- [ ] Collection **ref-model + custodian verbs**: `collection outdated` (member lineage has a newer version than pinned) + `rebind` (re-pin → new collection version with a diff/consistency proof) + `aux/origin.json` stamping on pull.
- [ ] Collection-as-OCI-index push/pull roundtrip gate.
- [ ] Write the ADR file (`docs/adr/00xx-ownership-boundary.md`) from this issue once accepted.
- [ ] Collections book chapter (deferred until #282 merges).

## Non-goals

- Being a registry, object store, cache, WORM enforcer, or DOI minter — external substrate, forever.
- Any bespoke registry/transport protocol code — if it's not `oci-client` / `object_store` (or an explicit shell-out), it doesn't belong.

## References

RFC §10 (distribution/RDM; DataLad = optional bridge, not backbone) · §14 (normative vs informative; format mandates *contracts* only) · ADR-0033 (collections → 3 projections) · ADR-0036 (versioning objects+refs) · ADR-0042 (non-sealed `aux/`) · ADR-0002 (cloud/concurrency) · #291 · #225 (cloud range-read) · #223 (collection model)
---

# [Comment #1]() by [gerchowl]()

_Posted on November 21, 2026 at 08:39 PM_

**The kit's requests cross the line this issue draws: a note and a proposal** (agent for the owner, 2026-11-21; from nerdmachines/kit#214). The owner asked whether tessera is "just a file format with handlers" or whether a different split of responsibility is better. Nothing below is decided.

**The tension.** This issue says: "Everything tsra owns is a **pure function or a proof** — no long-lived server, no storage, no network state." That was written for transport. Since then these have been requested or built here, and all of them are local state: the repository with refs and `gc` (ADR-0036, `tessera-io/src/repo.rs`), the commit log with replay on open (#905), keyed deltas, `current` and compaction (#781), retention (#906), refs to collections (#947), and a writer lock and durability order (#946). The stream writer's journal already was state.

**Proposal: keep the line, and add one layer below it.**

| Layer | What it holds | State |
|---|---|---|
| Format and proofs (`tessera-core`) | schemas, canonical bytes, seal, Merkle, verify | none; pure |
| Handlers (`tessera-io`) | read and write one product, range reads, which bytes to fetch | per call |
| **Local store** (a crate of its own in this workspace) | objects, refs with a real compare-and-set, `gc`, commit log and replay, overlay, flush, compaction, the keyed read | on local disk, behind a library; one fenced writer |
| Query adapter (`tessera-query`, ADR-0062 §1) | the DataFusion provider for one logical table, over the scan and the store | per query |
| Above tessera (kit, or any other consumer) | workspaces and merge, policy, placement, queries across collections, sync between nodes, every service | servers and network |

The sentence in this issue then reads: tessera owns pure functions, proofs **and an embedded local store; never a server and never network state**. The test stays the one used in the kit's record: if an independent implementation that opens the same directory must know a rule to return the right rows and bytes, the rule is tessera's.

**Why a crate of its own and not `tessera-io`, and why not in the kit:**
- The format crate changes when the format changes; the store changes for durability and concurrency. They have different tests (conformance and goldens against a crash harness) and should not block each other. The Python and WASM bindings should not carry locks and syncs.
- Other consumers need keyed deltas and compaction without the kit (the corpus case in #781's motivation).
- If the store lived in the kit, the recovery rules for a tessera directory would be specified in two repositories.

- ADR-0062 §1 already made this kind of cut: an engine-neutral scan in `tessera-io` and the DataFusion adapter alone in `tessera-query`, so that the crate the wheel links stays free of DataFusion. `tessera-wasm` depends on `tessera-core` only.

**Not checked:** how much of `tessera-io`'s stream writer and journal would move into such a crate, and what that costs.


