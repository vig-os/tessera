# ADR-0042 — Self-contained `.tsra` via non-sealed `aux/` members

Status: **Proposed** (2026-07-01). Refines ADR-0022 §6 (container layout), completes ADR-0037 §0
bug #2 (signature carriage), and lands the "one file to share" ask from #259.

## Context — "one file to share"

A `.tsra` was **already one self-contained file** for the *data*: a STORED zip64 archive whose
`manifest.json` lists every block by name + digest, with embedded schema (#248) and the full
`extra/` namespace (#255) both sealed inside. All of it travels in one file.

But two load-bearing pieces still rode *outside* the file as detached sidecars:
- The **signature** (`<file>.tsra.sig.json`, ADR-0037) — ed25519 over `manifest_hash`, kept adjacent
  because a signature can't sit *inside* the bytes it signs (that would be circular: adding it
  would change `manifest_hash`).
- The **ingest wall-clock** (`ingested_at`, chosen in the ADR-0025 / #259 discussion) — a real "when
  did this file get made" stamp that we deliberately kept OUT of the sealed manifest so re-ingest
  stays byte-identical (writer-determinism is a release gate: FEATURE-MATRIX §C).

Share just the `.tsra` and you **lose the signature**. That is the "losing stuff" problem the ask
called out. The wall-clock stamp is worse: today it has nowhere to live at all.

## Decision — `.gitignore` for the seal

A `.tsra` is a **zip**, which can carry **extra members beyond the sealed `manifest.json` + blocks**.
So the rule is exactly the one line the #259 design comment names:

> **Any zip member the manifest does *not* reference is `aux`: carried inside the one file, but
> *ignored by the seal* — excluded from `content_hash` / `manifest_hash` by construction.**

This is the `.gitignore` for the seal. The **manifest = tracked** (its `blocks` list defines what's
sealed); everything else in the container = **present-but-not-hashed**.

To make aux members findable + toolable, reserve one explicit prefix:

```
study.tsra  (one zip)
├── mimetype               ─┐
├── manifest.json          │ SEALED (the "index": manifest.blocks + embedded schema + extra
├── blocks/volume/…        │         all enter content_hash + manifest_hash)
├── blocks/events_0000/…  ─┘
└── aux/                   ─┐ SEAL-IGNORED (carried, not hashed)
    ├── signatures/<key_id>.sig.json   │  ← embedded ADR-0037 signature envelope
    └── provenance.json                │  ← ingest wall-clock + producer + host
                            ─┘
```

## The load-bearing invariant

**Adding or removing any `aux/` member MUST leave `content_hash` AND `manifest_hash`
byte-identical.** That is not a nice-to-have — it is what makes the whole approach safe:

- The **conformance corpus** (`tessera/corpus/corpus.json` + `corpus/files/*.tsra`) stays untouched.
  Every existing fixture reproduces its recorded triple, so a second implementation (P8 pure-Python
  reader) verifies unchanged.
- The **signature** can now live *inside* the file it signs — because adding it changes only the
  aux subtree, never the sealed region the signature covers. Non-circular by construction.
- The **ingest wall-clock** rides as `aux/provenance.json` with a fresh `ingested_at` every ingest,
  yet the sealed region (id / content_hash / manifest_hash) stays byte-identical across re-ingest.
  Writer-determinism on the seal-covered fields is preserved.

`tessera_io::container::add_aux_members` / `write_aux_members` enforce the invariant on the write
side (rewrite → atomic-rename, then reopen + assert the pre/post hashes AND the on-disk
`manifest.json` bytes match). Unit tests (`aux_members_are_carried_but_ignored_by_the_seal` +
`unpack_pack_round_trips_the_aux_subtree`) lock it in.

## Consequences

### Container (ADR-0022 §6, extended)

Zip entries, in order:
1. **`mimetype`** — STORED, first entry, magic (unchanged).
2. **`manifest.json`** — the sealed manifest (unchanged).
3. **`blocks/<name>`** — one entry per block payload, digest-verified (unchanged).
4. **`aux/<name>`** — zero or more non-sealed aux members. Reserved sub-namespaces:
   - `aux/signatures/<key_id>.sig.json` — embedded ADR-0037 envelope.
   - `aux/provenance.json` — ingest-time wall-clock / producer / host record.

All entries remain STORED with the pinned 1980-01-01 mtime. Aux members never trigger a
`manifest.blocks` entry; they are found by prefix (`AUX_PREFIX = "aux/"`).

### Signing (ADR-0037, completed)

- `tessera sign` **embeds** the signature by default (`aux/signatures/<key_id>.sig.json`); the
  legacy detached form is opt-in via `--sidecar`.
- `tessera verify-sig` reads the embedded form first, falls back to the detached sidecar. The
  ed25519 envelope + `verify_manifest` semantics are unchanged — only the location moved.
- **Single-signer semantics** on `sign`: each call clears any prior `aux/signatures/*` members
  before writing the new one, so a re-sign never leaves stale signatures. Co-signed products
  (multiple key_ids in the same file) are a follow-up verb (`sign --add`), not a byproduct.
- The **detached form remains** for the OCI double-layer distribution path (ADR-0037 §0 bug #2:
  `tessera push` still carries `<file>.tsra.sig.json` as a second layer over the wire for
  registries that surface layers to their UI) and for legacy operators.

### Ingest provenance (new)

`tessera_io::provenance::stamp_ingest_provenance` writes `aux/provenance.json` at the tail of every
ingest path in `tessera_ingest::engine` — the batch `seal_to_tsra`, the blob streaming
`seal_streaming_to_tsra`, and the HDF-compound streaming path (after the atomic rename to the
id-named final path). Fields:

```json
{
  "ingested_at": "2026-06-25T12:34:56Z",  // RFC-3339 UTC wall-clock at seal
  "producer":    "tessera/<TESSERA_VERSION>",
  "host":        "<HOSTNAME or HOST env, else omitted>"
}
```

Hostname is `std::env::var("HOSTNAME").or_else(|| std::env::var("HOST"))` — no external `hostname`
crate is pulled into the workspace. All fields are optional at read time (old aux stamps parse).

**Test seam**: `TESSERA_SKIP_PROVENANCE=1` turns the stamp into a no-op — for the byte-identical
whole-archive tests and for deterministic docs-as-tests fixtures that would otherwise embed a live
wall-clock. Callers can also pin any field via `ProvenanceOptions`.

### `unpack` / `pack` round-trip

`unpack` now explodes any aux subtree under `outdir/aux/<name>`; `pack_dir` re-embeds it if
present. So a `.tsra` that carried an embedded signature + provenance survives unpack → pack with
byte-identical seal AND the aux payload intact. Test:
`unpack_pack_round_trips_the_aux_subtree`.

### `tree` / `ls` (ADR-0028 unified hierarchy)

- `tree` renders an `aux` node listing every embedded aux member (`aux/signatures/<key_id>.sig.json`,
  `aux/provenance.json`, …) distinct from the `sidecars` node (which still enumerates the
  adjacent-on-disk `<file>.tsra.sig.json` / `<file>.tsra.fcrypt.json`).
- `tsra ls FILE` lists `aux/  (N members)` at the top level.
- `tsra ls FILE aux` enumerates the members.
- `tsra ls FILE aux/<name>` reads one member (pretty-printed if JSON).

`status_line`'s signed-badge accepts either an embedded signature OR a detached sidecar.

## Relation to other ADRs

- **ADR-0022 (container)** — extends §6's entry layout with the `aux/` prefix; sealed layout
  unchanged.
- **ADR-0037 (signing trust)** — completes §0 bug #2: with the embedded default, the signature
  can never be lost by sharing just the `.tsra`. The archival principle "verify offline forever"
  strengthens: the verifier now needs only one file, not two.
- **ADR-0025 (ingest model)** — adds the wall-clock `ingested_at` recording without disturbing the
  "normalise at the door" seal (the sealed timestamp is still the acquisition instant from the
  input, not the wall-clock at ingest).
- **ADR-0028 (unified hierarchy)** — aux members are the mechanism the "derived sidecars" §
  wants for #215; this ADR lands the container-level plumbing.
- **ADR-0041 (field decryption)** — a future rung-2 `aux/fcrypt.json` slot is now available inside
  the file, symmetric with today's detached `<file>.tsra.fcrypt.json`. Not implemented here.

## Non-goals

- **Not putting wall-clock or the signature INSIDE the seal.** That would break writer-determinism
  (S15) or become circular (signing).
- **Not stacking multiple signatures on `sign`.** Single-signer semantics on the verb; co-signing
  is a follow-up (`sign --add`).
- **Not changing the OCI double-layer distribution.** `push` / `pull` still carry the detached
  sidecar as a second registry layer when it's present — the two channels coexist.
