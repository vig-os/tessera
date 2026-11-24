---
type: issue
state: open
created: 2026-11-23T16:32:11Z
updated: 2026-11-23T16:32:11Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/962
comments: 0
labels: feature, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-24T11:59:40.063Z
---

# [Issue 962]: [feat(io): declared-loss seal for sequenced lanes — record a lost seq in provenance instead of refusing the table](https://github.com/vig-os/tessera/issues/962)

Context: owner design discussion on 2026-11-23 about the DAQ data path for PETsys and glint (nerdmachines/glint#421, nerdmachines/kit #92 and #115). This is one of the kit-facing requests indexed in #945.

## What exists

`SequencedTable` (ADR-0065 §7.2–§7.3, #812) already does most of what a DAQ needs:
- **Empty slices are first-class:** a `seq` pushed with no rows is the same as an empty batch.
- **Order:** batches are sealed in `seq` order whatever order they arrive in.
- **A missing `seq` is refused:** `close` fails with `StreamedTableError::Missing { seq }`, and a producer that lost a `seq` can only `abort`.

## Problem

In a DAQ, a time slice can be **lost for real**: a UDP retransmit window expires (glint#421), or a DMA frame-loss flag is set by the card (PETsys). The run must still seal, because the other slices and lanes are valid data. It must also record **which slices are missing and why**, so analysis can tell "no hits" (empty `seq`) from "lost" (declared `seq`). Today the choice is to refuse the whole table or abort, and both discard good data.

## Proposal

- A **declared-loss entry:** `SequencedTable::declare_lost(seq, reason)`, which fills the `seq` with no rows and records `(seq, reason, declared_at)`.
- `close` accepts declared-lost `seq`s as present. Undeclared gaps are still refused, so the strict default stays.
- The sealed lane carries its **loss record** in provenance (`Generation`) or as a companion block: lost `seq`s with reasons, plus counts. It's covered by `content_hash`, so two runs with different losses never share an id.
- An optional **policy in the writer config:** a maximum number or fraction of lost `seq`s per lane before `close` refuses anyway. `preflight()` (#909) reports the policy.
- Taps (#794 / #908) can surface declared losses live, so the operator sees loss as it happens.

## Acceptance

- [ ] A lane with declared-lost `seq`s seals; the loss record round-trips through read; an undeclared gap is still refused.
- [ ] `content_hash` differs between the same rows with and without a declared loss (property test).
- [ ] Empty `seq` and lost `seq` stay distinguishable after seal.
- [ ] The policy cap refuses at close with a typed error naming the lane and count.

Related: #203, #812, #909, #794, #908, #945.
