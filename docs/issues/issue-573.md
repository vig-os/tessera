---
type: issue
state: open
created: 2026-10-16T21:45:17Z
updated: 2026-10-17T08:08:34Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/573
comments: 1
labels: bug, area:core, area:integrity
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-18T06:56:51.974Z
---

# [Issue 573]: [bug(core): a reader drops unknown manifest fields, so a correctly sealed newer product fails verify as tampering](https://github.com/vig-os/tessera/issues/573)

## The defect

A Rust reader **silently drops** any manifest field it does not model, then recomputes `manifest_hash`
over its own re-serialisation. So a product sealed correctly by a newer writer that added a field fails
`verify` as **tampering**, while the same file with an *unauthenticated* field spliced in passes.

The independent reference reader (`tessera/corpus/reference_reader/reader.py:145-147`) does the
opposite, because it hashes the parsed JSON object as SPEC §4 defines it. The two conformant readers
disagree on the same bytes.

- `Manifest` has no `deny_unknown_fields` and no catch-all (`tessera-core/src/manifest.rs:26-94`).
- `Manifest::canonical_bytes` re-serialises the struct (`manifest.rs:146-150`), not the parsed object.
- SPEC §4 defines the seal over "the manifest object with exactly one key deleted ... no other
  normalization" (`tessera/docs/SPEC.md:46-48`).
- RFC §13 makes forward reading normative: "v(N−1) warns + reads-what-it-can on vN"
  (`docs/rfc-tessera.md`, §13 "Additive-only evolution + bidirectional read").
- `tessera inspect --help` promises "Fields are added compatibly (readers default them, which is how
  `producer`/`generation` arrived)" (`tessera-cli/src/main.rs:238-239`). That holds only in the
  backward direction (a new reader on an old file).

## Repro (probe, tessera-core at dev `fa581f5`)

The probe adds `"actors": [{...}]` to each of the 8 corpus manifests:

| case | Rust `Manifest::from_json_verified` | reference reader `verify_tsra` |
|---|---|---|
| A: unmodified corpus file | Ok (struct round-trip == golden, 8/8) | ok=True |
| B: `actors` added, resealed per SPEC §4 | **Err(integrity: manifest_hash mismatch)** 8/8 | **ok=True** |
| C: `actors` spliced in, old seal kept | **Ok** (field silently dropped) 8/8 | ok=False |
| D: unknown key nested in `producer`, resealed | Err(integrity) 8/8 | — |
| E: explicit `"actors": []`, resealed | Err(integrity) 8/8 | — |

Every pre-ADR-0058 build already reports a valid `generation`-bearing product as corrupt. The same
will happen to any future additive field (#564's `actors`, ADR-0059's stats), and the operator sees
`Integrity` ("tampering, corruption, or a producer bug"), which is the misdiagnosis ADR-0058's
amendment exists to prevent.

The same class affects the embedded `ProductSchema` (`schema.rs:322-326`, `from_value` drops
unknown rules, so an older reader validates a product against a rule it never evaluated) and
`Collection` manifests.

## Fix

Designed in ADR-0060 §2 (proposed with #564): keep unknown fields verbatim (hashed, so the seal verifies
per SPEC §4), surface them as a warning, refuse every **re-seal** operation on a manifest that carries
fields this build does not understand, refuse `validate` on an embedded schema rule it does not
understand, and pin the canonical form of empty collections. This is a prerequisite for #564 P1.

---

# [Comment #1]() by [gerchowl]()

_Posted on October 17, 2026 at 08:08 AM_

Two more cases from a fresh reviewer's probe on dev fa581f5 (PR #586 audit), likely the same root cause (Rust verifies by re-serialising its parsed struct, not the parsed JSON object):
- A manifest that **omits `sources`**, and one with **`"study": null`**, are both valid per SPEC §4 (docs/SPEC.md:46-53). The Python reference reader verifies them; the Rust reader reports `Integrity` (manifest_hash mismatch), because serde defaults/normalises the field before re-hashing.
- On the unknown-field cases the two readers disagree the other way: Rust B (field added + resealed) → Err, C (field added, old seal) → Ok (field silently dropped). Reference reader: B ok=True, C ok=False.
The fix should hash the canonical form of the **received** JSON object, and its tests should include all four cases plus a cross-reader conformance check.

