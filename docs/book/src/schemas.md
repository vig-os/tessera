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

## The column contract: `tables` and `arrays`

`fields` and `blocks` say which metadata a product carries and how many blocks play each role. A schema can
also say what is **inside** a block of a role (ADR-0065 §8). No built-in product declares one yet; the
draft consumer schemas of #801 (`tessera/crates/tessera-core/tests/fixtures/801-consumer-schemas/`) do.

An abridged, illustrative example:

```json
"tables": {
  "pethits": {
    "columns": [
      { "name": "event_id", "dtype": "u8", "description": "Geant4 event id", "unit": "1" },
      { "name": "n_compton", "dtype": "u2", "description": "Compton count", "unit": "1", "optional": true },
      { "name": "edep_keV", "dtype": "f4", "description": "Deposited energy", "unit": "keV", "nullable": true }
    ],
    "sequence_key": ["event_id"],
    "keys": [["event_id"]],
    "references": [
      { "columns": ["event_id"], "to": { "scope": "block", "block": "events", "columns": ["event_id"] } }
    ]
  }
},
"arrays": {
  "lyso_map": { "dtype": "float32", "rank": 2, "axes": ["y", "x"], "unit": "1" }
}
```

| key | meaning |
| --- | --- |
| `tables.<role>.columns` | the columns in storage order; each is a block column (`name`, `dtype`, `nullable`, `unit`, `scale`, `description`, …) |
| `optional` (on a column) | a block may leave the column out altogether; per-row "not recorded" is `nullable` instead |
| `sequence_key` | the ordered key columns the rows are sorted by |
| `order` | absent: rows are in `sequence_key` order; `"producer"`: the producer's own order, with an empty `sequence_key` |
| `keys` | lists of column names, each unique per row |
| `references` | `columns` pointing at `to { scope, block, columns }`; `scope` is `"block"` or `"source(<role>)"`, anything else fails to parse; optional `note` |
| `arrays.<role>` | `dtype` (array vocabulary, e.g. `int16`), `rank`, `axes` (empty = the producer's choice), `unit` |

For now the contract is parsed, carried and sealed with the schema, and no block is checked against it.
Checking a block's columns against its role is the rest of #809; the row checks are #833, `keys` and
`references` are #780, and the contract's own consistency (a column listed twice, a key naming an undeclared
column) is #844.

Both maps are left out of the JSON when empty, so a schema without a contract serialises exactly as before
and every sealed file keeps its bytes. **Binaries up to 0.1.0-alpha.5 don't know these keys**: they parse
such a schema, drop the contract without a word, and validate only its `fields` and `blocks`. Their data
reads are unaffected, since each block's own spec carries its columns. A reader that refuses what it can't
honour is #830.

## Open-world products

A product name the registry does not know is **allowed** — the field stays absent from the seal and
nothing validates it. That is deliberate (ADR-0029's permissive escape hatch): a domain Tessera has never
heard of should not have to patch the registry to store its data. You lose the schema guarantees, not the
ability to seal.
