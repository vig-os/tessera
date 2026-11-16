# Tessera reference reader (independent, pure-Python) — the v1.0 conformance gate

This is a **second, independent** implementation of the Tessera read path, written in pure Python
**from `tessera/docs/SPEC.md` alone** — no access to the Rust engine. It exists to prove the spec is
sufficient: a fresh implementer, given only the spec, must be able to open every `.tsra` test vector
and reproduce its golden `id` / `content_hash` / `manifest_hash` (and verify each block's digest).

This is the **v1.0 release gate** (ROADMAP P8 / #211). It was produced by a spec-only agent and
reproduced all 6 corpus goldens on the first attempt; the clarifications it surfaced are folded back
into `SPEC.md`.

## What it depends on
Only the container (§6) + canonical hashing (§2–§4): Python stdlib `zipfile` + `json`, plus `blake3`
and an RFC 8785 `jcs` encoder. It does **not** decode array/table payloads — reproducing the identity
triple needs only the stored bytes, not the pcodec/Vortex codecs.

## Run it

```sh
python -m venv .venv && .venv/bin/pip install -r requirements.txt
.venv/bin/python run.py     # prints a PASS/FAIL table over corpus/files/*.tsra
```

Exit code 0 ⇔ all fixtures reproduce their goldens, every manifest in `corpus/seal-cases.json` reaches its
`verdict`, and the reader refuses each crafted refusal archive for its stated reason. The Rust reader checks the
same seal-case file (`tessera-core/tests/seal_cases.rs`), so the two readers are compared case by case. The refusal
archives today list a block name twice (SPEC §5, #734), with a foreign digest and with the block's own, built from a
corpus fixture and re-sealed; the Rust reader refuses the same two (`tessera-io/tests/unique_block_names.rs`).

## What it checks

`manifest_verdict` (and `verify_tsra`, which adds the container and block payloads) returns a verdict for every
input; neither raises (`verify_tsra` reports an archive `zipfile` can't read, such as an encrypted member, as
not ok). In SPEC §7's order, so a manifest with two faults gets the verdict of the first:

1. **Refused, not I-JSON (SPEC §4):** text that is not JSON or not UTF-8 (UTF-16, a byte-order mark), a key repeated
   in any object, `NaN` / `Infinity`, a number outside a double's range, a lone surrogate, nesting deeper than 127
   objects/arrays.
2. **Refused, outside the declared shape (SPEC §5):** `reader.MANIFEST` is one declarative table, one row per field
   of the Rust `Manifest`, `BlockRef`, `Source`, `Producer`, `Generation` and `IngestTransform`, checked at every
   depth: each record is an object (never the array form), each declared field has its JSON type (string, boolean,
   an object for a map, an array for a list), a block `kind` is one of `array`/`table`/`chunk_index`/`blob`, a block
   `name` follows the ADR-0063 grammar, every `id_inputs` value is a string, `producer` is a Producer object or a
   legacy string, and a required field is present. `null` stands for an absent value only where the Rust
   field is optional (`producer`, `generation`, `schema`, `study`, the hashes, a block's `digest`, a source's
   `content_hash`, Producer's `git_*`/`dirty`, `generation.config_ref`); the maps and lists refuse it. Opaque
   values hold any JSON: `schema`, `blocks[].spec`, and the values of `metadata`, `extra`, `generation.config` and
   `ingest_transform[].params`. A key the table doesn't declare is a newer field: it is sealed, never type-checked.
   The check walks each record's **declared** keys only, so a newer key is never visited and nothing is exempted
   by a path string: a top-level key literally named `producer.agents` can't stand in for the nested field. This
   reader doesn't report newer-field paths (the `newer_fields` of `seal-cases.json` are the Rust view's, rendered
   as SPEC §4 defines), and the case `recon_int16/key-spelling-a-nested-path-resealed` holds both readers to it.
3. **Refused:** a `tessera_version` whose major (before the first `.`) is not an optional `+` and ASCII digits, or
   is above the supported major.
4. **Refused:** a block name listed twice (SPEC §5).
5. **Tampered:** `id` ≠ `digest(JCS(id_inputs))`.
6. **Refused:** a block without a `digest` (SPEC §5).
7. **Tampered:** `content_hash` ≠ the MMR root of the block digests, or `manifest_hash` ≠ the seal over the parsed
   object without `manifest_hash` (a missing hash is tampered).

Deliberate differences from the Rust reader, each outside what these verdicts cover:

- It reads **manifests only**. A collection (`collection.json`) and the embedded product schema are checked by the
  Rust reader alone: the seal-case corpus has no collection or schema cases yet, and SPEC doesn't define a
  collection's `content_hash` leaf, so a spec-only reader can't verify one.
- It **doesn't validate the embedded schema** (`schema` is opaque here, as it is to the Rust manifest verify).
- Integers beyond 2^53 (in opaque values) are hashed as each JCS encoder renders them; whether the two readers
  agree there is open in #886.

## Why it matters
Until a second codebase reads Tessera correctly from the spec alone, "the spec" is really "whatever
the Rust does." This reader closes that gap — and re-running it after any spec or corpus change keeps
the spec honest.
