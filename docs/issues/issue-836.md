---
type: issue
state: open
created: 2026-11-11T04:18:32Z
updated: 2026-11-11T04:18:32Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/836
comments: 0
labels: feature, area:core
assignees: none
milestone: none
projects: none
parent: 799
children: none
synced: 2026-11-11T08:50:24.280Z
---

# [Issue 836]: [feature(core): categorical code tables on the column contract (G13; deferred from ADR-0065)](https://github.com/vig-os/tessera/issues/836)

Deferred from ADR-0065 §8 (#800) and gap G13 of docs/spikes/801-consumer-gap-analysis.md. Toward #801.

Enum / categorical columns (`hit_type`, `source_type`, `source_photon_id`, coincidence labels) have no code-table slot on `Column` (`tessera-core/src/block/table.rs:12-71`; `tessera-io/src/table.rs:97-99` points at "a categorical descriptor — a schema choice" that doesn't exist). Until this lands, the C ABI carries such columns as their integer codes (clean lane), and refuses Arrow `Dictionary` types.

Scope (design first; waits on the owner's survey question 5 in the gap analysis §7): a code-table descriptor on the schema's column contract (code → label, optional vocabulary), sealed with the schema, corpus-neutral (skip-serialising when absent), validated at the bridge (a code outside the table is refused).

