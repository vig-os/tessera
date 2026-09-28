# Ingesting your own data

The previous chapter is about vendor files. This one is about **yours** — a Parquet table, an Arrow file,
a CSV, a NumPy array — and it is the shorter story, because you do not need a vendor decoder for data you
produced yourself.

## The ladder: preserve, or normalise

Tessera can take in foreign data two ways, and picking the wrong one is the most common early mistake.

| Verb | What you get | What you give up |
| --- | --- | --- |
| `ingest blob` | bit-faithful bytes · seal · provenance · signing · distribution | interoperability, query, ROI reads, per-field integrity |
| `ingest table` / `ingest array` | all of the above **plus** codecs, column projection, chunk-level integrity, cross-arch-deterministic bytes | your source format's quirks, dropped at the door by design |

It is a **ratchet, not a fork**: everything `blob` gives you, the normalising verbs also give you. `blob`
is the honest fallback for anything that cannot be carried into a primitive without lying about it — and
when that happens you are told so, with the reason and the command.

The only question at the door is whether your data's *logical values* fit Tessera's flat column set or its
dense N-D grid. Usually they do.

## Which verb?

The verb names the **primitive**, not your file format — because the primitive is what the product *is*:

- your data is **rows of typed fields** (measurements, events, a table) → `ingest table`
- your data is a **dense numeric grid** (an image, a volume, a field) → `ingest array`
- neither, or you are not ready to decide → `ingest blob`

That also means the verb is how you override a file that misrepresents its shape. A Parquet full of
flattened volumes is still volumes; a NumPy structured array is a table, not an array. Name the primitive
you actually have and Tessera will not argue with you — but it will never *guess* it either, because
nothing in the bytes distinguishes a 2-D image from an N-row × M-column table.

## Recipes

### A Parquet or Arrow table

Nothing to declare: these formats carry their own dtypes, so the whole invocation is the verb, the file
and the product's identity.

```text
tessera ingest table study.parquet study.tsra \
  --name study-01 --timestamp 2024-03-01T12:00:00Z --meta study=COHORT-A
```

`--from` is detected from the file's magic bytes (`PAR1` for Parquet, `ARROW1` for Arrow IPC / Feather).
The file **extension is never consulted** — a `.parquet` that is really Feather would otherwise become a
Tessera bug rather than your typo.

Your source's physical encoding does not survive, and that is the point: ingest is a *logical re-encode*
through Tessera's own deterministic codecs. A snappy Parquet and a zstd Parquet of the same logical table
produce the **same** `content_hash`. So does the same table written by pyarrow, by polars and by DuckDB —
there is a CI gate that checks exactly that.

### A CSV

CSV carries no schema, and Tessera will not invent one — an inferred schema depends on which rows got
sampled, so the same file could seal two different ways, and a seal has to be reproducible. Declare the
columns instead:

```text
tessera ingest table measurements.csv out.tsra \
  --name demo --timestamp 2024-03-01T12:00:00Z --from csv \
  --column run_id:u4 --column detector:str --column energy_kev:f8? --column coincident:b1
```

- dtypes: `i1 i2 i4 i8` · `u1 u2 u4 u8` · `f4 f8` · `b1` · `str`
- `?` allows NULLs (`energy_kev:f8?`) — an empty numeric field is not zero
- declarations are **positional**, and checked against the header row, so a wrong order is an error rather
  than every value landing one column over
- `--delimiter` for TSV; `--null-token NA` to add a NULL spelling beyond the empty field
- numbers are parsed by Rust's standard library, which is correctly rounded and locale-independent — a
  `de_DE` locale cannot turn `1.5` into something else

If you would rather not declare anything, convert first and let the converter's dtypes carry through:

```text
duckdb -c "COPY (SELECT * FROM 'input.csv') TO 'input.parquet' (FORMAT parquet)"
tessera ingest table input.parquet out.tsra --name demo --timestamp 2024-03-01T12:00:00Z
```

Both routes land in the same place. They produce the identical `content_hash`.

### Attaching meaning, so the table is worth keeping

A Parquet or CSV has column *names* and *dtypes* and nothing else — no descriptions, no units, no
vocabularies. A sealed table that left those empty would be a CSV with a seal on it, which is not the
point of the format. There are two doors, and you can use either:

**At ingest**, with `--column-meta cols.toml` — the annotations land *inside* the seal:

