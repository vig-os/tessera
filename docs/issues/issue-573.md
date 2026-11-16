---
type: issue
state: closed
created: 2026-10-16T21:45:17Z
updated: 2026-11-15T21:30:11Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/573
comments: 3
labels: bug, area:core, area:integrity
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-16T09:00:47.985Z
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

---

# [Comment #2]() by [gerchowl]()

_Posted on November 15, 2026 at 04:02 PM_

**Mirror case, found in the PR #862 round-3 review at 38b6d61.** A `manifest.json` with an extra top-level key (`"forged": true`) passes `tessera verify`: the output is OK, and `unzip -p … manifest.json` shows the key. The in-repo reference reader reports `ok=False`.

**Cause:** this is the same root cause as this issue. `Manifest` has no `deny_unknown_fields`, and the seal is computed over the re-serialised struct rather than over the bytes in the file. So the unknown keys a newer writer adds are dropped, which is #573 as reported. Unknown keys that a tamperer adds are dropped the same way, which is the new case: the seal covers bytes other than what other readers see.

**Fix direction:** decide the rule once.
- Either the seal covers the raw manifest (JCS over the parsed JSON `Value`, keeping unknown fields), so both newer writers and tampering are handled correctly.
- Or `manifest.json` must be exactly its canonical serialisation.

Both cases need a test that fails today. This is integrity-critical, so it should go before alpha.6.

---

# [Comment #3]() by [gerchowl]()

_Posted on November 15, 2026 at 08:33 PM_

Owner decisions 2026-11-15: alpha.6 is cut after #573 and #734 land, as one security release; one GitHub security advisory covers ≤ alpha.5 (the zip reader differential plus the manifest-seal bypasses), drafted for owner review; #869: record the codec-profile id beside the edition.

