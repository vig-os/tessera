# ADR-0066 — Seal the stored canonical bytes, not a re-serialisation

**Status:** Accepted (2026-11-18); proposed 2026-11-17. Issue: #897. Evidence:
[spike #897](../spikes/897-seal-encoding.md) (prototypes and every number:
[`docs/spikes/897-seal-encoding/`](../spikes/897-seal-encoding/README.md)).

**Decision:** option **A′**, chosen by the owner on 2026-11-18 (#897, PR #900). JSON stays; `manifest_hash` moves
out of the manifest; the seal hashes the stored bytes; writers emit JCS form; readers refuse non-canonical bytes
and keep I-JSON; 64-bit values are schema-typed strings (#895, no longer held). A and B are kept below as rejected
alternatives with their reasons. The amendments to other ADRs (§ "Amends and supersedes") land with the
implementation, not with this ADR.

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
attack replay shows the class is still open in alpha.6. Over the 13 tracked products (12 distinct manifests: the
two `study.tsra` fixtures are byte-identical), 59 of 59 respellings (key order, whitespace, `\u` escapes,
`64` → `64.0`, pretty layout) keep the seal and verify. On 7 products a sealed
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

## Decision: A′

1. **Stored form.** `manifest.json` MUST be byte-identical to its own RFC 8785 JCS serialisation: no whitespace,
   keys in JCS order, JCS string escapes and number forms, UTF-8. It carries no `manifest_hash` key. Writers emit
   JCS bytes (no pretty printing; `jq .` gives the readable view).
2. **Seal.** `manifest_hash = "blake3:" + hex(blake3(manifest.json bytes))`. Because the stored bytes are the JCS
   bytes A already hashes, **every existing seal keeps its value** (13/13 tracked products, 12 distinct manifests;
   8/8 goldens).
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
   stored manifest (13/13 products, 12 distinct manifests), and hashing the slice gives the same `id`.
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
10. **No migration verb** (owner, 2026-11-19). There are no real products to keep, so there is no `tessera migrate`
    and no second reading path. A reader refuses a product written before this ADR (a pretty `manifest.json`
    with an in-band `manifest_hash`, no `manifest.seal`) with an error that names the format it found (the
    `tessera_version` the manifest declares and the layout evidence) and the format the reader requires. The corpus
    is re-packed in the cutover; anyone with an alpha product re-ingests it.

    **The format-version string does not change at this break** (decided here). Today `TESSERA_VERSION` is
    `"0.0.0"` (described by ADR-0052's Update of 2026-07-03: the sealed `producer` stamps the format version, and
    both versions are `0.0.0`). ADR-0066 leaves it at `"0.0.0"` for three reasons. (a) The value is sealed inside
    `producer`: a bump would move every `manifest_hash` and lose step 4's proof that the mechanism change kept all
    seals, which is the evidence that A′ changed nothing but where the seal lives. (b) ADR-0052 §1 keeps the format
    deliberately unfrozen before 1.0 (SPEC self-designates pre-1.0), so a break needs no version signal to be
    legitimate; the version is a freeze marker, to be set when the format is frozen. (c) A reader cannot use the
    string to tell the layouts apart anyway: the alpha and the A′ products both declare `"0.0.0"`. The layout
    (`manifest.seal` present, no in-band `manifest_hash`, bytes equal to their JCS form) is what the reader checks,
    so the refusal names the declared `tessera_version` and the missing or extra members.

    If the owner wants a visible version at this break, that is a separate decision that moves every seal and
    belongs in step 8's regeneration.
11. **The break ships alone, before the bridge work** (owner, 2026-11-19). It is its own alpha and lands before the
    C ABI bridge (#454 → #811, ADR-0065), so the bridge is built once against the final format. Two further format
    changes ride the same alpha because each moves hashes anyway: #720 (rule 12) and #828 (rule 13).
12. **#720 rides the break** (owner, 2026-11-19). `content_hash` covers the primary blocks only, as ADR-0028 §4
    already says; derived-sidecar blocks (stats, pyramids, projections, the chunk index's stat columns) leave it.
    `manifest_hash` still commits to every block, so the integrity of the accelerators is unchanged. Today
    `ManifestBuilder::seal` folds every block ref into `merkle_root`, so the Accepted ADR and the code disagree.
13. **#828 rides the break** (owner, 2026-11-19). The per-column codec profile `tessera-table-v2` becomes the default
    table codec profile, replacing today's fixed BtrBlocks + Pco choice. A design note comes first (rules, the
    profile's contents per product type, how the choice is recorded in the manifest, determinism across arches),
    then the code.

### Rejected alternatives

- **A** keeps one seal over many byte strings, and the S7 reader split exists today. Each new spelling class costs
  another rule in Rust and in the Python reference reader.
- **B** (deterministic CBOR with dCBOR-style numeric reduction, sealed over its stored bytes) is equally sound on
  the attack replay: 0 respellings, and 0/7 sealed reader splits. Numeric reduction is required for that; without
  it, CBOR's separate float and integer types bring back the S7 split, because JavaScript can't tell `64.0` from
  `64`. B is the only option that stores 64-bit integers natively. Its cost: it moves every seal, so every alpha
  signature, `Source` edge and pin breaks. None of the four decoders tested (`cbor2`, `cbor-x`, `minicbor`,
  `cbor4ii`) refuses non-deterministic input, and none of the writers reduces floats, so B needs a strict decoder
  and a reducing writer of our own in Rust, Python and JavaScript. It also needs a decoder to read at all.
  #895's typed strings give A′ exact 64-bit values in all three readers. B would have been the better choice only
  if native 64-bit integers in schema-less places were a hard requirement; the owner weighed that against seal
  continuity, no new dependency and `jq` legibility, and chose A′.

## Consequences

- The seal is the hash of a file: `b3sum manifest.json` checks it, any language checks it without a JSON library,
  and the repository's `objects/<manifest_hash>` holds exactly the bytes that hash to its name (ADR-0036's content
  addressing becomes literal).
- No new dependency. Stock JCS libraries reproduce the stored bytes exactly (Python `jcs` and JS `canonicalize` on
  13/13 products (12 distinct manifests) and 200,000/200,000 doubles).
- A third-party writer that emits "sorted, compact JSON" without a JCS library is refused (Python's `json.dumps`
  writes `1.0`, `1e-07`, `-0.0`). That is a loud failure at the writer, never a second seal.
- The stored manifest is one line (27,103 B for the 13 products, 12 distinct manifests, vs 40,804 B pretty).
- Every reader (Rust, the Python reference reader, `tessera-wasm`) must run the canonical check and I-JSON; a
  reader that only hashes would accept a sealed respelling, one without I-JSON a rounded integer. The seal cases
  must carry both.
- **Seals move, in steps.** The A′ mechanism keeps every existing seal (13/13 products, 12 distinct manifests,
  proven in the cutover step on its own head). #895 can move the seals of products that embed a built-in schema,
  #720 moves `content_hash` of every product that carries a derived-sidecar block, and #828 moves the block digests,
  hence `content_hash` and `manifest_hash`, of every table product whose codec choice changes. Each such step
  regenerates the goldens it moves in its own PR. All land in the same alpha, so no release carries an intermediate
  format. Signatures made before the alpha stop verifying against the new seals; with no real products that is
  accepted.
- **No upgrade path.** Alpha products are refused, not converted (rule 10). The refusal names both formats so the
  cause is clear.
- The bridge (#454 → #811) waits for this alpha (rule 11).
- Option C (per-field commitments for PHI redaction, ADR-0040) is not decided here; it needs salted per-field
  digests inside the document under either encoding (SD-JWT-style in JSON), and A′ doesn't block it.

## Amends and supersedes

- **ADR-0028 §4** is not amended: #720 makes the code do what it says (rule 12).
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

## Implementation plan

Ordered; the architect files one issue per step as a sub-issue of #897. Steps 1–3 prepare the break (step 1
can move the seals of products that embed a built-in schema; steps 2 and 3 move none); step 4 is the A′ cutover;
steps 5–7 ride the same alpha; step 8 is the final cross-check; step 9 cuts it.

**Goldens between steps (rule).** Every step's PR keeps CI green on its own. A step that moves a seal regenerates
the goldens it moves in that same PR and explains each one (which hash, which product, why). Step 8 is then the final
cross-check of the whole corpus against the three causes, not the only regeneration. Per rule 11 nothing from the
bridge work (#454 → #811) starts before step 9 is released.

1. **#895: product schemas declare 64-bit string encodings and value ranges.**
   - *Crates:* `tessera-core` (`schema`, `FieldSpec`), `tessera-ingest` (the hard-coded DICOM SV/UV and curated range
     checks read the schema), `tessera-py` (`Builder`) and the Python reference reader.
   - *Depends on:* nothing; may run in parallel with 2 and 3.
   - *Scope:* as written in #895. A `FieldSpec` whose `dtype` can exceed ±(2^53−1) (`u64`, `i64`) declares a string
     encoding; optional inclusive `minimum`/`maximum`; built-in schemas declare their rules. The bound stays a
     format invariant (rule 7). Recorded in the amendments of 2026-11-19 to
     [ADR-0020](0020-canonical-encoding-and-identity.md) and
     [ADR-0060](0060-agentic-actors-and-prov-mapping.md).
   - *Acceptance:* a schema with a bare `u64` metadata field is refused; an out-of-range value is refused naming the
     field and its source; in-range values are unchanged. The built-in schemas are embedded in the manifests, so a
     schema that gains a declaration moves the seal of the products that embed it. This PR regenerates those
     goldens and explains each. Step 4 then proves seal preservation against the goldens as they stand on its
     base. The `Builder` and the reference reader follow the same rule.
2. **Writers emit the JCS form of every sealed JSON document.**
   - *Crates:* `tessera-io` (`pack`, `pack_dir`, exploded form, `WriteSession`, repository `objects/`),
     `tessera-cli` (`commit`/`publish`/`seal`, collection writer), `tessera-ingest` (`collection.json`),
     `tessera-py`.
   - *Depends on:* nothing.
   - *Scope:* every door that writes `manifest.json` or `collection.json` writes exactly `JCS(object)`, still with the
     in-band `manifest_hash` key; readers are unchanged. Docs and snapshots that print `manifest.json` show the
     single-line form, with `jq .` as the readable view.
   - *Acceptance:* a test per door asserts `bytes == JCS(parse(bytes))`; the 13 tracked `.tsra` are re-packed with
     `corpus.json` unchanged; the reference reader's `run.py` passes unchanged.
3. **The stored-bytes API and the canonical-bytes check.**
   - *Crates:* `tessera-core`.
   - *Depends on:* nothing (additive).
   - *Scope:* `canonical::check_canonical(bytes)` (strict parse: duplicate keys, lone surrogates, I-JSON, depth; then
     refuse bytes that differ from their own JCS form, naming the first differing byte); `Manifest::from_stored(bytes,
     seal)`, `Collection::from_stored(bytes, seal)` and `to_stored()` on each, returning `(bytes, seal)` with no
     `manifest_hash` in the bytes; typed errors for non-canonical bytes and seal mismatch.
   - *Acceptance:* unit tests over the spike's cases: T4–T7 and S8 refused as non-canonical, S5b refused by I-JSON (a
     test shows the byte comparison alone would pass it), S7 (`64.0`) refused; for all 13 tracked manifests
     `blake3(to_stored().0)` equals the recorded seal.
4. **The A′ format cutover (closes #901).** One PR, because the layout changes once.
   - *Crates:* `tessera-core`, `tessera-io`, `tessera-cli`, `tessera-ingest`, `tessera-py`, `tessera-wasm`, the Python
     reference reader, `corpus/`, SPEC and the ADRs.
   - *Depends on:* 2 and 3 (1 is not required).
   - *Scope:*
     - SPEC §2/§4/§6/§7/§9 and the dated amendments to ADR-0020 D4, ADR-0022, ADR-0037, ADR-0054 and ADR-0060.
     - The container gains the `manifest.seal` member (entry 2) and the stricter member set; the same for the
       exploded form; collections get `collection.seal`.
     - Every reader verifies through `from_stored`; the in-band `manifest_hash` key is refused.
     - **The alpha layout is refused, with no migration verb (rule 10):** the error names the format found and the
       format required.
     - Repository objects hold the stored bytes; `WriteSession`, sign and verify read the seal member (the envelope
       is unchanged).
     - The Python reference reader: seal = `blake3(file)`, canonical check with `jcs`, I-JSON kept, explicit
       lone-surrogate refusal.
     - `seal-cases.json` rewritten as `(bytes, seal)` vectors, including T4–T7, S5b, S7 and S8; the 13 tracked
       products re-packed.
   - *Acceptance:*
     - All 13 re-packed products (12 distinct manifests) verify with every `corpus.json` golden unchanged from its
       base (the seal-preservation proof; it must hold on this step's own head, whatever step 1 had already
       regenerated, and before 5 and 7 move any hash).
     - An alpha-layout product is refused with an error naming both formats.
     - Rust, the reference reader and `tessera-wasm` reach the same verdict on every seal case.
     - #901's case (a sealed `64.0` in an integer field) is refused by all three.
     - A test per writer door shows the written `manifest.seal` equals `blake3(manifest.json)`.
5. **#720: `content_hash` covers the primary blocks only (ADR-0028 §4).**
   - *Crates:* `tessera-core` (`ManifestBuilder::seal`, a `canonical | derived` class on the block ref), `tessera-io`
     (the writers that attach `.cidx`, pyramid and stats blocks), `tessera-py`, the reference reader, SPEC.
   - *Depends on:* 4 (so the seal-preservation proof of step 4 stays clean). Its design decision is recorded in the
     issue, since #720 names the manifest representation of the class and the `manifest_hash` rule as open.
   - *Scope:* the block class in the manifest; `content_hash` over primary blocks; `manifest_hash` still over every
     block (rule 12).
   - *Acceptance:* a test that fails today: adding or regenerating a derived block leaves `content_hash` unchanged
     and changes `manifest_hash`; changing a primary block changes both.
     The goldens this moves are regenerated in this PR, each explained.
6. **#828, design note: `tessera-table-v2`.**
   - *Crates:* none (a document under `docs/`, plus the ADR or ADR amendment it needs).
   - *Depends on:* nothing; should finish before 7 starts and may run in parallel with 1–5.
   - *Scope:* the per-product-type profile table from the #828 sweep, how the profile is named in the manifest,
     the speed/size trade-off the default takes, and the determinism requirement across arches (ADR-0026, #468,
     #472). Owner review before 7.
   - *Note:* [`docs/spikes/915-tessera-table-v2.md`](../spikes/915-tessera-table-v2.md) (#915).
7. **#828: `tessera-table-v2` becomes the default table codec profile.** *Status: implemented in #828 (2026-11-22);
   the rule is recorded in [ADR-0024](0024-table-block-payload.md)'s 2026-11-22 amendment and SPEC §5b, and the
   moved goldens are listed in the #828 PR.*
   - *Crates:* `tessera-io` (`table`), `tessera-core` (the codec field in the table block spec), `tessera-ingest`,
     `tessera-cli`, `tessera-py`, the reference reader, benches.
   - *Depends on:* 4 and 6.
   - *Scope:* implement the note of step 6.
   - *Acceptance:* per the note; round trips are lossless on the #822 fixtures; the cross-arch determinism gate
     passes; size and speed meet the note's numbers.
     The goldens this moves are regenerated in this PR, each explained.
8. **Corpus and conformance regeneration, once.**
   - *Crates:* `corpus/`, `tessera-cli` snapshots, the reference reader, `tessera-wasm`.
   - *Depends on:* 1, 5 and 7.
   - *Scope:* each of steps 1, 5 and 7 already regenerated the goldens it moved, in its own PR (the rule above).
     This step re-runs the whole corpus once more as the final cross-check: any remaining difference is a defect, and
     every moved hash (`content_hash`, `manifest_hash`, block digests) is traced to step 1, 5 or 7. Step 1 (#895)
     is a cause too: the built-in schemas are embedded in the manifests. `id` moves only where #895 touched a field
     that is part of `id_inputs`.
   - *Acceptance:* a per-product table of which hash moved and why; Rust, the reference reader and `tessera-wasm`
     agree on the new corpus.
9. **Cut the alpha.**
   - *Crates:* none (release).
   - *Depends on:* 8 (which depends on 1).
   - *Scope:* release notes name the format break, the refusal of alpha products, the moved hashes and that signatures
     made before it do not verify. The bridge work (#454 → #811) starts afterwards.
