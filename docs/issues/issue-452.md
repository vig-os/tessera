---
type: issue
state: closed
created: 2026-09-28T19:03:44Z
updated: 2026-09-29T05:57:39Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/452
comments: 0
labels: none
assignees: none
milestone: 0.1.0-alpha.2
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:32.035Z
---

# [Issue 452]: [Resolve generation.config_ref payloads, and a `tessera provenance` verb that walks the derived_from DAG](https://github.com/vig-os/tessera/issues/452)

## Context

#417 gave ADR-0058's sealed recipe an operator surface: `tessera inspect` now prints the
structured `producer` (every build field, not just the folded `tool/version (commit)` one-liner)
and the `generation` record, and `inspect --json` dumps the whole sealed manifest so provenance is
scriptable without reaching into the container format.

Two items from that issue's **P2** are deliberately still open, because each is a materially bigger
job than printing a digest. Splitting them out so #417 can close on what actually shipped.

## 1. Resolve a `config_ref` to its carried block

`generation.config_ref` is a `blake3:` digest pointing at a config / `.ini` / `.cfg` **Blob block
carried in the same `.tsra`** (ADR-0058 §2) — the bit-faithful route for large vendor config, e.g.
the DUPLET DAQ `acq.cfg.LYSO4x9_6_SIPMGEN1_230223`. `inspect` prints the digest; it does not
dereference it.

Wanted: resolve the digest to the block and show (or extract) the config the product was actually
generated with, so the recipe is readable end to end rather than a hash the operator must chase by
hand. Open questions worth settling first:

- Is this a flag on `inspect` (`--resolve-config`), a mode of the existing `extract`, or its own verb?
- What happens when the digest does not resolve — a carried block that was dropped, or a `config_ref`
  pointing at a block in a *different* `.tsra`? Silent omission is wrong; this should be a loud,
  typed error, but it must not make an otherwise-valid product fail to `inspect`.
- Large vendor config should stream, not be buffered to render.

## 2. `tessera provenance <file>` — the DAG view

Today `inspect` shows one product's own producer + generation + its `sources[]` edges. ADR-0058 §5's
actual operator question is a chain question: *what identity did this inherit, from where, and what
recipe made each hop?*

Wanted: a verb that walks `derived_from` and prints producer + generation + inherited identity per
hop. This needs parent resolution (the parents are separate `.tsra` files, possibly in a collection,
possibly remote), which is why it is not a rendering change — it is the operator view of the
provenance DAG, and it should reuse `provenance::verify_chain`'s walk rather than grow a second one.

## Not in scope

Cross-product generation-graph queries (\"which config produced this cohort?\") stay deferred to the
downstream index concern (#297/#299) — ADR-0058 already records that as a non-decision.

## References

- ADR-0058 §2 / §5 · #417 · #409 · #390 (the operator-surface gap this family keeps hitting)
- `tessera/crates/tessera-cli/src/nav.rs` (`producer_lines` / `generation_lines`)
- `tessera/crates/tessera-cli/tests/cmd/provenance.trycmd` (the walkthrough to extend)

Refs: #417