```toml
[energy_kev]
short_name  = "Photon energy"
description = "Calibrated single-photon energy at the detector face"
unit        = "keV"
scale       = 1.0          # physical value = raw × scale

[patient_ref]
description = "Site-issued pseudonym, never the raw MRN"
sensitivity = "identifying"
```

**After the fact**, with `tessera commit --set …` — content-addressed, one new object, lineage preserved.

An entry naming a column that does not exist is an error, not a no-op: the failure worth preventing is
believing you classified `patient_id` when you typed `patient_ids` and the artifact sealed it unclassified.

### Columns you did not classify are `unknown`, not `public`

Every column a generic ingest seals without a `--column-meta` tier is stamped **`unknown`**. This is
deliberate and it is not pedantry. The vendor paths each have a defence — GE listmode never sees PHI
(they are scan events), DICOM classifies at the door per PS3.15 — and generic ingest has neither. Stamping
`public`, which means *safe in clear*, over a column that might hold patient identifiers would be a false
claim sealed permanently. `unknown` says the true thing, and leaves something an operator can gate on
before sharing.

If a column *name* looks like a direct identifier (MRN, patient id, name, DOB, accession, UID) and you did
not classify it, ingest says so once on stderr and carries on. It never blocks.

### When a column cannot be carried

Some source types have no honest flat representation, and those are refused rather than mangled. Each
refusal names the column, the reason, and what to do:

| Source column | Why | What to do |
| --- | --- | --- |
| `binary`, `fixed_size_binary` | base64-in-text would lie about the type and defeat the codec | `--exclude <col>`, or `ingest blob` |
| `list`, `map`, `union` | a per-row variable count has no fixed column shape; exploding rows would destroy the row identity you join on | `--exclude <col>`, or `ingest blob` |
| `decimal128(p > 18)`, `decimal256` | no lossless integer carrier, and decimal→float is lossy for exactly the data that uses decimals | `--exclude <col>`, or `ingest blob` |
| `interval` | encodes calendar arithmetic (a month is not a fixed duration) | store the resolved duration instead |
| a nullable `str` / `b1` | the table backend's validity mask currently covers the fixed-width numeric dtypes only | `--exclude <col>`, or fill the nulls in the source |
| a very wide `fixed_size_list` | that is a flattened grid, not a tuple — its home is the **array** primitive | `ingest array` |

What *is* carried, quietly and losslessly, is a set of things that look like losses and are not: a
`struct` flattens to dotted columns (`pos.x`, `pos.y`), a narrow `fixed_size_list` expands to `xyz.0 …`, a
dictionary/categorical column becomes its **values** (the source's integer codes are dropped on purpose —
they encode a writer-specific ordering that would otherwise reach your `content_hash`), a `decimal`
becomes an integer plus a `scale`, and a timestamp becomes integer ticks plus a recorded epoch. Every one
of those transformations is written **inside the seal**, so the artifact carries its own record of what was
done to it rather than relying on a warning you saw once.

### Many files at once

The CLI verbs build a one-product ingest under the hood. When you have several products, or a derivation
graph between them, write that graph down instead — a declarative spec (ADR-0035) runs through the same
engine:

```toml
[collection]
name      = "COHORT-A"
timestamp = "2024-03-01T12:00:00Z"

[[product]]
name   = "measurements"
role   = "raw"
schema = "table"
format = "parquet"
input  = "data/measurements.parquet"

[product.column_meta.energy_kev]
unit = "keV"
```

```text
tessera ingest --spec cohort.toml --out cohort-out
```

A spec is an archival artifact in its own right: its hash is sealed into every product it produced, so
"how was this made" is answerable from the product alone.

## The walkthrough, executed

Everything below is a **test**. It runs on every build against the real binary, so it cannot drift from
what the tool actually does.

{{#include ../../../tessera/crates/tessera-cli/tests/cmd/ingest_table.trycmd}}

## What you can rely on

For any input and any released build of a given Tessera version, ingesting that input produces the same
`content_hash`. Which formats a build can *read* may vary with how it was compiled (`tessera info` tells
you what yours has); the **bytes** produced for a readable one may not. That is enforced, not asserted:
the ingest conformance corpus is regenerated under four feature configurations in CI and every one must
agree with the committed goldens, and the reader's batch size, the writer's compression, the row-group
size, the dictionary encoding and the worker count are each pinned as *not* reaching the hash.
