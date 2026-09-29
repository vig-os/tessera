---
type: issue
state: closed
created: 2026-07-07T08:37:56Z
updated: 2026-09-28T17:55:09Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/343
comments: 1
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:47.490Z
---

# [Issue 343]: [feat(ingest): apply GEDDF column dictionary to ALL listmode tables — singles/coin/time-markers/coin-counters are still bare](https://github.com/vig-os/tessera/issues/343)

## Problem

The GEDDF column dictionary (units/scale/short_name/description) from #307/#310/#331 is applied **only to the terminal `events-*` tables**. The intermediate listmode products ship with bare, ambiguous columns:

```
singles_0000    ms u4 · en u2 · ax u1 · tx u2 · td u2         ← no units, no scale, no desc
coin_3p_0000    ms u4 · en_0 u2 · dt_0 u2 · ...  (12 cols)    ← keV? ADC? which pair?
time_markers    tm u4 · idx u8 · length ...                   ← 'tm' vs events' 'ms' — inconsistent
coin_counters   tm u4 · idx u8 · cc u4                        ← 'cc' = coincidence count per window?
```

A scientist opening `singles.tsra` alone cannot tell whether `en` is keV, ADC counts, or MeV. Naming isn't even consistent with the events tables (`tm` vs `ms`).

## Proposal

Extend `apply_geddf_dictionary` (tessera-ingest/src/ge_hdf5.rs) to annotate the intermediate tables, keyed by their `dataset_group` (singles/coin/time-markers/coin-counters), reusing the same GEDDF dictionary source of truth (`docs/dictionaries/ge-discovery-mi-listmode.toml`). Add the missing group definitions to the dictionary. Harmonize the timestamp column name (`tm` vs `ms`) or document why they differ.

Found during: DUPLET first-user review — only `events-*` carried the dictionary; all upstream tables were bare.
Follow-on to #307 (mechanism, done), #310 (quantize), #331 (nested-path fix).
---

# [Comment #1]() by [gerchowl]()

_Posted on September 28, 2026 at 05:55 PM_

Closing as **done** — verified on `origin/dev` in the 2026-09-28 backlog triage.

Evidence: ADR-0058 accepted (docs/adr/0058 status line lists #343 as implemented) + supporting ab83ba9/2e3a2ff.

https://claude.ai/code/session_01XdERKMVDAwfMJSKdTytNnK

