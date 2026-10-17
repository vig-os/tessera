---
type: issue
state: open
created: 2026-10-16T17:15:54Z
updated: 2026-10-16T23:03:16Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/566
comments: 1
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-17T06:32:48.699Z
---

# [Issue 566]: [feat(py/cli): generalize ADR-0047 crypto-shred identity envelope — Python Builder + `tessera attach-identity`](https://github.com/vig-os/tessera/issues/566)

## Context

Dogfooding (anatomist MorePET/anatomist#91) at tessera `dev` `fa581f5`. ADR-0047 implements an elegant crypto-shred pattern — an `age`/X25519-encrypted identity envelope rides in `aux/identity/`, outside the seal; `tessera reidentify` recovers it with the recipient private key, `tessera shred` drops it. The whole pattern is exposed **only** through `tessera ingest dicom --crypto-shred --recipient <age-pubkey>`.

## Gap

The `Builder` pyo3 surface is `add_array / add_table / add_source / set_field / pack`. There is no `add_aux_member` (or `attach_identity`) that would let a Python producer put an age-encrypted identity envelope into `aux/identity/*.age` before `pack()`. Rust-side the primitive exists: `tessera-io::aux::write_aux_members` and the ADR-0047 identity-envelope writer, both consumed by the DICOM ingest path.

Impact: the ADR-0047 pattern is unreachable from any product that isn't produced by `tessera ingest dicom`. Anatomist's steps write their own `.tsra` from Python (ingest and every derived step), so they cannot attach a crypto-shred identity envelope on the Python write path. The workaround — post-process with `tessera attach-identity` — doesn't exist either; the only writer surface for aux is the `sign` command (fixed to signatures).

## Proposed shape

Two complementary pieces, both narrow:

1. **CLI:** `tessera attach-identity <file.tsra> --recipient <age-pubkey> [--recipient …] --identity <doc.json>` — reads a producer-supplied canonical identity JSON (equivalent to ADR-0047 §1's identity document shape), encrypts to the recipient set, writes `aux/identity/identity.age` into the sealed `.tsra` in place. Idempotent; errors if an envelope is already present (use `tessera shred` first). The seal is unchanged (same aux-is-outside-seal invariant as `sign` and `--crypto-shred`).

2. **Python:** `Builder.add_aux_member(name: str, bytes_: bytes)` for a generic path, or a specific `Builder.attach_identity(doc_json: str, recipients: list[str])` that mirrors the DICOM ingest path. The specific one is safer (writes the canonical ADR-0047 shape and enforces the `aux/identity/` prefix); the generic one is more useful for #407.

Either closes the "Python producer can't ride ADR-0047" gap.

## Repro

tessera `dev` `fa581f5`:
```python
import tessera
b = tessera.Builder("rex-fictus-ingest", "array", "...", "2026-01-01T00:00:00Z")
# ... add_array / add_source / set_field ...
# No way here to attach an age-encrypted aux/identity/identity.age before pack.
b.pack("out.tsra")
```
CLI post-process attempt:
```
$ tessera attach-identity out.tsra --recipient age1... --identity doc.json
error: no such subcommand: `attach-identity`
```

## Not goals / linked

- Not re-doing ADR-0055 (whole-file encryption, #378) — this is per-product identity-tier only.
- Not changing the DICOM ingest path.
- Related, not duplicate: #562 (Python Builder lacks `with_producer`/`with_generation`), #563 (`add_source` can't pin content_hash). This is the aux-writer sibling.

## Environment

- tessera `dev` `fa581f56041b38a48a8d20f4a9d033505dad8185`
- CPython 3.12, Nix-built abi3 wheel (`nix build .#wheel`)
- Linux x86_64

---

# [Comment #1]() by [gerchowl]()

_Posted on October 16, 2026 at 11:03 PM_

From MorePET/anatomist#86 (ADR-0007, PR MorePET/anatomist#103): following an owner decision (2026-10-17), anatomist bases its crypto-shredding **on the ADR-0047 identity envelope** rather than on a store of its own. This issue therefore **gates** anatomist's per-dog ingest (#87). What anatomist needs from it, concretely (checked at `dev` fa581f5):

1. **A non-DICOM producer.** anatomist's inputs are NIfTI (`ct.nii.gz`) plus a clinical spreadsheet, not DICOM. Today the envelope is reachable only through `ingest dicom|dicom-series --recipient` (`engine.rs:396/434`), and there is no `ingest nifti` subcommand. `IdentityDocument` requires `dicom_header`. The ask: make `dicom_header` optional, or generalise it (for example `source_header: null`), with `curated_identifying` carrying producer-supplied fields such as a clinic id, a patient id, series ids and source file digests.
2. **Python:** `Builder.attach_identity(doc_json, recipients)` (the safer of the two shapes proposed above), which writes the canonical `aux/identity/identity.age` before or after `pack`, with the seal unchanged. Or the `tessera attach-identity` CLI, which a host-side step can call.
3. **Recipient discipline:** anatomist encrypts each envelope to **exactly one** recipient, the dog's own key. `inspect` (without decryption) reporting the recipient count would let a consumer enforce that.
4. **Binding:** until #407 signs `aux/`, an envelope can be swapped without breaking the product's signature. anatomist will put the product's `manifest_hash` inside its identity document, so a swap is detectable on `reidentify`. A tessera-level binding (the envelope commits to `manifest_hash`) would be better.

