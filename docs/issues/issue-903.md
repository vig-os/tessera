---
type: issue
state: open
created: 2026-11-18T20:04:26Z
updated: 2026-11-18T21:12:06Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/903
comments: 0
labels: feature, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-19T09:52:10.087Z
---

# [Issue 903]: [feat(io): keyed read that takes rows from encoded sub-chunks without a scan — set-shaped get_many(keys, columns) with bytes-read counters](https://github.com/vig-os/tessera/issues/903)

Filed by an agent for the owner, 2026-11-18, from the kit storage benchmark (nerdmachines/kit#98 for nerdmachines/kit#97). Feature request; the measurements were posted as ADR input on #780 and #781 and are repeated here only as far as the request needs them.

## Motivation

kit keeps app state (settings, layouts, tasks, graph edges, document blocks) in keyed tessera collections and has no row store beside them (owner direction 2026-11-18, nerdmachines/kit#97). Its dominant read is "give me these rows by key, these columns". That read has to cost microseconds, not a millisecond.

## What already exists

- #780 declares keys and proposes a key → (product, leaf, row range) index. Its acceptance measures rows read.
- ADR-0062 / #654 give a streaming scan with projection, filter pushdown and chunk pruning. Every read goes through a Vortex scan.
- `SPIKE-RESULTS.md` measured 500 random takes in one call on numeric columns (flat from 2.7 M to 100 M rows). It did not measure a single-key read of a full row with strings.

## The gap

Finding the row is cheap; fetching it through a scan is not. Per lookup on one 1,048,576-row leaf, position already known (harness on Vortex 0.93.0 from crates.io, default writer settings, data hot):

| Read | `scan().with_row_indices()` on the file | Same scan, file in memory | Take on the encoded arrays in memory |
|---|---:|---:|---:|
| One small integer column | 109 µs, 249 B read | 72 µs | 0.24 µs |
| The 300-byte string column | 1,000 µs, 1.9 MiB read | 518 µs | 3.7 µs |
| Full row, six columns | 1,182 µs, 4.3 MiB read | 751 µs | 5.7 µs |

- A scan call costs about 70 µs whatever it reads, and it fetches one whole segment per projected column.
- A take on the arrays in their Vortex encodings (`RepeatedArrayProbe::execute_scalar` in `vortex-array`) returns the same values: 12,288 of 12,288 scalars matched the oracle, compared as normalised display strings (a typed comparison is pending, nerdmachines/kit#113). The encoded arrays took 0.78 of the memory of the decoded Arrow arrays.
- With 256 keys in one call, two small columns cost 8.7 µs and 14 KiB per row through the scan, so batching spreads the fixed cost.

## Proposal

1. **A set-shaped keyed read on a keyed table:** `get_many(keys, columns)`.
   - Sort the keys, descend the `{hash, stats}` hierarchy (ADR-0028) by key range to the sub-chunks that hold them, and fetch each needed sub-chunk once per projected column.
   - Take the rows from the fetched sub-chunk in its encoded form. No scan is built per call.
   - A single-key read is the same call with one key. There is no separate row-at-a-time path.
2. **A bounded cache of fetched sub-chunks,** keyed by block digest and position, verified against the hash path once on entry (the verify-once rule of ADR-0062 §7). Its bound is in bytes and is a declared setting.
3. **Counters on every call:** bytes fetched, sub-chunks fetched, cache hits. #780's acceptance would then show bytes read beside rows read.
4. **The DataFusion provider (#654) uses the same path** for a keyed join or an `IN (…)` list on the declared key, in place of a filtered scan.

This is a reader-side change. Nothing sealed changes.

## Acceptance

- [ ] `get_many` returns the same rows as a filtered scan, for every table shape in the conformance corpus that declares a key.
- [ ] A lookup whose sub-chunks are cached issues no I/O (instrumented), and its time is reported in the performance book beside the scan path. The harness figure to compare with is 5.7 µs per full row.
- [ ] A cold lookup fetches at most one sub-chunk per projected column (instrumented bytes).
- [ ] A sub-chunk that fails its hash check is never returned and never cached.
- [ ] The cache stays within its declared bound under a workload larger than the bound.

## Open

- Whether an in-memory overlay of unflushed commits (#905) is served by this same call or by the embedding application. kit needs read-your-write either way.
- Whether Vortex's public API lets a reader fetch and hold one sub-chunk without a scan at the fork pin; the harness scanned each column once at setup and probed afterwards.

## Not measured

The fork pin and the canonical codec profile; cold reads and object storage; tables above 10⁶ rows for the in-memory arm; real data (the test strings are random).

## Related

#780 (keys, index), #781 (deltas), #904 (small-row profile: sub-chunk size), #905 (commit log), #654 / ADR-0062 (provider), ADR-0028 (hierarchy), ADR-0054 (caches). Harness: `bench/vortex-delta/` on nerdmachines/kit#98, stages 1c, 1d and 1f.

