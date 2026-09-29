---
type: issue
state: closed
created: 2026-09-08T09:21:12Z
updated: 2026-09-28T21:11:48Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/416
comments: 0
labels: none
assignees: none
milestone: 0.1.0-alpha.2
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:37.174Z
---

# [Issue 416]: [fix(ingest): streaming path silently drops [product.generation]/[product.producer] (ADR-0058 gap)](https://github.com/vig-os/tessera/issues/416)

## Problem

Surfaced by the #415 review (ADR-0058 landing). The **batch** ingest path routes a spec's
`[product.generation]` / `[product.producer]` through `apply_spec_metadata(m, meta, parents, p)`
(`tessera-ingest/src/engine.rs:667`), but the **streaming** path
(`stream_to_listmode_product_2p_to_file`, ~`engine.rs:499`) only receives `inherited` / `inherited_study`
/ `p.metadata` — it never plumbs `p.generation` / `p.producer`, and `tessera-io::WriteSession` has no
`with_generation` / `with_producer`.

## Failure scenario

An operator specs a recipe on a large listmode product:

```toml
[product.generation.config]
energy_window_keV = "425-650"
```

If the streaming path is selected (large product, or `--streaming`), the seal **succeeds with no
generation recorded**. Because `requires_generation` defaults to `false`, validation does not catch it.
Silent provenance loss on exactly the large-acquisition products most likely to use streaming.

## Fix

- Add `with_generation(Generation)` / `with_producer(Producer)` to `tessera-io::WriteSession` (mirror
  `ProductBuilder`).
- Plumb `p.generation` / `p.producer` into `stream_to_listmode_product_2p_to_file` and stamp them on
  the streamed seal.
- Extend the through-line test (`engine.rs:~1130` currently exercises generation/producer only on the
  batch raw path) to cover the streaming path — assert the sealed manifest carries the spec's
  generation/producer.

## Not a back-compat hazard

Pure additive plumbing; no corpus regen expected for existing fixtures (they don't stream with a
generation spec). Follow-up to #409 / ADR-0058.

Refs: #409
