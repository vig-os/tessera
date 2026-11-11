---
type: issue
state: open
created: 2026-11-11T01:18:54Z
updated: 2026-11-11T01:18:54Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/818
comments: 0
labels: feature, python
assignees: none
milestone: none
projects: none
parent: 799
children: none
synced: 2026-11-11T08:50:32.505Z
---

# [Issue 818]: [feature(py): a streaming writer in tessera-py for the Python Stage 2 path (ADR-0065 §13.3)](https://github.com/vig-os/tessera/issues/818)

Follow-up from ADR-0065 §13.3 (#800). Toward #799 / #803.

The surveyed Stage 2 pairwise path (`geddf_analysis/pairwise.py`, MorePET/geddf-analysis `d6cd7857`) and its coincidence writer are **Python** (h5py + numpy), not C++. They need a **streaming** writer in `tessera-py`; today `tessera-py` has only the batch `Builder` (`tessera/crates/tessera-py/src/lib.rs:281`), which holds every encoded payload in RAM (`:283`).

Scope (design first, once the owner's consumer surveys report): expose `WriteSession`/`StreamWriter` table streaming to Python with the same sequence-key and schema contract as ADR-0065 §7/§8, so a Python and a C++ producer seal identical products for identical keyed batches.

