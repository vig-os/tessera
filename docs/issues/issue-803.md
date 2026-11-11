---
type: issue
state: open
created: 2026-11-11T01:02:45Z
updated: 2026-11-11T01:26:03Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/803
comments: 1
labels: feature
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-11T08:50:40.842Z
---

# [Issue 803]: [chore: port downstream readers (gPET-sim2listmode, Stage 2 / geddf-analysis) to tessera logical APIs](https://github.com/vig-os/tessera/issues/803)

Sub-issue 4 of #799. Tracking; scope from the owner's two consumer surveys; work lands in the consumer repos.
---

# [Comment #1]() by [gerchowl]()

_Posted on November 11, 2026 at 01:26 AM_

Reader-port inventory: §6 of [801-consumer-gap-analysis.md](https://github.com/vig-os/tessera/blob/docs/801-consumer-gap-analysis/docs/spikes/801-consumer-gap-analysis.md#6-reader-ports-803) (PR #820).
1. gPET-sim2listmode (C++) is **S**: `SimulationDataManager.cpp` holds over 90 % of the read surface. It is blocked on a C ABI read side (#805), since #800 is writer-only.
2. gPET-readout is **S**: one module (`g4ring_io.py:116-133`).
3. gPET-pipeline is **M**: its HDF5 post-condition validator becomes `tessera schema`.
4. geddf-analysis is **M** to read (it needs row-range reads #807 and the nullable fix #804) and **L** to also write `coincidences.h5`. gPET-reco also reads `coincidences.h5` and is not inventoried; it's a question for the surveys.


