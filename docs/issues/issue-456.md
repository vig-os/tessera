---
type: issue
state: open
created: 2026-09-28T20:04:24Z
updated: 2026-09-29T06:53:07Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/456
comments: 0
labels: none
assignees: none
milestone: 0.1.0-beta
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:30.982Z
---

# [Issue 456]: [collapse the vendor ingest verbs onto the primitive axis (ADR-0056 §4)](https://github.com/vig-os/tessera/issues/456)

ADR-0056 §4 decides that the CLI names verbs on **one** axis — the primitive — and that vendor decoding becomes a `--from` backend plus its scoped flags:

```text
tessera ingest table <FILE> [--from parquet|arrow|csv|ge-hdf5]
tessera ingest array <FILE> [--from npy|nifti|dicom|dicom-series|raw|tiff]
tessera ingest blob  <FILE>
```

#386 added `ingest table` (and `ingest array` in its follow-up) with the generic `--from` values only, and did **not** touch the existing `dicom` / `dicom-series` / `ge-hdf5` / `blob` verbs — that migration is CLI-surface churn across trycmd walkthroughs, the book, and the shipped example TOMLs, and it does not belong in the same change as the feature itself.

What is left:

- vendor `--from` values routed on the primitive verbs
- the vendor verbs `#[command(hide = true)]` for one release cycle with a stderr notice naming the replacement invocation, then deleted
- `raw` folded into the array primitive. **Note the coupling:** `raw` currently produces a `recon` product and `docs/examples/migrate-petct-study.toml` relies on that, which is why ADR-0056 §12a(c) exempts it from the §7 laundering rule for now. Folding it in is what lifts that exemption — and it moves existing goldens, so it is a deliberate corpus event.
- vendor ergonomics re-delivered as cookbook recipes (#389)

ADR-0056 §4's argument for doing it at all: two naming axes is a defect that compounds with every new format, and pre-1.0 is the only moment it is cheap.
