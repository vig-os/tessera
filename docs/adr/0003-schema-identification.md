# ADR-0003 — Schema identification (register D3): string product names, open-world

Status: **Accepted** (2026-06-26, as-built) · Register: D3 · Supersedes the original proposal
(per-schema monotonic numeric ids + `<plugin>:<id>` namespacing + reserved ranges).

## Context
D3 asked how product schemas are identified. The original proposal was a numeric id allocator with
reserved ranges. The implemented `SchemaRegistry` (P1, #200) took a simpler, more interoperable route.

## Decision
1. **Products are identified by a stable string name** — `recon`, `listmode`, `sinogram`, `spectrum`,
   `roi`, `transform`, `calibration`, `sim`, `device_data`. The name is the manifest `product` field
   and the registry key. No numeric ids, no reserved ranges.
2. **Open-world.** `SchemaRegistry::validate` accepts unknown products (returns `Ok`) — a custom or
   downstream domain can ship its own product types without registering centrally or forking the
   engine. A product that *claims* a built-in name MUST satisfy that schema.
3. **Namespacing is by string convention** when needed (e.g. `vendor:product`), not a numeric
   plugin-range scheme. Field identity within a schema is the fd5 stable-string `FieldSpec.id`
   (rename-safe), not an ordinal.

## Why
- String names are self-describing in the manifest (a reader sees `"product":"recon"`, not `"7"`),
  which serves FAIR Interoperability far better than an opaque numeric registry.
- Open-world matches the engine's "schema-driven, domain-agnostic" principle (schemas are embedded
  data, not engine code) — new domains must not require a central allocator.
- A numeric-id allocator adds coordination cost (who owns the registry? reserved ranges?) with no
  benefit at the format layer; content-addressed `id` already provides global uniqueness.

## Consequences
- No global id authority to maintain; collisions are avoided by string convention + the open-world
  rule (unknown ⇒ permissive).
- Built-in schemas evolve additively (new optional fields/blocks bump the schema `version` string;
  field `id`s never change) without renumbering.
- If a future need for compact numeric tags arises (e.g. a binary index), it can be layered as a
  derived lookup without changing the canonical string identity.
