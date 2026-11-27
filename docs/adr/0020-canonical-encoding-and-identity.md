# ADR-0020 — Canonical encoding + identity model + manifest/BlockRef schema

**Status:** Accepted (P0) · **Register:** D4, D5 · **Issue:** #195

## Context
Tessera products are content-hashed and immutable. For that to mean anything across machines,
language implementations, and time, three things must be pinned: (1) how the manifest is serialized
when hashed (any byte difference changes the hash), (2) what "identity" is and how it differs from
"contents", and (3) the exact field set + invariants of the manifest and block references. The spike
hashed identity over an ad-hoc ``-joined string and rolled only block digests into
`content_hash` — so the manifest's own metadata (name, sources, schema) was **not** tamper-evident,
and the hash wasn't reproducible by a second implementation.

## Decision

### D4 — Canonical encoding = RFC 8785 JCS
All hashing is over **RFC 8785 JSON Canonicalization Scheme** bytes (`serde_jcs`): lexicographically
sorted object keys, no insignificant whitespace, ECMAScript-canonical number forms. JSON (not CBOR)
because the manifest is JSON-native, human-readable, and FAIR-legible; JCS is the established standard
for deterministic JSON and is trivially reproducible by any language (JS/Python/Go/Rust all have it).
CBOR was rejected: binary, less legible, no size win at manifest scale.

### D5 — Three distinct hashes
- **`id`** — *logical identity*. `blake3:` over `JCS({id_inputs})`, where `id_inputs` is a declared,
  ordered key→value map (default keys: `product`, `name`, `timestamp`). Stable across re-ingest of the
  *same logical product* and rename-safe (renaming a file or re-encoding the bytes does not change it).
  `id` is **not** the Merkle root — re-deriving an equivalent product keeps `id` even if storage bytes
  differ. The declared `id_inputs` are recorded in the manifest so identity is transparent + verifiable.
- **`content_hash`** — *data fingerprint*. Merkle root (`blake3` hash-of-hashes) over the **ordered**
  per-block digests. Changes iff payload bytes change. Integrity-only (random access is Vortex/zarrs'
  job, not this tree).
- **`manifest_hash`** — *the seal*. `blake3:` over `JCS(manifest)` with the `manifest_hash` field
  itself omitted. Covers the **entire** manifest — metadata, `id_inputs`, `sources`, and every
  `BlockRef` (which carries its block digest), so it transitively commits to all payloads too. This is
  the single root a signature / container index / WORM record binds to. Tamper any byte anywhere →
  `manifest_hash` changes. "Seal = hash of hashes" = `manifest_hash` over a manifest that embeds
  `content_hash`.

### Manifest & BlockRef schema (normative field set)
`Manifest` = `tessera_version` · `id` · `id_inputs` (ordered map) · `product` · `name` · `description`
· `timestamp` (RFC 3339, normalized to UTC) · `schema?` · `blocks: [BlockRef]` (order is significant —
fixes the Merkle order) · `sources: [Source]` · `content_hash?` · `manifest_hash?`. The two hash
fields are `None` while building, `Some` after `seal()`.
`BlockRef` = `name` · `kind` (`array`|`table`) · `digest` (required at seal) · `spec` (shape-specific,
opaque to the core). A missing digest at seal is a hard error (else a block would be invisible to the
content hash yet present in the manifest).

### Verification
`Manifest::verify()` recomputes and checks all three hashes: `id` from `id_inputs`, `content_hash`
from block digests, `manifest_hash` from the canonical bytes — returning typed `Integrity` errors that
name the field, expected, and actual values. Re-serializing a parsed manifest through JCS reproduces
`manifest_hash` byte-for-byte (the conformance "canonical encoding" gate).

## Consequences
- Hashing is reproducible across implementations and versions → unblocks fixtures, the conformance
  corpus, and the v1.0 independent reader.
- The whole manifest is now tamper-evident (closes the metadata/sources gap).
- Pre-1.0, so changing the `id`/seal derivation now (no persisted products yet) is free.
- `serde_jcs` is the one new core dependency (pure Rust; `itoa`/`ryu` for canonical numbers).

## Reconciliation — `content_hash` construction superseded by ADR-0028
The `content_hash` defined here is the **single-level / flat** root over the ordered block digests
(`blake3` of the concatenated digests — one level, no interior tree). **ADR-0028 supersedes this
construction** with a **recursive MMR** root (a node-hash applied at every level + an append-friendly
history tree). That is a deliberate **v0.2 identity revision** — `content_hash` values change and the
golden corpus regenerates with it. Scope of the supersession is **only the `content_hash` construction**:
this ADR's `id` (logical identity), `manifest_hash` (seal), JCS canonicalisation, BlockRef/Manifest
schema, and error taxonomy are **unaffected and remain Accepted**.

