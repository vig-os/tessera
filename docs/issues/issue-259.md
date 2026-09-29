---
type: issue
state: open
created: 2026-07-01T12:34:33Z
updated: 2026-09-28T17:59:09Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/259
comments: 1
labels: none
assignees: none
milestone: 0.1.0-beta
projects: none
parent: none
children: none
synced: 2026-09-29T07:53:00.736Z
---

# [Issue 259]: [feat(format): self-contained .tsra — embed signature (+ ingest-time, field-encryption) as non-sealed zip members (one shareable file)](https://github.com/vig-os/tessera/issues/259)

## The ask (from real use)
> "it should be one file to share, containing everything — never losing stuff easily."

Today a `.tsra` is **already one self-contained file** for the *data*: the STORED zip holds the manifest + every block + the embedded **schema** (#248, sealed) + the **extra/** namespace incl. the full DICOM header (#255, sealed). All of that travels inside the one file.

**But the signature is a *detached sidecar*** (`<file>.tsra.sig.json`, ADR-0037), and field-encryption (ADR-0041) would be another (`.tsra.fcrypt.json`). Share just the `.tsra` and you **lose the signature**. That's the "losing stuff" problem.

## Why they're external today (and why it's fixable)
A signature can't live *inside the bytes it signs* (circular: adding it changes `manifest_hash`). BUT a `.tsra` is a **zip**, which can carry **extra members beyond the sealed manifest+blocks**. So embed the signature as a **non-sealed zip member** (e.g. `signatures/<key_id>.sig.json`): inside the one shareable file, still outside the cryptographic seal (correct), `verify-sig` reads it from within, `pack`/`unpack` round-trip it. This is the "signatures-as-hashed-block" fork ADR-0037 deferred — now requested. Ties into #215 ("derived sidecars" in the unified hierarchy).

## Scope
- Container: allow **non-sealed auxiliary members** in the `.tsra` zip (a reserved prefix, e.g. `aux/`), excluded from `content_hash`/`manifest_hash` by construction; `verify` ignores them for the seal but can list them.
- `sign` writes the signature **into** the `.tsra` (default) with `--sidecar` to keep the detached form; `verify-sig` prefers the embedded one, falls back to the sidecar.
- **Ingest wall-clock** (the determinism-safe `ingested_at` + tool + host — chosen in discussion) rides the same way: a non-sealed `aux/provenance.json` member, so re-ingest stays byte-identical in the *sealed* region while the one file still records when/where/what.
- `push`/`pull`: a fully self-contained `.tsra` needs only one OCI layer (aux members already inside), though the detached-layer form stays supported.
- `tree` already lists a `sidecars` node; extend it to show embedded aux members too.

## Non-goals
- Not putting wall-clock/signature *in the seal* (breaks writer-determinism, S15).
---

# [Comment #1]() by [gerchowl]()

_Posted on July 1, 2026 at 12:38 PM_

### Design framing: aux members are "`.gitignore`d from the seal"

The `.tsra` seal already works like git's **index**: the **manifest lists exactly what's sealed** (the blocks). So the rule is simply —

> **Any zip member the manifest does *not* reference is `aux`: carried inside the one file, but *ignored by the seal* (excluded from `content_hash`/`manifest_hash` by construction).**

That's a `.gitignore`-for-the-seal: the manifest = tracked/hashed; everything else in the container = present-but-not-hashed. Reserve an explicit prefix so tools find them:

```
study.tsra  (one zip)
├── manifest.json            ─┐
├── blocks/volume/…           │ SEALED (the "index" — manifest lists these,
├── blocks/events_0000/…      │         content_hash + manifest_hash cover them)
│  (schema + extra/ ride in manifest.json, also sealed) ─┘
└── aux/                      ─┐ SEAL-IGNORED (carried, not hashed)
    ├── signatures/<key_id>.json   │  ← the detached signature, now inside the file
    └── provenance.json            │  ← ingested_at wall-clock + tool + host
                               ─┘
```

**Integrity caveat (matches git semantics):** aux members are *carriage, not integrity-guaranteed*. Adding/removing an `aux/` member never changes the seal — which is exactly why the signature (which must sit *outside* the thing it signs) can live there. Anything that needs tamper-evidence goes in the manifest/seal (schema, extra/, sources, producer already do). So:

- **one file** to share (never lose the signature),
- **determinism preserved** (aux is outside the seal → re-ingest byte-identical in the sealed region),
- **verify** still checks only the sealed region; `verify-sig` reads `aux/signatures/…` from within.

`unpack`/`pack` round-trip `aux/` verbatim; `push`/`pull` need only the single artifact.

