---
type: issue
state: open
created: 2026-11-18T20:04:28Z
updated: 2026-11-18T20:04:45Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/904
comments: 0
labels: feature, area:io, adr
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-19T09:52:09.692Z
---

# [Issue 904]: [feat(format): a small-row table profile — declared sub-chunk and leaf sizes, a key-defined split rule and a delta chain limit (follow-up to #869)](https://github.com/vig-os/tessera/issues/904)

Filed by an agent for the owner, 2026-11-18, from the kit storage benchmark (nerdmachines/kit#98 for nerdmachines/kit#97). Feature request, follow-up to #869.

## Motivation

#869 decided that a table profile is a named set of declared constants, and that a second profile for collections of small, frequently changed rows is allowed. Its last line made that profile conditional on measurements: "only worth defining if key lookup and small updates meet their budgets on Vortex leaves plus delta leaves". Those measurements now exist. They show which constants matter and that the current ones are wrong for small rows.

## What already exists

- `BLOCK_ROWS = 2²²` and `ROWS_PER_GROUP = 2¹⁶`, frozen for format compatibility.
- A sealed `TableSpec.edition`, and the #869 decision to record a codec-profile id beside it.
- #828 and #796 propose profiles for codecs (per-column, archival). They do not cover layout constants.
- #781 proposes compaction; its comments carry the split-rule measurements.

## The gap

Measured on 10⁶ rows of about 350 bytes (harness on Vortex 0.93.0 from crates.io, not the fork pin):

- **Sub-chunk size sets the cost of a cold keyed read.** Vortex's default is 8,192-row blocks with a 1 MiB minimum per segment. A full-row read fetches 4.3 MiB, and a narrow column in a large leaf is read whole (375 KiB for one 3-bit value).
- **Smaller sub-chunks trade scan speed for read size.** With 1,024-row blocks and 16 KiB segments, a full-row read fetches 1.3 MiB, a six-column scan takes 86 ms where it took 23 ms, and the file grows 10.6 %. Smaller sizes were not tried.
- **Splitting leaves by row count defeats block sharing after any insert or delete.** After a compaction that folds one insert-only delta, 0 % of the downstream leaves are shared with the old base under a row-count split and 87.5–100 % under a key-defined split. At 4,096-row leaves that is 212 MiB of new leaves against 7 MiB, for about 3.5 MiB of new rows.
- **A merge-on-read scan grows linearly with the number of deltas:** 3.7 MiB with none, 26 MiB through 8, 1.7 GiB through 600.

## Proposal

Define one small-row profile as declared constants, sealed by its profile id (#869):

1. **Rows per sub-chunk and minimum segment bytes,** chosen from a sweep on the fork pin that goes below 1,024 rows.
2. **Leaf size bounds** (minimum and maximum rows per leaf), far below `BLOCK_ROWS`.
3. **A key-defined split rule** for keyed, sorted tables: cut after a row whose key hash matches a declared pattern, within the leaf size bounds. It is a pure function of the rows and the constants, so a compacted table is canonical for its row set.
4. **A delta chain limit:** the number or total size of deltas above which a logical read may refuse or a compaction is due. The value follows from the merge-on-read cost above.
5. **Whether the columns of one sub-chunk are stored next to each other,** so that a full row is one fetch and not one per column. This is a question to settle by measurement, not a requirement yet.

The existing profile stays the default and its constants do not change.

## Acceptance

- [ ] The same input with the same edition and profile gives the same bytes across debug and release builds and across architectures, gated in the determinism corpus for the new profile.
- [ ] A product written with the profile records its id, and a reader refuses an id it does not know.
- [ ] The performance book has the sweep: bytes fetched for a cold full-row read, scan time and file size, per sub-chunk size.
- [ ] After a compaction that folds a clustered insert-only delta, the share of downstream leaves kept by digest is reported, and is far above the row-count rule's 0 %.
- [ ] A logical read over more deltas than the limit behaves as the profile declares.

The budgets themselves (bytes per cold read, share kept) are the owner's to set once the sweep is in.

## Open

- `ROWS_PER_GROUP` is frozen today. Letting it vary by profile is a format event, to be sequenced with corpus regeneration (noted on #869).
- Leaf names and `Partition.index` churn when a middle leaf splits or merges under a key-defined rule (first comment on #781).
- The harness found that the crates.io Vortex writes different bytes from debug and release builds. #788 reports the fork as validated; the new profile's shapes should be added to that check.

## Related

#869 (profile id), #903 (keyed read), #905 (commit log), #906 (retention), #781 (deltas, compaction, split-rule measurements), #780 (keys), #828 / #796 (codec profiles), ADR-0024, ADR-0028, ADR-0064. Harness: `bench/vortex-delta/` on nerdmachines/kit#98, stages 1b and 1c.

