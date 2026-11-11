---
type: issue
state: open
created: 2026-11-10T11:20:08Z
updated: 2026-11-10T11:20:08Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/795
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-11T08:50:45.186Z
---

# [Issue 795]: [feat(core,io): signed live checkpoints over the streaming MMR root — verify blocks before the seal](https://github.com/vig-os/tessera/issues/795)

Follow-up to the DAQ discussion on #203 / #794. A producer (cyclotron DAQ, also host audit
streams via vigil) wants consumers to **pull and verify committed blocks while the product is still
being written**, not only after `seal()`. The pieces exist: the CT-style history tree with a live
root that advances per committed block (ADR-0028 §2, `with_live_index`), and
`tessera_core::hash::{inclusion_proof, consistency_proof, verify_consistency}`. What is missing is
a **signed statement of the live root** during the write — a signed tree head, as in Certificate
Transparency.

## Proposal

### 1. Checkpoint record (canonical JSON, JCS + blake3 like the spine, ADR-0054)

```json
{ "v": 1, "stream": "<session/product id>", "seq": 42, "root": "blake3:…",
  "leaves": 1337, "committed_at": "<RFC 3339>", "prev": "blake3:<hash of checkpoint 41>",
  "key_id": "…", "alg": "ed25519-env-v1", "key_format": "…" }
```

Signed with the existing ADR-0037 envelope; `signed_at`/`key_format` bound as in §0. Emitted by the
ordered committer **every N commits or T seconds (whichever first), and always at `close`/`seal`**.
Stored as `checkpoints.jsonl` next to the journal in the stage dir; carried into the sealed product
as a non-identity `aux/` member (ADR-0042) — **signing never changes identity** (ADR-0037 §3 default).

### 2. What a live consumer verifies

- **A block:** `blake3(block)` → `inclusion_proof` → `root` of checkpoint *k* → signature.
  Same strength as a per-block signature at O(log n) proof size and one signature per checkpoint
  (a TPM signs in tens of ms — per-block signing would not keep up with a DAQ).
- **Append-only:** `consistency_proof(k → m)` — nothing before checkpoint *k* was rewritten.
- **Stats:** the `{hash, stats}` nodes (ADR-0028) put the per-chunk statistics under the same root,
  so live stats are authenticated too (their *correctness* still depends on #633).
- **The seal:** the manifest root must equal the last checkpoint's `root`; the seal signature closes
  the chain. The seal stops being a separate trust event.

The unsigned window is the tail after the last checkpoint — the same bound as the fsync interval;
document it per record kind.

### 3. Keys and counter-signatures

- The producer key is named at `open()` (via `with_producer`, ADR-0058) before the first append.
- Key loaders per ADR-0037 §2's "ssh-agent / PIV / HSM" rung, plus **TPM**: stage 1 a TPM-sealed
  ed25519 seed (systemd-creds, no format change); stage 2 a non-exportable TPM key — most deployed
  TPMs have no Ed25519, so this needs a new `alg` (ECDSA P-256). Coordinated with vigil (issue
  linked below).
- A consumer **counter-signs** checkpoints it has pulled (ADR-0037 persona 3 counter-signatures):
  forward integrity against a later compromise of the producer.

### 4. One checkpoint format with vigil

vigil's audit trail already adopts tessera-core's MMR bit for bit and signs checkpoints over it
(vigil ADR-0005 §7–8). Define the checkpoint record **once, in tessera-core**, and use it for both
tessera streams and vigil trails: one verifier for data, logs and audit.

## Archival rule (ADR-0037 §1)

Everything needed to verify in year 30 is embedded at seal: all checkpoints, the producer key
material/trust snapshot, any counter-signatures. If a TPM quote is ever attached to a checkpoint,
the expected-PCR snapshot must be embedded too; TPM EK certificate chains are not a trust anchor
(X.509/PKI is rejected in §4).

## Acceptance criteria

- [ ] Checkpoint record + canonical encoding specified in SPEC; conformance fixtures (signed
      checkpoints, inclusion + consistency across checkpoints, a tampered block, a rewritten
      prefix, a dropped checkpoint).
- [ ] `StreamWriter` emits checkpoints at the configured cadence and at close/seal; no checkpoint
      ever names a root whose blocks are not journal-durable.
- [ ] Library + CLI verify of a **stage dir in progress**: every committed block up to the newest
      checkpoint verifies; the tail after it is reported as unsigned, not as an error.
- [ ] Sealed product: last checkpoint root == manifest root; checkpoints carried as `aux/`;
      `content_hash` identical with and without checkpoints (property test).
- [ ] Counter-signature of a checkpoint by a second key verifies offline.
- [ ] No change to identity, sealed bytes or the Vortex pin.

## Open questions

- Checkpoints as `aux/` (default) vs a hashed block committed by the manifest (the ADR-0037 §3
  fork — would allow `attestation.required` hard-reject of stripped checkpoints).
- New `alg` for TPM keys (ECDSA P-256) vs staying ed25519-only with sealed seeds.
- Cadence defaults per record kind (per-run product vs 24/7 machine log).
- The read API for a live puller over an open stage dir — overlaps with the DataFusion provider
  over committed-but-unsealed fragments (kit DAQ ADR-0010, related to #654).

## Related

- vig-os/vigil#58 — hardware checkpoint signers (TPM), algorithm agility, shared checkpoint record
- vig-os/vigil#59 — tessera adoption path for vigil trails
- vig-os/vigil#214 — journald cursor bridge (host audit streams that would carry these checkpoints)
- #203, #794, #654, #633

