# ADR-0066 — Seal the stored canonical bytes, not a re-serialisation

**Status:** Proposed (2026-11-17). The owner decides between the options below; this ADR records them and a
recommendation. Issue: #897. Evidence: [spike #897](../spikes/897-seal-encoding.md) (prototypes and every
number: [`docs/spikes/897-seal-encoding/`](../spikes/897-seal-encoding/README.md)). Holds #895 until decided.

## Context

ADR-0020 D4 defines the seal as `manifest_hash = blake3(JCS(manifest − manifest_hash))`. SPEC §4 has the
verifier compute it over the object it parsed from `manifest.json`, which "MAY be any valid JSON" (SPEC §2,
§6). So the verifier hashes a **re-serialisation** of what it read, not what is stored. Every integrity bypass
so far lived in the gap between the two:

- #573: a typed model dropped unknown keys before re-serialising, so a spliced key verified.
- #873/#883: a record read from a JSON array, a variant from an object, and duplicate keys whose winner differs
  per parser: one sealed text, several readings.
- #886: JCS writes every number as a double, so `9007199254740993` and `9007199254740992` share a seal.

Each was closed by its own rule (verify the parsed object, declared-form check, duplicate-key refusal, I-JSON
integers in PR #894). The gap stays: any byte string whose re-serialisation is the sealed one verifies. The spike's
attack replay shows the class is still open in alpha.6. Over the 13 tracked products, 59 of 59 respellings (key
order, whitespace, `\u` escapes, `64` → `64.0`, pretty layout) keep the seal and verify. On 7 products a sealed
`64.0` in an array shape splits the readers (the spike's case S7): Rust refuses the block, JavaScript reads
`64`, Python a float.

ADR-0020 rejected CBOR as "binary, less legible, no size win at manifest scale" and never weighed the number
model. The owner allows a format break in alpha "if the end result is correct" (2026-11-17); few real `.tsra`
exist.

The documents in scope are everything sealed or signed as JSON: the manifest with its embedded `ProductSchema`,
the `Collection`, `id_inputs`, the signature envelope (ADR-0037) and the ingest `spec_hash` (ADR-0035).

## Options

- **A — keep alpha.6 (JCS + I-JSON, re-serialise and hash), add #895** (schema-declared 64-bit values as strings).
- **A′ — keep JSON, seal the stored bytes.** `manifest.json` is stored as exactly its JCS bytes, without a
  `manifest_hash` key; `manifest_hash = blake3(stored bytes)` lives outside them; readers refuse bytes that are not
  their own JCS form; I-JSON and #895 stay.
- **B — deterministic CBOR** (RFC 8949 §4.2.1 plus a tessera profile) as the stored manifest; JSON only as a
  derived view; seal over the stored bytes; signatures as COSE_Sign1 or the current envelope in CBOR.

Selection criteria (issue #897): an IETF/ISO spec, a standard deterministic encoding, mature readers in
Rust/Python/JS, exact integers. Only JSON+JCS (no exact integers beyond 2^53) and deterministic CBOR pass the rest;
TOML, YAML, RON, Ion, Protobuf, MessagePack and the others are rejected in the issue and the spike.

## Decision (proposed): A′

1. **Stored form.** `manifest.json` MUST be byte-identical to its own RFC 8785 JCS serialisation: no whitespace,
   keys in JCS order, JCS string escapes and number forms, UTF-8. It carries no `manifest_hash` key. Writers emit
   JCS bytes (no pretty printing; `jq .` gives the readable view).
2. **Seal.** `manifest_hash = "blake3:" + hex(blake3(manifest.json bytes))`. Because the stored bytes are the JCS
   bytes A already hashes, **every existing seal keeps its value** (13/13 tracked products, 8/8 goldens).
3. **Where the seal is stored.** In a `manifest.seal` member holding the 71 ASCII bytes `blake3:<64 hex>`,
   entry 2 of the `.tsra` (after `mimetype`, before `manifest.json`, so one ranged read gets both), and as a
   `manifest.seal` file beside `manifest.json` in the exploded form. It is self-attesting, as the in-band field is
   today: it catches accidents, and a missing or mismatching seal is an integrity failure. Trust still comes from
   what references the seal: the signature envelope, `Source` edges, collection member pins, citations
   `id@manifest_hash`, the OCI annotation `vnd.tessera.manifest_hash`. Rejected homes: inside the manifest (only
   possible by hashing "the manifest minus a key", the re-serialisation this ADR removes); an `aux/` member
   (unsealed and optional by definition, ADR-0042); the zip archive comment (forbidden by SPEC §6); the signature
   envelope alone (unsigned products would lose the accident check).
4. **Readers refuse non-canonical bytes.** Verification: strict parse (duplicate keys, lone surrogates, I-JSON
   integers, depth, as today) → JCS-serialise the parsed value → refuse if it differs from the stored bytes → compare
   the seal → typed layer. The canonical check is **required**: the seal alone catches every edit after sealing,
   but not a respelling the attacker seals (`64.0` in an integer field, a pretty layout), which splits readers.
   With the check, one seal has one byte string and one meaning.
5. **Collections** follow the same rules (`collection.json`, `collection.seal`).
6. **Logical `id`** keeps `blake3(JCS(id_inputs))`. JCS is compositional, so those bytes appear verbatim inside the
   stored manifest (13/13), and hashing the slice gives the same `id`.
7. **Numbers.** I-JSON stays at every writer door. 64-bit values are decimal strings whose type the schema
   declares (#895, the JSON-LD `xsd:long` pattern); an untyped integer beyond ±(2^53−1) in an opaque object is
   refused with the advice to write a string. Readers keep the I-JSON check too: rule 4's byte comparison alone
   accepts an integer beyond 2^53 that a double holds exactly (`9007199254740992`, `1730000000123456800` are their
   own JCS form), and such a value is what a rounding writer emits (spike S5b).
8. **Signature envelope** (`ed25519-env-v1`) is unchanged and stays valid: of its six bound fields only the
   `manifest_hash` string comes from the product, and its value is preserved. Its payload is a JCS
   re-serialisation of six known string fields after a strict parse, which gives the bypass class nothing to act
   on.
9. **Ingest `spec_hash`** stays `digest(JCS(spec))`: it identifies a recipe parsed from TOML (an identity, like
   `id`), not a stored document's seal.
10. **Migration.** Readers refuse the alpha layout (a pretty `manifest.json` with an in-band `manifest_hash`). A
    `tessera migrate` verb converts it: verify under the alpha rules, then write the A′ form. The seal is unchanged,
    so embedded `aux/signatures/*` and every external reference stay valid. One reading path in readers; the alpha
    path lives only in `migrate`.

### Why not A, why not B

- **A** keeps one seal over many byte strings, and the S7 reader split exists today. Each new spelling class costs
  another rule in Rust and in the Python reference reader.
- **B** (deterministic CBOR with dCBOR-style numeric reduction, sealed over its stored bytes) is equally sound on
  the attack replay: 0 respellings, and 0/7 sealed reader splits. Numeric reduction is required for that; without
  it, CBOR's separate float and integer types bring back the S7 split, because JavaScript can't tell `64.0` from
  `64`. B is the only option that stores 64-bit integers natively. Its cost: it moves every seal, so every alpha
  signature, `Source` edge and pin breaks. None of the four decoders tested (`cbor2`, `cbor-x`, `minicbor`,
  `cbor4ii`) refuses non-deterministic input, and none of the writers reduces floats, so B needs a strict decoder
  and a reducing writer of our own in Rust, Python and JavaScript. It also needs a decoder to read at all.
  #895's typed strings give A′ exact 64-bit values in all three readers. **B is the better choice if native
  64-bit integers in schema-less places are a hard requirement**; then ship it with numeric reduction and the
  seal over the stored bytes.

## Consequences

- The seal is the hash of a file: `b3sum manifest.json` checks it, any language checks it without a JSON library,
  and the repository's `objects/<manifest_hash>` holds exactly the bytes that hash to its name (ADR-0036's content
  addressing becomes literal).
- No new dependency. Stock JCS libraries reproduce the stored bytes exactly (Python `jcs` and JS `canonicalize` on
  13/13 manifests and 200,000/200,000 doubles).
- A third-party writer that emits "sorted, compact JSON" without a JCS library is refused (Python's `json.dumps`
  writes `1.0`, `1e-07`, `-0.0`). That is a loud failure at the writer, never a second seal.
- The stored manifest is one line (27,103 B for the 13 products vs 40,804 B pretty).
- Every reader (Rust, the Python reference reader, `tessera-wasm`) must run the canonical check and I-JSON; a
  reader that only hashes would accept a sealed respelling, one without I-JSON a rounded integer. The seal cases
  must carry both.
- Option C (per-field commitments for PHI redaction, ADR-0040) is not decided here; it needs salted per-field
  digests inside the document under either encoding (SD-JWT-style in JSON), and A′ doesn't block it.

## Amends and supersedes

- **ADR-0020 D4**: the seal is over the stored canonical bytes, and `manifest_hash` is no longer a manifest field.
  D5's three hashes keep their meanings and values. PR #894's I-JSON amendment stays (rule 7).
- **ADR-0022 and SPEC §6**: the member list gains `manifest.seal` (entry 2), and `manifest.json` loses "any valid
  JSON".
- **ADR-0054**: its "load-bearing fact, the seal is over a canonicalization, not over file bytes" becomes "the
  seal is over the canonical bytes, stored as such". The protected property (codec independence of the spine) is
  unchanged and now enforced at read. Its remark on dCBOR is answered by the spike.
- **ADR-0037**: no change to the envelope or its values; the seal it binds is read from `manifest.seal`.
- **ADR-0060**: the newer-field rules are unchanged (a newer key verifies, is warned about and blocks a re-seal).
  §2's mechanism "seal over the parsed object" is replaced by "seal over the stored bytes".
- **SPEC §2, §4, §7, §9**: canonical stored form, seal definition, verification order, conformance vectors.

## Implementation plan (ordered; issues are filed only after acceptance)

1. SPEC §2/§4/§6/§7/§9 and the ADR amendments above.
2. tessera-core: `canonical::check_canonical`; `Manifest`/`Collection` stored-bytes constructor and writer returning
   `(bytes, seal)`; `manifest_hash` leaves the typed record; seal cases as `(bytes, seal)` pairs, including the
   respelling and S7 cases.
3. tessera-io: container (seal member, member set), exploded form, repository objects, `WriteSession`, sign/verify;
   OCI unchanged.
4. Call sites in tessera-cli, tessera-ingest, tessera-py and tessera-wasm.
5. Corpus: re-pack the 13 `.tsra` (goldens unchanged), rewrite `seal-cases.json`, refresh snapshots.
6. Python reference reader: seal = `blake3(file)`, canonical check with `jcs`, explicit lone-surrogate refusal.
7. `tessera migrate`.
8. #895 (schema-typed 64-bit strings), unheld.
