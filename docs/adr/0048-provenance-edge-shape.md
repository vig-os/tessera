# ADR-0048 — Provenance edge shape: merkle-rooted single reference, per-file digests in `aux/` (not a path list in the seal)

**Status:** Accepted (2026-07-02, resolves #267). Builds on **ADR-0040** (PHI hygiene, `--source-label`),
**ADR-0042** (self-contained `aux/` members, outside the seal), **ADR-0034/S15** (writer-determinism),
and the FAIR-export half of #267 already shipped (#275: opaque `urn:tessera:source:<merkle>` entities).

## Context — what "provenance-as-list" was supposed to fix, and the trap in the obvious fix

A multi-file provenance edge (a DICOM series' `ingested_from`) records its sources as a **comma-joined
string** in `Source.reference`. That is a string hack (proper structure would be a list), and #267 asked
to make it a **structured list** so `ls` / `inspect` / `export` carry per-file detail.

The export half shipped in #275: FAIR records emit one opaque `urn:tessera:source:<merkle>` entity per
edge with the merkle `content_hash` as `sha256` — **no path, no PHI, valid JSON-LD.** This ADR settles
the remaining "manifest data-model half."

**The obvious data-model change is actively harmful.** Turning `reference` into
`Vec<SourceFile { reference, digest, size }>` where `reference` is each source **path** would:

1. **Regress PHI.** An 890-slice DICOM series would put **890 patient-bearing absolute paths back into
   the sealed manifest** — re-opening exactly the leak `--source-label` (ADR-0040) was built to close.
   The whole point of the source-label seam is that PHI-bearing paths never reach the seal; a per-path
   list in the seal walks straight back through it.
2. **Force a corpus regen for a schema break.** Changing `Source.reference`'s *type* changes the
   manifest JSON of every product with sources → every `manifest_hash` moves → conformance-corpus +
   writer-determinism goldens regenerate. That is the highest-risk class of change, spent on a field
   whose PHI-safe content is a **single hash**, not a list of paths.

So the naive list is the wrong shape: it is *more* PHI-leaky and *more* seal-destabilising than the
status quo, to carry information (which exact files) that is either already redacted on purpose
(`--source-label`) or already committed to cryptographically (the merkle root).

## Decision

### §1 — The sealed edge stays a single merkle-rooted reference

A `Source` remains `{ role, reference, content_hash }`:

- `reference` is **one** value — a clean `--source-label` (recommended for multi-file / clinical
  sources) or, absent a label, the single path / URN. It is **not** a list and **not** comma-joined
  structure the reader must parse. (The legacy comma-joined form still parses; producers are steered to
  `--source-label` for the multi-file case, and `export` already treats `reference` as opaque.)
- `content_hash` is the **blake3 merkle root over the per-file digests** (`source_digest`, already
  shipped) — so the edge cryptographically commits to *the exact set of source bytes* regardless of how
  many files, without naming them in the seal.

This keeps the seal small, PHI-controllable, and determinism-stable: **no schema break, no corpus
regen.** "Which files, structurally" is answered outside the seal (§2), not by bloating the edge.

### §2 — Per-file structure lives in a PHI-safe `aux/` digest manifest (when a consumer needs it)

When a caller wants per-file provenance (an audit that must enumerate the N slices), it rides as an
**additive `aux/` member** (ADR-0042), *outside* the seal:

```
aux/sources/<role>.json   e.g. aux/sources/ingested_from.json
{
  "role": "ingested_from",
  "merkle_root": "blake3:…",          // == the sealed edge's content_hash (reconciles)
  "count": 890,
  "files": [
    { "digest": "blake3:…", "size": 524288 },   // NO path — digest + size only
    …
  ]
}
```

Properties, each load-bearing:

- **PHI-safe by construction.** It carries **digests + sizes, never paths / filenames.** A blake3 digest
  is not PHI; the patient-bearing path never appears. (A producer that *wants* to record clean per-file
  labels may add a `label` field — but the default, and the only thing the ingest engine emits, is
  digest+size.)
- **Determinism-safe.** `aux/` is outside `content_hash` / `manifest_hash` by construction (ADR-0042,
  proven byte-identical on add/remove) → **the conformance corpus is untouched**; the seal never sees it.
- **Reconciles with the seal.** `merkle_root` in the aux manifest equals the sealed edge's
  `content_hash` — a verifier folds the `files[].digest` list and checks it matches, so the (unsealed)
  aux manifest is still *authenticated* transitively by the sealed root it must reproduce.
- **Ordered + deterministic.** `files` is in the same order the merkle root folds (source order), so the
  aux manifest is itself byte-stable across re-ingest.
- **Optional + detachable.** Ship lean (edge only) or fat (with the aux manifest); a reader uses it if
  present, and `tessera tree` / `aux_names` already list it (#268), `read_aux` already reads it.

### §3 — Non-goals

- **No per-path list in the seal** (§Context — PHI + determinism). The sealed edge is a single
  reference + merkle root, period.
- **No new sealed schema field** → no corpus regen.
- **Not a replacement for `--source-label`** — the label is still how PHI-bearing references are kept
  out of the seal; the aux manifest is the *structured, PHI-free* companion for the multi-file case.

## Consequences & follow-up

- **The user-visible #267 problem is already solved** (#275 export: opaque URNs, no path/PHI leak, valid
  JSON-LD). This ADR closes the *data-model* question: the answer is "don't put the list in the seal."
- **The `aux/sources/<role>.json` emission is a clean, low-risk follow-up** — `source_digest` already
  computes the per-file digests it would list (it currently keeps only the root); exposing them as a
  `Vec<(digest, size)>` and stamping the aux member after seal (the same seam as `aux/provenance.json`,
  ADR-0042) is additive, corpus-neutral, and needs no seal change. Filed as the #267 implementation
  step; the format contract (this ADR) is what had to be decided first, because the *wrong* shape (a
  path list in the seal) is the one that would have been expensive to undo.