**As-built (2026-06-26):** the MMR root (ADR-0028 §1–2) is now implemented in `tessera_core::hash`
(domain-separated leaf `0x00` / node `0x01`, peaks-carry, bag-the-peaks; incremental == batch). The flat
construction is **superseded** — `content_hash` is the recursive MMR root, and the golden corpus, SPEC §3,
and the pure-Python reference reader were regenerated to it (conformance 3/3 + reference reader 6/6).
(ADR-0028 remains **Proposed** for its further parts — chunk-index, pyramid, sidecars, fused pass.)

## Amendment (2026-11-16) — JCS input is I-JSON; integers beyond ±(2^53−1) are refused (#886)
D4 pinned the canonical *output* but not its *input*. RFC 8785 accepts only I-JSON (RFC 8785 §3.1,
RFC 7493) and serialises every number as an IEEE-754 double (RFC 8785 §3.2.2.3), so an integer beyond
±(2^53−1) is rounded before it is hashed: a manifest sealed with `9007199254740993` verified when served
with `9007199254740992`. The seal did not pin the value. Python's `jcs` agrees with `serde_jcs`, so
readers never disagreed; the hash was simply not exact.

**Decision (owner, 2026-11-16).** Tessera enforces JCS's own input rule in every sealed JSON document:
the manifest (embedded schema, metadata, `generation`, `ingest_transform` params), `id_inputs`, a
collection and a signature envelope. One check, `tessera_core::canonical::check_i_json`, refuses a number
whose value as parsed is an integer beyond ±(2^53−1), with `Error::InexactInteger` naming its path. It
runs in `canonical::parse_strict` (every reader) and in `canonical::to_bytes` (every writer, before any
byte is hashed, so a `u64`/`i64` from Rust code is refused too); a streamed `WriteSession` applies it
when a block or a metadata, `extra` or recipe value is declared, not hours later at the seal. The rest
of I-JSON's MUSTs were already enforced there (UTF-8 without lone surrogates, unique names). Large values travel as decimal strings;
user input (Python `set_field`/`with_generation`, a TOML spec's `[product.metadata]`, CLI
`commit --set` and `ingest … --meta`) is refused with that advice and never converted. SPEC §1 states the rule.

**User-typed numbers and DICOM (owner, 2026-11-17).** A value a person types on the command line is read by
`canonical::parse_user_value`: text that is JSON by the RFC 8259 grammar is JSON, and a number in it beyond the
bound, or past a double's range (`1e400`, a 401-digit integer, `[1e400]`), is refused naming the flag and the typed
text. It is never kept as a string, which the CLI used to fall back to whenever `serde_json` could not hold the
number. Text that is not JSON but that Rust's `str::parse::<f64>()` reads as a number (`+5`, `.5`, `5.`, `01`, `inf`,
`Infinity`, `NaN`, `+1e400`) is a number the person meant, spelled in a way JSON does not allow: it is refused, naming
the flag and key and saying why (not finite, past a double's range, or not JSON number spelling: "write `5`, not
`+5`"), and the error shows how to keep the text, the JSON-quoted form `--set k='"+5"'`. Non-numeric text (`hello`,
`0x10`, `1_000`, `1.2.3`) is still a bare string. `NaN` and ±infinity have no JSON spelling, and `serde_json` writes
them as `null`, so a seal would have pinned `null`: every f64→JSON conversion on a write path goes through
`canonical::json_number` (one value) or `canonical::to_json_value` (a typed document), which refuse them
(`Error::NonFiniteNumber`); a TOML spec's `inf`/`nan` is refused at `spec::parse_str`. DICOM curated `DS`/`FD` fields (`KVP`, `SliceThickness`, `PixelSpacing`,
`RescaleSlope`/`RescaleIntercept`, a series' `ImagePositionPatient`/`ImageOrientationPatient`) stay JSON numbers; one
beyond the bound is refused when the file is read, naming the tag and its source text (`(0018,0060) KVP = "1E16" is
outside the range a sealed JSON number can hold (±(2^53−1), I-JSON RFC 7493 §2.2)`), with no advice to write a string
because the person ingesting a file cannot. 64-bit integer tags (`SV`/`UV`) are written as strings by their type.

