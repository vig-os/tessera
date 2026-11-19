---
type: issue
state: open
created: 2026-11-17T19:56:04Z
updated: 2026-11-19T05:25:55Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/897
comments: 9
labels: priority:high, semver:major, spike, area:integrity, adr
assignees: none
milestone: none
projects: none
parent: none
children: 895, 912, 913, 914, 720, 915, 828, 916, 917
synced: 2026-11-19T09:52:10.991Z
---

# [Issue 897]: [spike(format): revisit the seal encoding: JCS+I-JSON vs deterministic CBOR vs per-field Merkle (ADR-020)](https://github.com/vig-os/tessera/issues/897)

## Question
ADR-020 (ROADMAP D4) chose JCS (RFC 8785) over deterministic CBOR for the seal. #886 showed what that costs: sealed JSON numbers are floats, so metadata integers are capped at ±(2^53−1). Real values such as epoch-ns timestamps (~1.7e18) must be stored as strings and lose their type. The owner (2026-11-17) will accept breaking the format in alpha if the result is correct. Should the seal format change before 1.0?

## Options to evaluate (spike → ADR amending or superseding ADR-020)
- **A. Keep JCS + I-JSON** (alpha.6 as shipped), and let schemas declare 64-bit fields stored as strings (#895). Smallest change. Large integers keep their type only through the schema.
- **B. Store the manifest as deterministic CBOR** (RFC 8949 §4.2 core deterministic encoding, or the dCBOR profile). Native u64/i64/bignum, a byte-string type for digests, valid UTF-8 required, sorted keys, no duplicate keys. JSON becomes a derived *view*.
  - This only closes the problem if the **stored, sealed** form is CBOR. Sealing CBOR while readers still parse a stored JSON manifest keeps the JS/float reader disagreement.
  - Check: readers in Rust (ciborium/minicbor), Python (cbor2) and JS/wasm (cbor-x → BigInt). Is there a determinism conformance corpus? What happens to human inspection (`unzip -p … | jq`), FAIR, the RO-Crate/DataCite projections, signatures (ADR-0037), collections, `id_inputs`, `.tsra` layout (SPEC §6) and the wasm verifier?
- **C. Per-field Merkle over the metadata tree** (objecthash / SD-JWT style), outer layer of hashes. Closes the hash collision, but not reader disagreement on number values. Its real value is per-field proof and redaction of PHI (ADR-0040 redact phase). It can combine with A or B.

## Acceptance
- A spike doc plus an ADR with a pre-mortem / design audit, decided by the owner, **before** any implementation.
- If B or C is chosen: ordered sub-issues (format, readers, wasm, Python reference reader, corpus regen, SPEC, migration note). One planned breaking alpha, not several.
- alpha.6 ships with A (it closes #886 now and fails closed); this spike does not block it.

---

# [Comment #1]() by [gerchowl]()

_Posted on November 17, 2026 at 07:56 PM_

Context for the spike:
- ADR-0020 rejected CBOR only on legibility grounds: "binary, less legible, no size win at manifest scale". It never weighed the number model (floats, so a ±2^53 cap) or digests stored as hex text rather than bytes, which is what #886 exposed.
- Owner, 2026-11-17: there are few real `.tsra` files in use, so breaking the format now costs little and gets more expensive after 1.0.
- #895 (schema-declared string encodings) is **held** until this spike decides, because option B would make it moot.

---

# [Comment #2]() by [gerchowl]()

_Posted on November 17, 2026 at 08:00 PM_

Notes from the owner discussion (2026-11-17):
- **Legibility under CBOR:** `unzip -l` still works, but `unzip -p … manifest.cbor` is binary. Agents and humans need a decoder: `tessera inspect --json`, `python -m cbor2.tool`, or the `cbor-diag` CLI.
  - A stored, unsealed `manifest.json` copy is **rejected**: it would become a second, unverified view, the same class as the zip differential.
  - The spike should check whether the zip itself can carry a pointer to the decoders (README entry or archive comment).
- **TOML:** integers are exact signed 64-bit, and out-of-range values must be refused by the spec. But it has no u64 above 2^63, no bignums and no null, nesting is awkward, and there is no canonical form standard, so we would define our own.
- **YAML:** rejected. Implicit typing (the Norway problem, YAML 1.1 vs 1.2), several spellings per value, and anchors/aliases (billion-laughs blowup) make it a differential generator. Also no canonical form standard.

---

# [Comment #3]() by [gerchowl]()

_Posted on November 17, 2026 at 08:01 PM_

More candidates (owner, 2026-11-17):
- **RON:** rejected.
  - No independent spec; the `ron` crate *is* the format.
  - Practically Rust-only, with no maintained Python or JS reader. That breaks the independent Python reference reader (D6) and the wasm/JS reach.
  - No canonical form.
  - The text carries Rust struct and enum names.
- **JSON5 / KDL / EDN:** no canonical form; only JSON5 is cross-language, and it is no better than JSON on numbers.
- **Amazon Ion:** typed arbitrary-precision ints and decimals, with isomorphic text and binary forms. But it has no standard canonical encoding for hashing and is a niche ecosystem. Worth one paragraph in the spike, not a candidate.
- **Selection criteria for the spike:**
  1. an IETF/ISO spec (not one implementation);
  2. a standard deterministic encoding;
  3. mature readers in Rust, Python and JS;
  4. exact integer types.
  Only JSON+JCS (fails 4) and deterministic CBOR (RFC 8949 §4.2) pass the rest.

---

# [Comment #4]() by [gerchowl]()

_Posted on November 17, 2026 at 08:01 PM_

Binary candidates (owner, 2026-11-17):
- **serde is not a format.** It is Rust's serialization framework; formats plug into it. CBOR via `ciborium`/`minicbor` *is* the serde+binary route.
- **Protobuf:** rejected.
  - Upstream docs say its deterministic mode is not canonical across languages or versions (map order, unknown fields, default omission).
  - Data is unreadable without the `.proto` (field numbers only), so it is not self-describing, which hurts FAIR.
- **FlatBuffers / Cap'n Proto:** need a schema to read, and the bytes depend on builder choices (padding, vtable dedup), so there is no canonical form. Zero-copy is irrelevant at manifest size.
- **bincode / postcard:** Rust-only in practice, not self-describing, layout tied to Rust types, and a moving spec (bincode 1→2).
- **MessagePack:** self-describing with typed u64/i64, but no deterministic-encoding standard. CBOR (RFC 8949) is effectively MessagePack standardised, with a deterministic profile.
- **BSON / Avro:** BSON preserves insertion order and has no canonical form; Avro data needs its schema, and only the schema has a canonical form.
- **Conclusion:** deterministic CBOR remains the only binary candidate that meets all four criteria.

---

# [Comment #5]() by [gerchowl]()

_Posted on November 17, 2026 at 08:05 PM_

**Library maintenance check (2026-11-17), asked by the owner ("cbor's gh seems stale"):**

| Library | Language | Last release | Repo activity | Notes |
|---|---|---|---|---|
| ciborium 0.2.2 | Rust | 2024-01 | pushed 2026-10, 57 open issues | the popular serde one; releases stale |
| serde_cbor | Rust | 2021 | n/a | dead, avoid |
| minicbor 2.3.1 | Rust | 2026-10 | active | — |
| cbor4ii 1.2.5 | Rust | 2026-11 | active | — |
| coset 0.4.2 (Google, COSE) | Rust | 2026-03 | active | — |
| dcbor 0.25 (Blockchain Commons) | Rust | — | — | pre-1.0, niche |
| cbor2 | Python | — | active 2026-11 | — |
| cbor-x | JS | — | active 2026-11 | — |

**Status:** the owner asked "are we sure to take that route?". Not decided yet. This spike must produce evidence (a prototype bake-off) before any implementation issue is filed.

---

# [Comment #6]() by [gerchowl]()

_Posted on November 17, 2026 at 08:06 PM_

**How comparable formats solve this (owner question, 2026-11-17)** — this adds a third option, A′.

- **Most hash the stored bytes and never re-canonicalise:**
  - OCI image manifests: the digest covers the exact blob bytes and lives *outside* the blob, in the referencing descriptor.
  - BagIt (RFC 8493) and RO-Crate+BagIt: checksums over files as stored.
  - git: objects are hashed as stored.
  - DSSE (in-toto/sigstore): signs the exact payload bytes and abandoned canonical JSON on purpose.
- **Large integers:**
  - RO-Crate (JSON-LD) writes typed literals: `{"@value": "1730000000000000000", "@type": "xsd:long"}`, a string plus a declared type. That is option A with schema typing (#895).
  - TUF's canonical JSON forbids floats outright.
- **Canonicalisation and CBOR users:** W3C Verifiable Credentials use JCS or RDF canonicalisation and hit the same I-JSON limits. C2PA (content provenance) uses CBOR + COSE.

**A′ — JSON kept, seal over the stored bytes:**
- `manifest_hash` moves out of the manifest (to a seal entry or the signature envelope), so the seal hashes `manifest.json` exactly as stored. This removes the #573/#873 re-serialisation class by construction.
- Writers still emit JCS form, for determinism (S15).
- I-JSON stays, and 64-bit values are typed strings declared by the schema (#895, JSON-LD style).
- The logical `id` keeps JCS over the small, known `id_inputs`.
- **Cost:** a format change to where `manifest_hash` lives. JSON stays readable, and no new dependency is needed.

The bake-off should compare A (as shipped plus #895), A′ and B (deterministic CBOR).


---

# [Comment #7]() by [gerchowl]()

_Posted on November 18, 2026 at 12:22 AM_

**Owner decision (2026-11-18): option A′.** JSON stays. `manifest_hash` moves out of the manifest, the seal hashes the stored bytes, writers emit JCS form, and readers refuse non-canonical bytes and keep I-JSON. 64-bit values are schema-typed strings (#895, now un-held). Evidence: PR #900 (spike + ADR-0066). Next steps: ADR-0066 → Accepted on PR #900 after a fresh review, then ordered implementation sub-issues taken from the ADR's plan. B (deterministic CBOR) is rejected for the reasons recorded in the spike.

---

# [Comment #8]() by [gerchowl]()

_Posted on November 19, 2026 at 12:16 AM_

Owner decisions 2026-11-19:
- **Order:** the ADR-0066 format break ships before the bridge.
- **Migration:** none. There are no files to keep, so old products are refused and no `tessera migrate` verb is built.
- **Same breaking release:** #720 (content_hash covers primary blocks only, as ADR-0028 §4 says) and #828 (codec profile `tessera-table-v2` as the new default) ride with it.

---

# [Comment #9]() by [gerchowl]()

_Posted on November 19, 2026 at 05:25 AM_

ADR-0066 is Accepted and merged (#900, 7b5cedd). Its implementation plan is filed as the sub-issues of this issue, in this order:

| Step | Issue | What | Depends on |
|---|---|---|---|
| 1 | #895 | product schemas declare 64-bit string encodings and value ranges | — |
| 2 | #912 | writers emit the JCS form | — |
| 3 | #913 | stored-bytes API and canonical check (core) | — |
| 4 | #914 | A′ cutover; closes #901 | 2, 3 |
| 5 | #720 | content_hash covers primary blocks only | 4 |
| 6 | #915 | tessera-table-v2 design note (owner review) | — |
| 7 | #828 | tessera-table-v2 becomes the default | 4, 6 |
| 8 | #916 | final corpus cross-check | 1, 5, 7 |
| 9 | #917 | cut the format-break alpha | 8 |

The bridge work (#454 → #811) starts after #917 ships. Review nits from #900 are filed as #918.


