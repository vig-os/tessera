# Product-schema reference

Every `.tsra` declares a **product** — `recon`, `listmode`, `blob`, … — and that product's schema is its
contract: which metadata fields it must carry, which it should, and which blocks play which role. The
schema is **embedded in the sealed file** (ADR-0029), so a product carries its own contract and a reader
never has to guess.

Three tiers, and the difference matters when you are writing your own data:

| tier | what happens if it is absent |
| --- | --- |
| **required** | a hard **block** — `tessera schema <file>` fails and the product is invalid |
| recommended | a non-fatal **warn** on stderr at ingest — the FAIR-completeness nudge |
| optional | nothing |

To check a file you already have, against its own embedded contract:

```console
tessera schema study.tsra          # validate: required fields + block roles
tessera schema study.tsra --json   # dump the file's embedded schema
```

The tables below are the **built-in registry** — what `tessera` knows without being told. They are
generated from `SchemaRegistry::builtin()` rather than written beside it, and
`schema_reference_matches_the_committed_copy` fails the build if the two drift. A schema cannot gain a
required field without this page gaining it in the same commit. Regenerate with:

```console
cd tessera && cargo run -q -p tessera-core --example schema_reference \
  > tests/derived-docs/schema-reference.md
```

## The built-in products

{{#include ../../../tessera/tests/derived-docs/schema-reference.md}}

## Open-world products

A product name the registry does not know is **allowed** — the field stays absent from the seal and
nothing validates it. That is deliberate (ADR-0029's permissive escape hatch): a domain Tessera has never
heard of should not have to patch the registry to store its data. You lose the schema guarantees, not the
ability to seal.