**Why on the value, not the token.** The rule could be read off the number's text, but the parser yields
a double, and rounding to the nearest double is monotone with 2^53−1 and 2^53 both representable: an
integer token is beyond the bound exactly when its double is. The value check is therefore the token rule
for every integer spelling (`9007199254740993.0`, `9.007199254740993e15`, `1e300`) and one implementation
serves readers and writers alike. It refuses one class the token rule would let through: a token with a
fraction that rounds to a double beyond the bound (from `9007199254740991.5`, which ties-to-even rounds to
2^53), whose fraction the double has already lost and which collides the same way. Both readers must
round each token correctly (`serde_json`'s `float_roundtrip`, Python's `float`) to agree at the bound. Fractions below the bound lose precision too
(`0.1000000000000000055511151231257827` and `0.1` share bytes); I-JSON makes that a SHOULD, not a MUST,
and it is not refused.

**Consequences.** Products sealed before this rule (≤ 0.1.0-alpha.5) that hold such a number are refused
(fail closed; their seal never pinned the value). No tracked corpus product holds one, so no golden hash moved.
Of the typed ingest sources, every vendor door that writes a JSON number validates its source values before anything is
written, in that format's words and through one formatter (`ingest::sealable`): DICOM's curated fields (the tag and the
element's text), and NIfTI's header, as one table of fields (the sform `srow_x/y/z`, the qform `quatern_b/c/d`,
`qoffset_x/y/z` and `pixdim[0..=3]`, the time step `pixdim[4]` and `toffset`, `scl_slope`/`scl_inter`), each named with its value
(`nifti: pixdim[1] = "10000000000000000" is outside the range a sealed JSON number can hold …`); an affine entry computed from
in-range fields that still falls out of range (`1e13` m is `1e16` mm) is refused naming the fields it came from. A NIfTI spacing
that is zero or negative is still clamped to 1 as the reference `nifti_quatern_to_mat44` does; a non-finite one is refused,
never clamped. The generic seal-time check can only name a JSON path, so for a typed field (a `column_meta.scale`, an affine, a
transform) its message is neutral ("outside the range a sealed JSON number can hold") and gives no advice to write a string: only a
free-form `metadata` field, which accepts one, gets it (`Error::InexactInteger::as_string`). A non-zero number that underflows to
`0.0` (`5e-400`) would seal a different number, so it is refused wherever a person or a file spells one (`--meta`, a JSON value, a
TOML float, a `--column-meta` `scale`, a DICOM `DS`); zero in any spelling and an exact denormal (`1e-320`) are kept. The DICOM header dump is text (`SV`/`UV` included), GE HDF5 attributes are not read into JSON and a vendor `.ini`
is carried as an opaque blob. The reference
reader refuses the same numbers, and `corpus/seal-cases.json` holds both readers to the same verdicts.

## Amendment (2026-11-19) — product schemas declare their 64-bit encodings and value ranges (#895)
Step 1 of [ADR-0066](0066-seal-encoding.md)'s implementation plan (rule 7: "64-bit values are decimal strings whose
type the schema declares"). The 2026-11-16 amendment made ±(2^53−1) the bound of every sealed JSON number, but the
product schemas did not state it: a `FieldSpec` could declare `"dtype": "uint64"`, the conflict surfaced only as a
by-value refusal at write time, and ingest code decided per source which numbers were strings.

**Decision (owner, accepted with ADR-0066 A′, 2026-11-18/19).**

1. **The bound is a format invariant** (SPEC §1), not a schema setting. A schema that relaxed it would reopen the #886
   collision, and a seal must not depend on product data. Schemas only state how their fields meet it.
2. **A schema is consistent with the bound.** A metadata `FieldSpec` whose `dtype` can hold an integer beyond it
   (`int64`, `uint64`) declares `"encoding": "string"`; its values are then the canonical decimal digits of an integer of
   that dtype in a JSON string (`"-12"`, `"1730000000000000000"`; no `+`, no leading zero, no `-0`), by type, never by
   magnitude. A bare 64-bit field, an encoding on any other dtype, and a dtype that is neither a `DType` name nor
   `string`/`coded`/`json` are refused (`Error::SchemaField`, naming the schema and the field). The DICOM door reads the
   spelling of each curated number from its field: a string-encoded field gets the element's integer as exact decimal
   digits (a 64-bit `SV`/`UV` read from its text, never through a double), every other field a JSON number.
3. **Optional inclusive ranges.** A numeric `FieldSpec` may declare `minimum` and/or `maximum`, within the bound and not
   empty. A value outside them (a number, each element of an array of numbers, or a string-encoded integer, compared
   exactly) is refused where it enters with `Error::FieldValue`, naming the field and the value's source in that door's
   words: the DICOM tag and its text (`dicom: (0018,0060) KVP = "5000": field 'kvp' of schema 'recon': 5000 is above its
   maximum 1000`), the ingest-spec key (`product[0].metadata.kvp`), the CLI flag (`--set kvp`, `--meta kvp`), or
   `metadata.kvp` at seal.

**Where it is checked.** `ProductSchema::check_definition` and `check_metadata` run in every `ProductBuilder::seal`, so
every writer (Rust, the CLI, Python's `Builder`, every ingest door, a streamed `WriteSession`) is held to its product's
schema, embedded or built-in. Sealing refuses an embedded schema this build cannot read (a newer rule, the array form;
ADR-0060 §2.2), since it would seal against a rule it ignores. `ProductSchema::validate` (and so `tessera schema` and the
ingest engine) runs both checks too. A reader's `verify` does not: the rules are a schema's contract, checked where a
product is made or validated, not part of the seal. The doors that know a value's source check it first, so the person
sees the tag, key or flag: the DICOM door per tag, `ingest::spec::validate` per `[product.metadata]` key, and the CLI's
`commit --set` and `ingest … --meta` per flag. The Python reference reader applies the same two checks
(`schema_verdict`), and `corpus/schema-cases.json` holds Rust and Python to the same verdicts.

**Built-in schemas.** `recon` 1.1 ranges `kvp` to 0–1000 kV, wide enough for every medical scanner, so only a physically
impossible value is refused. `slice_thickness` has a minimum of 0 mm and no maximum: DICOM sets no upper bound on
SliceThickness, and a CT localizer records its scan length there (GE Discovery MI scouts read 1000.6 and 1300.8 mm). `sim` 1.1 declares `seed` a string-encoded `int64`. No other
built-in field is 64-bit or ranged; `rescale_*` and `pixel_spacing` keep only the format bound.

**Consequences.** The new keys are absent unless set (`skip_serializing_if`), so a schema that declares none of them keeps
its bytes (ADR-0060's newer-field rule; a build that predates them refuses to validate or re-seal against them). The
`recon` and `sim` schemas changed, which moves the seal of every product that embeds them: the three `recon` corpus
products and the goldens derived from them are regenerated in the same PR, each listed with its cause. A product sealed
under the old `sim` schema can no longer be re-sealed or validated by this build (its schema declares a bare `int64`),
and a `seed` written as a number is refused by type. The ADR-0065 bridge's consumer drafts (#801) declare 14 bare 64-bit
fields; each needs `"encoding": "string"` before a product is sealed against it.

## Amendment (2026-11-20) — D4: the seal is over the stored canonical bytes (ADR-0066 A′, #914)
Step 4 of [ADR-0066](0066-seal-encoding.md)'s implementation plan, the format cutover. D4's canonical encoding is
unchanged; what it is applied to changes.

- **Stored form.** `manifest.json` (and `collection.json`) is stored as exactly its own JCS bytes, with no
  `manifest_hash` key. A reader refuses bytes that differ from their JCS form, after a strict parse that keeps the
  2026-11-16 I-JSON rule (the byte comparison alone would accept an integer beyond 2^53 that a double holds exactly).
- **Seal.** `manifest_hash = digest(<stored bytes>)`, kept beside them in a `manifest.seal` member (`collection.seal`
  for a collection). It is no longer a manifest field: D5's three hashes keep their meanings and their **values**
  (the stored bytes are the bytes `JCS(manifest − manifest_hash)` produced, proven on all 13 tracked products), but the
  seal now lives outside the document it seals. "Verify the parsed object" (#573) becomes "verify the stored bytes":
  there is no re-serialisation left between what is stored and what is hashed.
- **`id`** keeps `digest(JCS(id_inputs))` (ADR-0066 rule 6); the ingest `spec_hash` keeps `digest(JCS(spec))`
  (rule 9).
- **No migration** (rule 10): a product in the alpha layout (no `manifest.seal`) is refused with an error naming the
  layout found and the layout required. `TESSERA_VERSION` stays `"0.0.0"`.

The Verification section above is superseded by SPEC §7's order: strict parse → canonical check → no in-band seal →
seal over the bytes → typed layer (`Manifest::from_stored_members`, the one reader door).
