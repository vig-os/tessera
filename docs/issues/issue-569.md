---
type: issue
state: open
created: 2026-10-16T20:45:24Z
updated: 2026-10-16T20:45:24Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/569
comments: 0
labels: docs, python, area:bindings
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-17T06:32:47.256Z
---

# [Issue 569]: [docs(py): Builder timestamp is identity (part of id) — say so, or Python producers break writer determinism](https://github.com/vig-os/tessera/issues/569)

## Context

Dogfooding from MorePET/anatomist#86 at tessera `dev` `fa581f5`, synthetic data only.

## Friction

`tessera.Builder(product, name, description, timestamp)` documents `timestamp` only as "ISO-8601 `timestamp`" (`tessera-py/src/lib.rs`, `Builder.__new__`). But it is one of the default `id_inputs` (ADR-0020: `id = blake3(JCS({product, name, timestamp}))`), and ADR-0046 defines it as the *acquisition or logical time of what the product represents*. A Python producer that does the obvious thing, `datetime.now(timezone.utc).isoformat()`:

- gets a new `id` and `manifest_hash` on every re-run of identical inputs, which breaks writer determinism (the ADR-0042 release gate) from the caller's side;
- seals a wall-clock value, which ADR-0042 says belongs in `aux/`.

The opposite mistake is just as easy: two products with the same `product` and `name` and a constant timestamp share one `id` (for example one derived step run on two subjects). Only `manifest_hash` tells them apart.

Repro (synthetic): pack the same content twice with `timestamp=datetime.now(...)` and compare `id`; it differs. Pack two different contents with the same `(product, name, timestamp)`; the `id` is equal.

## Ask (docs, plus an optional guard)

- The `Builder` docstring and `docs/book/src/bindings.md` should say that `timestamp` is identity (part of `id`), means the logical or acquisition time (ADR-0046), must be deterministic for a given product, and that `name` has to tell apart products of the same kind and time.
- Optional: warn when `timestamp` is within a few seconds of the wall clock at `pack()`.

## Linked

ADR-0020, ADR-0042, ADR-0046, MorePET/anatomist#86.

