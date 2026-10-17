# ADR-0060 — Agentic actors and the W3C PROV mapping

**Status:** Proposed (2026-10-16, design for [#564](https://github.com/vig-os/tessera/issues/564); revised 2026-10-17 after a two-reviewer design audit).
Extends **ADR-0058** (sealed `producer` + `generation`) with a third sealed slot, `actors`. Bounded by
**ADR-0020** (canonical encoding and identity), **ADR-0036** (`id` is the lineage, `manifest_hash` the
version), **ADR-0037** (the signature binds `signer`), **ADR-0040** (data protection), **ADR-0042**
(sealed vs `aux/`), **ADR-0047** (crypto-shred) and **ADR-0048** (the provenance edge shape).
Depends on defects this design found or that its audit found:
[#573](https://github.com/vig-os/tessera/issues/573) (a reader misreports every newer additive field as
tampering), [#574](https://github.com/vig-os/tessera/issues/574) (`publish` and `commit` drop sealed
provenance), [#587](https://github.com/vig-os/tessera/issues/587) (the seal doesn't bind integers above
2^53), [#590](https://github.com/vig-os/tessera/issues/590) (`--require-signer` checks a label the signer
chooses) and [#591](https://github.com/vig-os/tessera/issues/591) (no verb erases one version of a
lineage). The adopter is MorePET/anatomist#86.

> Design only. **No sealed-layout code is written until the owner accepts this document.** Every claim
> about current behaviour below is cited to `file:line` at `dev` `fa581f5`, or to a probe whose command
> and output are given. The probes ran in a scratch crate outside the repository against
> `tessera-core` at that commit; they are not part of this change.

## Context

Agentic pipelines are now a producer category of their own: one coding agent implements a parser,
another reviews it, and a person approves the merge. anatomist is building exactly this and wants every
derived `.tsra` to say who did what. Today it can't do that honestly:

1. **`generation.config` is opaque by contract.** "The config keys are the generator's business and
   are never inspected by the engine" (`tessera-core/src/provenance.rs:180-182`, ADR-0058 §2). An agent
   identity in that bag can't be typed, validated, filtered or exported.
2. **`producer` is scalar** (`manifest.rs:45-46`): one `{tool, version, git_commit, git_repo, dirty}`.
   A chain of three actors has nowhere to go but one opaque slot.
3. **The signature carries exactly one `signer`** (`signing.rs:43-53`), which is attribution only
   (ADR-0037). The person who signs is usually not the agent that wrote the content, and nothing in
   the format can say that.
4. **The PROV promise is unkept.** `white-paper.md:1662` says the provenance DAG "maps naturally to
   PROV-O (`wasDerivedFrom`, `wasGeneratedBy`) in the RO-Crate export". `export.rs:78-109` emits only
   schema.org `isBasedOn`, and `export.rs:114-149` emits only DataCite `IsDerivedFrom`. Neither
   exporter emits the producer at all.

## §0 — The decision, in one place

| # | Question | Decision |
|---|---|---|
| 1 | Where agent identity lives | A new sealed, ordered `Manifest.actors` list. `producer` stays scalar. |
| 2 | Compatibility | The field is **corpus-neutral** (omitted when empty; no `TESSERA_VERSION` bump). But today's reader rejects *any* newer additive field as tampering, so **#573 (P0) ships first**, and P0 depends on #587's integer rule. |
| 3 | Identity | `actors` is in `manifest_hash` (the version). It is not in `id` (the lineage) or `content_hash`, and the seal refuses an `actors` key in `id_inputs`. |
| 4 | Sealed vs aux | Who, which software, which model, which settings, which brief → sealed. Wall-clock, URLs, tokens, cost → `aux/actors.json`. Host, login and IP aren't recorded at all, because `aux/` ships unencrypted in the same file. |
| 5 | Data model | `{id, role, kind, acted_on_behalf_of?, …kind fields}`. The kinds are `software_agent`, `person` and `organization`. Roles are an open token with a reserved vocabulary. `brief`/`receipt` are `blake3:` digests. Delegation points at another actor's local `id`. Order is significant and sealed. |
| 6 | Producer and signer | Three distinct things. The signature binds `actors` through `manifest_hash`. It attests who asserted the record, never that the record is true. A person or organisation actor counts as **attested** only when a trusted key whose **pinned** identity is that actor's identity signed the version; every other one is shown as *unattested*. Approving a **change** (the code) and approving a **product** are different acts with different records (§6.5). |
| 7 | Privacy | A person's identity is an IRI: an ORCID iD, a ROR id, or a random pseudonym `urn:tessera:actor:<26 base32>`. `mailto:` and email-shaped strings are refused in **every** string field. No verb can erase one version today (#591), so the guidance for now is pseudonyms for anyone who may withdraw consent. De-identifying ingests accept pseudonyms only, with no names. |
| 8 | Schema enforcement | Yes: `ProductSchema.required_actor_roles`, a **claim** check. It ships in the same PR as the signer-binding check (P3). Built-in schemas ship it empty. |
| 9 | PROV | RO-Crate: a schema.org `CreateAction` plus PROV-O qualified associations for roles and delegation, with collision-proof `@id`s. DataCite: people and organisations become contributors, software becomes `TechnicalInfo`. No native PROV-JSON. The exporters refuse invalid actors and newer fields. P2 lands after PR #541. |
| 10 | Surfaces | Rust `Actor` + `ProductBuilder::add_actor`. Python `add_software_agent`/`add_person`/`add_organization` + `tessera.digest`. `[[product.actors]]` in the ingest spec. `commit --add-actor/--keep-actors/--remove-actor`. An `actors` section in `inspect` with attested/unattested marks. |
| 11 | Phasing | #587 → P0 (#573) → #574 → P1a-P1e → P2a-P2b → P3 (claim check + signer binding, one PR) → P3d (`inspect` marks) → P3c (change approval) → P4 (deferred). Erasing a sealed identity from your own store needs #591. Each step stays under 400 lines. |

## §1 — Where agent identity lives

| Option | Verdict | Why |
|---|---|---|
| (a) `generation.config.agent` (status quo, anatomist's interim plan) | **Rejected** | The bag is opaque by contract (`provenance.rs:180-182`). Validating its keys would break ADR-0058's "keys never inspected" rule for one domain. Exporters can't map an untyped bag to PROV. It also mixes two things: *how* the product was made (settings that steer the run) and *who* acted (a record of the run). That second line is §5.6's rule. |
| (b) `producer` becomes a list | **Rejected** | `ProducerRef` is an untagged `Structured(Producer) \| Legacy(String)` (`provenance.rs:89-96`). `inspect --json` documents that polymorphism to consumers (`tessera-cli/src/main.rs:230-234`), so a list would be a third shape every consumer must handle. `seal` stamps `producer` when it is absent (`product.rs:199-201`), which only makes sense for one software build. A person isn't a "producer", and an LLM harness isn't the build that wrote the bytes. Two concepts would end up behind one name. |
| (c) A new sealed `actors[]` | **Chosen** | Typed, so it can be validated, filtered and exported. It's additive and omitted when empty (§2). It's covered by `manifest_hash` (§3), so the signature binds it with no envelope change (§6). It mirrors PROV's agent and association model directly (§9). It costs one more manifest field, and the struct shape is format (ADR-0058's "one unapologetic hardcode"). |
| (d) An unsealed `aux/actors.json` only | **Rejected** | ADR-0058's context already settled this: provenance outside the seal is "strippable and forgeable … worthless as a reproducibility or attribution claim". Who approved a product is exactly the claim a signature has to bind. `aux/` still gets the non-deterministic half (§4). |
| (e) A carried PROV document as a Blob block | **Rejected** | A Blob is sealed by digest, so it is tamper-evident. But it is opaque to the engine (ADR-0038: "not Interoperable until decoded"), so `inspect`, schema enforcement and the exporters would all need a PROV parser. It also fixes one serialisation (PROV-JSON? PROV-N? Turtle?) into the format. PROV belongs at the export edge (RFC §0.6: RO-Crate is a *derived* layer). |

## §2 — Back- and forward-compatibility, verified

### 2.1 What today's reader does with an unknown field

`Manifest` has neither `deny_unknown_fields` nor a catch-all map (`manifest.rs:26-94`). Unknown keys are
therefore **silently dropped** on parse. `canonical_bytes` re-serialises the *struct*
(`manifest.rs:146-150`) rather than hashing the parsed object, so the seal is recomputed without the
dropped key. SPEC §4 defines the seal over "the manifest object with **exactly one key deleted**
(`manifest_hash`); no other normalization" (`tessera/docs/SPEC.md:46-48`). The independent reference
reader hashes the parsed object exactly that way (`tessera/corpus/reference_reader/reader.py:145-147`).
`tessera-io/src/write.rs:44-51` records the absence of `deny_unknown_fields` on the manifest types as
deliberate: it is meant to preserve forward compatibility.

**Probe.** For each of the 8 corpus files, the probe takes `manifest.json`, splices in
`"actors": [{"role": "implementer", "kind": "software_agent", "harness": "probe"}]`, and calls
`Manifest::from_json_verified`. For one file it also runs the reference reader's `verify_tsra` on a
rebuilt `.tsra` (`uv run --with blake3 --with jcs==0.2.1`).

| Case | Rust reader (dev `fa581f5`) | Reference reader |
|---|---|---|
| A: corpus file unmodified | Ok; struct round-trip == golden, **8/8** | ok=True |
| B: `actors` added, resealed per SPEC §4 | **`Err(integrity: manifest_hash mismatch)`, 8/8** | **ok=True** |
| C: `actors` spliced in, old seal kept | **Ok, field silently dropped, 8/8** | ok=False |
| D: unknown key nested in `producer`, resealed | `Err(integrity)`, 8/8 | — |
| E: explicit `"actors": []`, resealed | `Err(integrity)`, 8/8; the seal differs from the golden | — |

Abridged output for `recon_int16`:

```text
recon_int16: A baseline verify=Ok struct_roundtrip==golden:true spec_seal==golden:true
  B actors + SPEC-correct seal -> Err(integrity: manifest_hash mismatch — expected blake3:2cf48e6b…, got blake3:4fd0b90d…)
  C actors spliced, old seal kept -> Ok (parsed; struct has no actors; seal blake3:4fd0b90d0aa)
B_resealed reference reader ok = True
C_spliced_old_seal reference reader ok = False
```

**Findings.**

1. The two conformant readers **disagree on the same bytes**. The Rust reader follows its struct, the
   reference reader follows SPEC §4. Only the reference reader matches the spec.
2. ADR-0058's "older manifests stay readable via `#[serde(default)]`" holds only **backwards** (a new
   reader on an old file). **Forwards**, every build released before ADR-0058 already reports a valid
   `generation`-bearing product as `Integrity` ("tampering, corruption, or a producer bug"), which is
   the misdiagnosis ADR-0058's amendment §2 exists to prevent. `inspect --help` promises "Fields are
   added compatibly (readers default them …)" (`main.rs:238-239`). That promise is half true.
3. RFC §13 already made forward reading normative: "v(N−1) warns + reads-what-it-can on vN". The code
   doesn't implement it.
4. Case D shows the same failure for a key nested in a sealed sub-struct. The same class also affects
   the embedded `ProductSchema` (`schema.rs:322-326`): an older reader *validates* against a schema
   rule it dropped.
5. Case E shows that an explicit empty array is a *different seal* from absence. SPEC §5 says the same
   ("`[]` and `{}` are **present** values", `SPEC.md:51-53`). So a writer that emits `"actors": []`
   seals bytes that a skip-if-empty struct can't reproduce.

### 2.2 Decision: P0 (#573) lands before `actors`

Adding a field is safe only once a reader can carry a field it doesn't know. P0 implements RFC §13 and
SPEC §4 as written:

- **Verify the object, not the struct.** `from_json_verified` parses `manifest.json` into a
  `serde_json::Value` and computes the seal over that object with only `manifest_hash` removed. That
  is SPEC §4 exactly, and the same computation `reader.py:145-147` performs. The typed `Manifest` is
  then deserialised from the same object as a **view**. One rule covers every nested struct
  (`Producer`, `Generation`, `Source`, `BlockRef`, `IngestTransform`, `Actor`) and `Collection`, with
  no per-struct catch-all and no serde `flatten`. The two readers agree by construction.
- **Name what the view didn't understand.** The reader walks the object against the **declared field
  names** of each type: a const list per struct, kept honest by a test that serialises a fully
  populated instance and compares its keys with the list. Any key that its type doesn't declare is a
  *newer field*. **Values never count.** A re-serialisation diff was considered and rejected in
  review: it misreports every known key the struct drops when writing back out (`"study": null`,
  a schema's `requires_generation: false`, `schema.rs:241`, a member's `"kind": "product"`) as being
  from a newer format. A known value the reader can't interpret, such as `ActorKind::Other` (§5.2),
  adds its own path explicitly. The dotted paths are recorded on the `Manifest` (`newer_fields()`, a
  non-serialised field). `verify`/`inspect` print `warning: fields from a newer format revision are
  not interpreted: actors`. `inspect --json` prints the parsed object rather than the view, so it is
  still exactly the manifest (`main.rs:225-234`'s promise).
- **Opaque maps are never walked.** Their keys are data, not format: `id_inputs`, `metadata`, `extra`,
  `generation.config`, `ingest_transform[].params`, `blocks[].spec`, `actors[].sampling` and
  `FieldSpec.default`. `Manifest.schema` is skipped by the manifest walk and checked by the schema
  walk instead (next bullet but one).
- **Refuse to re-seal what you don't understand.** Every path that builds a new seal from an existing
  manifest returns `Error::NewerFormatFields { fields }` when `newer_fields()` is non-empty. That covers
  `ProductBuilder::from_manifest`, so `commit`, `publish` and the ingest engine's re-seal at
  `tessera-ingest/src/engine.rs:1234`. **`from_manifest` therefore becomes fallible.** Its signature
  changes to `-> crate::Result<Self>`, which is a public API change. Its three production call sites
  (`version.rs:75`, `version.rs:377`, `engine.rs:1234`) and its tests propagate the error. A re-seal that carried unknown fields blindly could break a carry rule
  written in a newer ADR (§10.1 is one), and a re-seal that dropped them would be the silent loss of
  #462 and #574.
- **Refuse to validate what you don't understand.** `ProductSchema::from_value` reports unknown
  keys by the same declared-field-name walk, recursing into `fields[]` (`FieldSpec`, including
  `Coded`'s serde-renamed `_vocabulary`/`_code`) and `blocks[]` (`BlockRequirement`). `validate_manifest`
  then fails with
  `Error::NewerFormatFields`. A gate that can't evaluate a rule must not report "valid".
- **Empty collections.** Writers **MUST** omit an empty `actors` list, as `ingest_transform`, `metadata`
  and `extra` already are. Readers **MUST** accept `"actors": []` too. Verification is over the object,
  so §2.1 case E verifies, and the view reads it as "no actors". So P0 readers and P1 readers agree on
  every encoding. The struct's own `#[serde(default)]` re-emission of absent `sources`/`blocks` as `[]`
  stops mattering for verification, because verification no longer re-serialises the struct.
- **Numbers: no integer above 2^53 in a sealed value** ([#587](https://github.com/vig-os/tessera/issues/587)).
  JCS (RFC 8785 §3.2.2.3) serialises every number as an IEEE-754 double, so distinct integers above
  2^53 canonicalise to the same bytes. #587's probe edited a sealed `9007199254740993` to `…992` and
  `from_json_verified` still returned Ok with an unchanged seal. Hashing the object can't fix that,
  because the ambiguity sits in the canonical form itself. So the rule, which #587 lands format-wide
  and **P0 depends on**, is: an integer outside ±(2^53−1) anywhere in a sealed value (`metadata`,
  `extra`, `generation.config`, `actors[].sampling`, unknown fields included) is refused at seal and
  validate, with a typed error telling the caller to encode it as a string. It is also refused at
  **verify**, so a crafted file can't use the ambiguity, but with a **distinct** error,
  `Error::UnboundInteger { path }` ("the seal doesn't bind this value; it may have changed since
  sealing"), never `Error::Integrity`. A product sealed by an older writer that holds such a value
  is not tampered with. Its seal simply doesn't cover the value, and reporting it as tampering would
  repeat the misdiagnosis §2.1 fixes. The consequence is real: every already-sealed product holding
  such an integer stops verifying with that error. `inspect` and the readers still open it and name
  the path. #587 owns that migration note (posted there). Floats are unaffected: a double has exactly
  one JCS form. P0's object-level verifier calls the same check. **Rejected: a precomputed-digest test for
  values above 2^53** (this ADR's previous draft). It would pin the collision rather than remove it.
- **Out of scope:** a newer writer that changes the *type* of a known field (for example a new
  `producer` shape outside the untagged `ProducerRef`) fails to parse instead of warning. Changing
  a field's type is a major-version change under RFC §13, never an additive one.
- **SPEC.** §5 gains the rows it is missing today (`producer`, `generation`, `ingest_transform`), the
  `actors` row arrives in P1a, and §4/§7 gain the forward-reading rule.

**Rejected: a per-struct `#[serde(flatten)]` catch-all** (this ADR's first draft). It was reproduced
in review on serde 1. Next to a flattened, internally tagged enum, the catch-all also captures the
tag and the variant fields, so `kind` and `harness` are each serialised twice. That is invalid JCS
input and breaks the seal. It also needs a catch-all on every nested struct, and its interaction with
the untagged `ProducerRef` is fragile.
**Rejected: refuse every unknown field** (`deny_unknown_fields` on the format types). It is honest
(fail-loud, like #462's staging header), but it contradicts RFC §13 and makes every additive field a
hard break for readers that only want to verify and read. It also turns an archive reader from the year
it was built into a brick. `write.rs:44-51` already rejected it for the manifest types.
**Rejected: bump `TESSERA_VERSION` and gate on it.** `seal` stamps `TESSERA_VERSION` into every
default producer (`product.rs:199-201`, `provenance.rs:77-86`), so a bump moves **every**
`manifest_hash`: a full corpus regeneration, which ADR-0052's 2026-07-03 update decoupled precisely to
avoid. And `check_version` compares only the major version (`manifest.rs:207-226`). A version bump
signals nothing that field presence doesn't already signal. The format stays `0.0.0` (ADR-0052 §1).

**What P0 can't fix.** Builds released before P0 will still report an `actors`-bearing product as a
`manifest_hash` mismatch. The release notes of the release that carries P1 must say so. The owner
decides whether P0 ships at least one release ahead of P1 (Owner decision 1).

### 2.3 Corpus and goldens

- Case A shows that the struct round-trip and the object seal both reproduce all 8 golden seals. No
  corpus manifest carries a field the struct doesn't model, so `newer_fields()` is empty on the whole
  corpus. Switching verification to the object is corpus-neutral.
- `actors` is `#[serde(default, skip_serializing_if = "Vec::is_empty")]`. That is the same mechanism
  that kept `ingest_transform` byte-identical (`manifest.rs:74-86`), so **every existing product,
  corpus fixture and golden hash is unchanged**. No regeneration is needed.
- P1a **adds** one fixture (`actors_chain`) to `corpus.json` and `corpus/files/`. Existing entries are
  untouched. The reference reader passes it unchanged, because it already hashes the object.

## §3 — Identity

- **`manifest_hash` covers `actors`.** It is a manifest field, and the seal covers the entire manifest
  (ADR-0020 D5). Changing an actor makes a new version.
- **`id` does not.** `id = blake3(JCS(id_inputs))` (`manifest.rs:152-155`), and the default
  `id_inputs` are `product`/`name`/`timestamp` (`manifest.rs:108`). `actors` is never added to them.
  ADR-0036 makes `id` the lineage handle: an approver appended later is a new *version* of the same
  product, not a new product. **The same data with different actors has the same `id` and a different
  `manifest_hash`.** That is correct under ADR-0036.
- **`actors` is forbidden in `id_inputs`.** A caller-built `id_inputs` holding actor data would make
  a lineage that the next `commit` breaks: `from_manifest` rebuilds the manifest through
  `Manifest::new` (`product.rs:39-44`), which resets `id_inputs` to the defaults (`manifest.rs:108`),
  so a `commit` of such a product would mint a different `id`. `seal()` therefore refuses an
  `id_inputs` key named `actors` with `Error::InvalidActor` (P1a, `actors_in_id_inputs_is_refused`).
  It can't recognise actor *values* under another key, so the book states the rule.
- **`content_hash` does not.** It is the Merkle root over block digests only (`manifest.rs:159-168`).
- *Correction (not an edit to ADR-0058):* ADR-0058's Consequences say "you cannot alter the recorded
  config/producer without changing the product id. Different config ⇒ different id". That is true of
  `manifest_hash`, not of `id`. Probe: re-sealing a product whose producer changed from
  `anatomist/0.3.0` to `tessera/0.0.0` printed `same id: true` (see #574's repro).
- **Writer determinism (S15).** Every actor field is caller-supplied. The engine adds no clock, no host
  and no random value, and §4 lists no wall-clock field. Given the same input, the same actor list
  produces the same bytes. The engine never sorts the list (§5.4), so order is the caller's
  deterministic input. The `sampling` bag is opaque, so keeping wall-clock values out of it is the
  writer's documented responsibility, exactly as for `generation.config`.

## §4 — Sealed vs `aux/`, field by field

The rule is ADR-0058's: **deterministic, identity-bearing → sealed; non-deterministic → `aux/`** (ADR-0042).

| Field | Where | Reason |
|---|---|---|
| `id` (local handle) | sealed | the target of `acted_on_behalf_of` and the join key for `aux/actors.json` |
| `role` | sealed | the claim being made ("X reviewed") |
| `kind` | sealed | selects the meaning of the remaining fields |
| `harness`, `harness_version` | sealed | which agent runtime acted; deterministic given the run |
| `model`, `provider` | sealed | which model, self-reported (§6.3) |
| `sampling.*` | sealed | settings like effort and temperature are part of how the result arose |
| `brief`, `receipt` | sealed | digests bind the exact instructions and trace without carrying them |
| `acted_on_behalf_of` | sealed | the accountability edge, and the one a signature most needs to bind |
| `identity`, `name` (person, organization) | sealed | attribution; see §7 for which forms are allowed |
| start/end wall-clock per actor | `aux/actors.json` | non-deterministic; the seal would change on every re-run |
| PR / issue / session URLs | `aux/actors.json` | change on rename, merge or hosting move; not identity |
| token counts, cost, latency | `aux/actors.json` | accounting, non-deterministic and run-noisy |
| host, user login, IP | **not recorded** | personal data, and `aux/` is no place to hide it (below) |

**`aux/` is not confidential.** Aux members ride unencrypted inside the same shareable `.tsra`
(ADR-0042). They are outside the seal, not outside the file. Whoever receives the product receives
`aux/actors.json` too, unless an ADR-0055 whole-file encryption wraps the whole thing. So `aux/` is
for data that is non-deterministic, never for data that is sensitive. The previous draft's "host,
login, IP → aux as personal data" would have shipped personal data in the clear, so this design
keeps those fields out of the file altogether. A pipeline that needs them for its own audit keeps them
in its own logs.

`aux/actors.json` is an object keyed by the sealed actor `id`, with an open bag per actor:
`{"<id>": {"started_at", "finished_at", "url", "tokens", "cost", …}}`. Unknown keys are kept, because
the file is outside the seal. The P1d writer refuses the keys `host`, `user`, `login` and `ip`, and any
email-shaped string value (§7.3). URLs can still name an account (a PR URL names its repository, not
its author), and the book says so. Adding or removing the file leaves both hashes byte-identical
(ADR-0042's invariant, enforced by `add_aux_members`). A key that names no sealed actor is a warning in
`inspect`, not an error, because aux is unauthenticated by design. A de-identifying ingest refuses
`--actors-aux` (§7.4).

## §5 — The data model

### 5.1 JSON shape (normative)

```json
"actors": [
  { "id": "impl", "role": "implementer", "kind": "software_agent",
    "harness": "agent-harness", "harness_version": "2.1.0",
    "provider": "example-lab", "model": "model-x-1",
    "sampling": { "effort": "high", "temperature": 0 },
    "brief": "blake3:9a1f…", "receipt": "blake3:47c2…",
    "acted_on_behalf_of": "owner" },
  { "id": "rev", "role": "reviewer", "kind": "software_agent",
    "harness": "other-harness", "model": "model-y-2", "acted_on_behalf_of": "owner" },
  { "id": "owner", "role": "approver", "kind": "person",
    "identity": "https://orcid.org/0000-0002-1825-0097" },
  { "id": "lab", "role": "owner", "kind": "organization",
    "identity": "https://ror.org/0example00", "name": "Example Lab" }
]
```

| Kind | Required | Optional |
|---|---|---|
| all kinds | `id`, `role`, `kind` | `acted_on_behalf_of` |
| `software_agent` | `harness` | `harness_version`, `provider`, `model`, `sampling` (object), `brief`, `receipt` |
| `person` | `identity` | `name`, `identity_commitment` (P4, §7.5) |
| `organization` | `identity` | `name`, `identity_commitment` (P4, §7.5) |

Optional fields are omitted when absent, never written as `null`, and `sampling` is omitted when
empty. That keeps one canonical encoding per value, as §2.2 requires.

### 5.2 Kinds — a closed `#[non_exhaustive]` set, unknown kinds kept opaque

PROV-O has exactly three agent subclasses (`prov:SoftwareAgent`, `prov:Person`, `prov:Organization`,
<https://www.w3.org/TR/prov-o/>), and schema.org's `agent` range is `Person | Organization`. That set
has been stable since 2013, so `kind` is a **closed** enum. It is `#[non_exhaustive]` so downstream
matchers can't lock the variant set, which is ADR-0049's precedent. An **unknown** kind value doesn't
fail the parse. `kind` is deserialised from its string with a fallback variant, `ActorKind::Other(String)`,
so the actor is kept, hashed (verification is over the object, §2.2), and shown as `unknown actor kind
'x' (newer format)`. `ActorKind::Other` always adds `actors[i].kind` to `newer_fields()`, even when the entry uses only
known field names, so every re-seal (any `commit`, `publish`, the ingest engine's) refuses it: this is
P0's rule applied to a value rather than a key.
**Rejected: an open string kind.** Every consumer would have to guess which fields apply.
**Rejected: refusing an unknown kind** (ADR-0049 does this for collection *member* kinds). A
collection reader can't traverse a member it doesn't understand, but an actor is a record that can be
carried without being understood.

### 5.3 Roles — an open token plus a reserved vocabulary

`role` matches `^[a-z][a-z0-9_]*$`, so it can be filtered across a corpus without case or spelling
drift. Any value is allowed. These names are **reserved**, have documented meanings, and are exported
as `tessera_core::actor::role::*` constants:

| Role | Meaning |
|---|---|
| `generator` | directly produced the content of this product |
| `implementer` | wrote the code or configuration that produced the content |
| `reviewer` | examined the content or its producing change and reported on it |
| `approver` | accepted it; the accountable sign-off |
| `owner` | the accountable principal on whose behalf the work was done |
| `operator` | ran the pipeline or instrument without authoring it |

One actor with two roles appears twice, with two local ids. One entry per (actor, role) keeps role
filtering a simple equality and mirrors PROV's one `hadRole` per qualified association.
**Rejected: a closed role enum** (#564 itself keeps the field open; a new role is additive).
**Rejected: `roles: [..]` per actor.** It is a second way to say the same thing.

### 5.4 Ordering, identity of entries, references

- **Order is significant and sealed.** By convention it is the chronological order of the acts. The
  engine never sorts: sorting would erase chain order, and a sorted-set meaning would need its own
  canonical rule. Reordering is a new version.
- **`id`** matches `^[a-z0-9][a-z0-9_-]{0,63}$` and is unique within the list.
- **`acted_on_behalf_of` is another actor's local `id`.** It must resolve, must not point at the
  actor itself, and the delegation graph must be acyclic. **Rejected: a free identity string** (#564's
  sketch). It can't be checked, it can dangle, and the same person would appear in two encodings
  (inline as a string and as an actor). If the principal matters, it is an actor.
- **Multi-agent chains** are simply several entries. The delegation edges carry the structure, and a
  sub-agent that delegated to another agent points at that agent. Collections don't carry actors in
  this ADR (§12).

### 5.5 `brief` and `receipt`

- **Format:** `blake3:<64 lowercase hex>`, which is the format's own digest convention
  (`tessera-core/src/hash.rs:15`). These are named `brief`/`receipt`, not `*_sha`: the value isn't a
  SHA, and labelling a BLAKE3 value as a SHA is the defect PR #541 is fixing in the exporter.
- **Over which bytes:** the exact bytes of the referent as the agent received them (`brief`: the
  instructions or system prompt) or emitted them (`receipt`: its trace or report). No normalisation is
  applied. `digest(bytes)` is the same function a Blob block's digest uses (ADR-0038).
- **Where the referent lives:** anywhere. The digest is enough to check a copy found later. When the
  referent is carried in this `.tsra` as a Blob block, its block digest equals the field, and
  `inspect --resolve-config`'s mechanism (ADR-0058 amendment) resolves it. That flag generalises to
  `--resolve`, so a missing referent is reported loudly only when resolution was asked for.
- **The circularity limit:** a receipt that names *this* product's `manifest_hash` can't be sealed
  inside the product. Such a receipt is its own product with a `derived_from` edge back (#564's
  pitfall), and this product carries no digest of it.

### 5.6 Provenance, not configuration

`actors` **records** who acted. It never **steers** anything: no engine or ingest behaviour branches
on it, apart from the structural validation in §10.1 and the schema rule in §8. A value that changes
what the run *does* (which model to call for step N, a temperature that the pipeline reads) is
configuration and belongs in `generation.config`. When the same value is both (the temperature the
agent ran with), it may appear in both places. They answer different questions.

### 5.7 Rust sketch

A **flat** struct with post-parse typing. Every kind-specific field is optional in the type, and
`validate_actors` (§10.1) enforces §5.1's per-kind requirements. Choosing flat over a `flatten`ed
internally tagged enum is deliberate (§2.2's rejected alternative, reproduced in review), and an
unknown kind needs a fallback that a derived tagged enum can't express (`#[serde(other)]` only
works on unit variants).

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Actor {
    pub id: String,
    pub role: String,
    pub kind: ActorKind,                  // serde(from = "String", into = "String")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acted_on_behalf_of: Option<String>,
    // software_agent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sampling: BTreeMap<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brief: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt: Option<String>,
    // person / organization
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[non_exhaustive]
pub enum ActorKind { SoftwareAgent, Person, Organization, Other(String) } // "software_agent", …
```

The fields are public, as `Producer`'s are (`provenance.rs:46-60`), and the constructors in §10.1 are
the supported way to build a valid actor. P1a pins the JSON byte form of §5.1 with a golden.

## §6 — `producer`, `actors` and the signature `signer`

### 6.1 Three distinct things

| Slot | Answers | Cardinality | Who fills it |
|---|---|---|---|
| `producer` (ADR-0058 §1) | which **software build** wrote the content | one | `seal` stamps tessera; an external tool overrides it |
| `actors` (this ADR) | which **agents, people and organisations** acted, in which role | zero or more | the caller |
| `signer` (ADR-0037) | which **identity label** a key holder attached to a signature | one per signature, unsealed sidecar | the signer |

A software agent is never the `producer`: the harness that drafted a parser isn't the parser build
that sealed the bytes. In the anatomist chain, each derived product has `producer = anatomist/0.3.0`
at the approved revision, `actors = [operator]`, and the host's signature. The implementer agent, the
reviewer agent and the approver person sit on the **change record** for that revision, which the approver
signs (§6.5).

### 6.2 What a signature attests, and what it doesn't

The signed envelope is `{alg, key_id, manifest_hash, signer, signed_at, key_format}`
(`signing.rs:43-53`). `manifest_hash` covers `actors` (§3), so **the signature already binds the full
actor list, with no envelope change**. A valid signature that verifies against a trusted key means
exactly this: *the holder of key K asserted, under the attribution label `signer`, at `signed_at`,
this exact manifest, including its actor list.*

It does **not** mean that the listed model ran, that the reviewer reviewed, that the brief was the one
used, or that `signer` is the approver. Those are claims inside the record that the signer vouches for.
They are not facts the cryptography establishes.

### 6.3 Claims, not attestations

`harness`, `model`, `provider` and `sampling` are **self-reported** by the harness, and then by
whoever wrote the manifest. No current mechanism lets a verifier check them. Model providers don't
sign their outputs, and a model id string is free text. A verifier **can** conclude integrity
(nothing changed since sealing), signer accountability (via the trust store, ADR-0037), and digest
binding (a brief or receipt found later is, or isn't, the one named). It **cannot** conclude which
model executed. `inspect` and the book say this in those words. P3b adds the one check that *is*
mechanical: whether the key that signed is pinned to the listed actor's identity (§6.4).

### 6.4 Binding an actor to a key: the pinned identity, never the label

The envelope's `signer` is chosen by the key holder. The signature binds it, so it can't be changed
after signing, but whoever signs can write any identity there. Today's `verify-sig --require-signer`
compares exactly that label (`tessera-cli/src/trust.rs:322-328`), so any trusted key can sign as
`signer = <the approver's ORCID>` and pass. That defect is filed as
[#590](https://github.com/vig-os/tessera/issues/590). The only identity a *verifier* controls is the
one the operator **pinned** when trusting the key: `tessera trust add --signer` writes `<name>.signer`
(`trust.rs:176`), which `entries()` loads into `TrustEntry.signer` (`trust.rs:156`, `trust.rs:207`).

So the rule for every actor check in this ADR is:

- **An actor is attested by a signature** only when (1) the signature verifies against a key found in
  the trust store (not `--pubkey`, which carries no pin, `trust.rs:283-287`), (2) that entry has a
  pinned identity, and (3) the pinned identity equals the actor's `identity`. If the envelope's
  `signer` differs from the pin, the check refuses, because the label is wrong.
- **P3b, `verify-sig --require-signer-role <role>`,** passes only if some actor with that role is
  attested by the verifying signature under that rule. It refuses, with a typed error naming which
  condition failed, when the key isn't in the trust store, has nothing pinned, or is pinned to an
  identity no actor with the role holds. P3b fixes #590 in the same PR, because both checks need
  the same pinned-identity lookup.
- **`inspect` marks every `person`/`organization` actor** as `attested (key <name>)` or
  `unattested`. It reads the embedded `aux/signatures/<key_id>.sig.json` or the detached
  `<file>.tsra.sig.json` (`tessera-io/src/container.rs:31-32`) and the trust store, and applies the rule
  above. An actor named in `acted_on_behalf_of` is a person or organisation actor too, so naming a
  real ORCID iD as a principal without that person's signature shows up as `unattested`. The writer
  made a claim *about* that person; the person didn't make it. Software agents are never marked: §6.3
  already makes their fields claims, and they carry no `identity` to pin. An accountable party that
  holds the key for an automated run, such as §6.5's operator, is an `organization`.

### 6.5 Approving a change is not approving a product

The adopter's real flow (MorePET/anatomist#86, its dogfooding comment of 2026-10-16 and the
owner's comments of 2026-10-17) has two acts that happen at different times, on different machines:

1. **The change.** An agent implements a step, another agent reviews it, and a person approves and
   merges the pull request, on their own machine. What they approve is a **code revision**.
2. **The products.** Later, an automated job on the compute host (`$KSB_JOB_OUT`, decision 2 of #86)
   runs that revision over data and writes one derived `.tsra` per step. The approver never sees
   these products, and their key isn't on that host.

The previous draft's §8 advice (`required_actor_roles = ["approver"]` on derived products) collapsed
the two. It would stamp the pull request's approver onto outputs they never saw. And either the
human's key would have to sit on the compute host, or `signer != approver` and P3b would fail by
construction. So the two acts get two records:

- **The change record.** A small product of the adopter's own type (for example `change_record`)
  whose sealed `metadata` names the subject revision. Its `actors` are the implementer agent, the
  reviewer agent and the approver person. Its blocks can carry the briefs and receipts as Blob blocks
  (§5.5). The approver seals and signs it once, with their own key, on their own machine. Under §6.4
  it is attested by that key's pinned identity. The adopter's schema puts
  `required_actor_roles = ["implementer", "approver"]` on this type.
- **The product.** Its `producer.git_commit` is the revision, sealed and `dirty = false`
  (`provenance.rs:46-60`). Its `actors` name only who acted in *this* run: an `operator`, which is
  the **organisation** that runs the pipeline (`kind: "organization"`, with a ROR id or a §7.2
  pseudonym such as one for a lab's compute service), and a `generator` (a `software_agent`) if a
  model produced content directly. The host signs the product with the **host's** key, which the
  verifier's store pins to the operator organisation's identity. Several host keys may pin the same
  identity, one `trust add --signer` each. A product may add a source edge
  `{role: "approved_change", id: <change record id>, content_hash: <its manifest_hash>}`, so it
  is self-describing. No approver actor appears on it.
- **P3c: `tessera verify-change <product> --change <record.tsra> --role approver`.** It passes when
  (a) the record's `approver` is attested under §6.4, (b) the record's sealed subject revision equals
  the product's sealed `producer.git_commit` and `git_repo`, (c) the product's `producer.dirty` is
  `false`, and (d) the product's `operator` is attested under §6.4 by the product's own signature.
  Without (d), anyone could seal a product that claims the approved commit. If the product carries an
  `approved_change` edge, it must also pin this record. The verifier learns "the code that made this
  product was approved by a person I trust, and an operator I trust ran it" without any person being
  named on the product, and without moving a key.
- **Why the operator is an organisation, not a software agent.** The previous revision allowed "a
  `software_agent` … with a pseudonymous or ROR identity", but `identity` exists only on `person` and
  `organization` (§5.1), and §6.4 attests only those kinds. A software agent has nothing to pin. The
  fix keeps the model as it is rather than giving software agents an identity. A signing key is held
  by an **accountable party**, never by a model or a harness. The software that ran is already
  sealed as `producer`, and the agent harness fields are claims (§6.3). An `identity` on a software
  agent would invite the reading "this key attests the model ran", which no mechanism supports (§6.3). An
  `operator` given as a `software_agent` is still valid to seal, because §8 keeps role→kind policy out
  of the engine. P3c, however, refuses it with a typed error naming the kind
  (`operator_must_be_attestable`). For anatomist: `operator` = the lab, as an organisation (ROR, or a
  pseudonym for its compute service), and the host key on `$KSB_JOB_OUT`'s machine is pinned to it.
- **Approving a product** is still possible and keeps its plain meaning. A person who reviewed
  *this* output (say, a segmentation) adds themselves as `approver` with `commit --add-actor` and signs
  that version with their own key. `required_actor_roles = ["approver"]` belongs on product types that
  really need a human to look at each one.

The subject's metadata keys (`subject_git_repo`, `subject_git_commit`) are the one convention P3c
reads. They are named in the book's change-record section, and P3c refuses a record without them.
The engine reads no other change-record content, and the type name is the adopter's.

## §7 — Privacy

A person's `identity` sealed into an immutable, content-addressed product **can't be redacted without
a new version**. No verb today can remove that old version from a store (§7.5), and a copy that has
already been shared can't be recalled at all. Everything below follows from that.

### 7.1 Which tier

ADR-0040's tiers classify fields *about the data subject* (a patient's `patient_id` is `identifying`
and "never in the clear in a shareable artifact"). An actor is a **contributor**, and naming one is
often the point (attribution, as with DataCite creators, RFC §13's
`creators: [{name, affiliation, orcid, role}]`, and ADR-0037's ORCID `signer`). So actor identities
don't use the subject tiers. They are still **personal data** under GDPR Art. 4(1), and in a
de-identified product they belong to a class PS3.15 strips (§7.4). The previous draft said they "are
not PHI", which was too broad.

### 7.2 Allowed identity forms (engine-validated in P1a)

`identity` must be an absolute IRI (`scheme:rest`), and one of:

- **An ORCID iD** (`https://orcid.org/` + the 16-character iD, checksum verified) for people, or a
  **ROR id** (`https://ror.org/` + 9 characters) for organisations. Both are persistent and public by
  the subject's own choice. Use these **only** for contributors who agreed to permanent, public
  attribution, as an author of a paper does (§7.5).
- **An institutional `https:` URI**, under the same consent condition.
- **A pseudonym** `urn:tessera:actor:<26 chars>`, matching `^urn:tessera:actor:[a-z2-7]{26}$`. That is
  130 bits in lowercase RFC 4648 base32. **Randomness rule:** a pseudonym MUST come from a CSPRNG and
  MUST NOT be derived from any identifier. A hash of an email or an ORCID iD can be confirmed by
  guessing, which is the reviewer's finding and the same weakness §7.6 names for low-entropy
  digests. The engine checks the shape only, since randomness can't be tested from one value. So the
  supported way to mint one is `tessera actor pseudonym` (P1e), from `getrandom` as `keygen` does.
  **Linkability is the custodian's choice:** the same pseudonym on two products links them ("the
  same reviewer"). The recommended scope is one pseudonym per person per project; one per product
  gives unlinkability. The mapping from pseudonym to person stays with the custodian, never in the
  product. That is the anatomist owner's 2026-10-17 rule for the clinic mapping: host-only and
  encrypted.
- **Refused:** `mailto:` IRIs and anything without a scheme (bare emails, `user@host`). Emails go
  stale, are linkable across contexts, and are the identifier most likely to be harvested. That makes
  it a red line like ADR-0040's raw paths (Owner decision 3).

### 7.3 The email ban covers every string, not just `identity`

`validate_actors` refuses, in **every** string of an actor (`name`, `harness`, `harness_version`,
`provider`, `model`, and every string at any depth inside `sampling`), a case-insensitive `mailto:` and
any substring matching the **email pattern**:

```text
[A-Za-z0-9._%+-]+@[A-Za-z][A-Za-z0-9-]*(\.[A-Za-z][A-Za-z0-9-]*)*\.[A-Za-z]{2,}
```

That is: a local part, `@`, then a domain of two or more dot-separated labels, **each starting with a
letter**, the last being two or more letters. A `name` may contain no `@` at all. `id`, `role`,
`brief` and `receipt` can't contain `@` by their own regexes (§5.3-§5.5).

**What it refuses, exactly, and why semver pins pass.** A semver version is `MAJOR.MINOR.PATCH` with
numeric fields (semver.org §2), optionally prefixed with `v`. So after `pkg@` comes a digit-led label
(`1`), or a `v1` label followed by the digit-led label `0`. Either way the domain part can't match,
whatever pre-release (`-alpha.beta`) or build metadata (`+exp.sha.5114f85`) follows. The previous
revision's pattern allowed digit-led labels and wrongly claimed to pass semver pins: it refused
`pkg@1.0.0-alpha.beta`, `pkg@1.0.0-beta.rc` and `pkg@v1.0.0-beta.rc`. The probe for this revision ran
both patterns over the same strings (`python3 -I probe.py`, the output is in PR #586):

| String | previous pattern | this pattern |
|---|---|---|
| `pkg@1.2.3`, `tool@latest`, `pkg@1.0.0-alpha` | passes | passes |
| `pkg@1.0.0-alpha.beta`, `pkg@1.0.0-beta.rc`, `pkg@v1.0.0-beta.rc` | **refused** | passes |
| `pkg@1.0.0-rc.1+build.5`, `pkg@1.0.0+exp.sha.5114f85`, `npm:@scope/pkg@2.1.0-next.3`, `pkg@^1.2.0-beta.x`, `pkg@~1.2.3-rc.a` | passes | passes |
| `jane.doe@example.org`, `x@mail.example.co.uk`, `MAILBOX@Uni-Example.DE` | refused | refused |
| `user@123mail.com`, `x@mail.163.com` | refused | **passes** |

The cost is the last row. An email whose domain has a **digit-led label** isn't caught, which is a
known false negative and is documented in the book. It is the price of never refusing a semver pin.
Also refused, by design: a non-semver tag that looks like a domain, such as `tool@next.beta`. The
book tells writers to use `tool@next` or a semver pin instead.

Login names (`user@host`, no dot) in software fields are **not** refused. They can't be told apart
from dist-tag pins such as `tool@latest`. The book says not to put logins there. This partly
disputes the round-1 audit's "apply the login ban to every free-text field": the ban applies to `name`
and `identity`, where nothing legitimate needs `@`.

### 7.4 De-identifying ingests

`--deidentify` and crypto-shred (`--recipient`, ADR-0047) set `PatientIdentityRemoved = YES`
(`tessera-ingest/src/dicom.rs:684-693`), after stripping `InstitutionName` and
`ReferringPhysicianName` among the PHI tags (`dicom.rs:659-660`). A ROR id for the clinic, or an
operator's ORCID iD, sealed beside that marker would put back exactly the class of identifier PS3.15
removes, while the product claims it's gone. So:

- In both modes, the ingest **refuses** any `person`/`organization` actor whose `identity` isn't a
  §7.2 pseudonym, any `name`, and `--actors-aux`. Software agents are allowed, and §7.3 applies to
  them as everywhere.
- **The rule persists past the ingest.** The scrubbed header lands in the sealed `extra.dicom_header`
  (`dicom.rs:799-802`), so the element `(0012,0062) = YES` is a sealed marker. `commit --add-actor`,
  `commit --keep-actors`, the ingest engine's re-seal and the CLI's `tessera validate` apply the same
  refusal whenever that marker is present. The check is one function in `tessera-ingest`, which
  owns `PHI_TAGS`; the CLI calls it, and `tessera-core` stays domain-agnostic. A library writer that
  builds a de-identified product without the CLI is caught by `tessera validate`, not at seal.
- Under crypto-shred, the real identities of pseudonymous actors may ride in the identity envelope
  (§7.5, P4). They are never sealed.

### 7.5 Erasure: what works today, and what needs a mechanism

The previous draft's recipe ("`commit` without the person, then `forget` + `gc`") **doesn't work**,
against the code as it stands:

- `commit` pins the old tip in a `supersedes` edge *with* its `content_hash`
  (`tessera-cli/src/version.rs:124`).
- `gc` treats every source `content_hash` that names an in-repo object as reachable
  (`tessera-io/src/repo.rs:389-394`), so the old version naming the person stays reachable from the
  new tip and is never collected. §10.1's carry rule relies on that same reachability.
- `forget` deletes the whole lineage's ref and log (`repo.rs:296-308`). A later `gc` then drops
  **every** version, not just the one naming the person.

So **today**, a store you control can only forget a sealed identity by dropping the whole lineage.
Erasing one version needs a **redacting commit**: ADR-0040 §2.1's `tessera redact`, which no one has
built. It is filed as [#591](https://github.com/vig-os/tessera/issues/591) (a tombstone edge that keeps
the `manifest_hash` but drops the `content_hash` pin `gc` follows). Copies already shared are out of
reach of any repository verb.

**What is safe today** (the book's privacy section says this before anyone seals a person):

1. Seal an ORCID iD, ROR id or `name` only for a contributor who agreed to **permanent**, public
   attribution.
2. Everyone else, and anyone who might withdraw consent, is a §7.2 **pseudonym**. Withdrawal then
   means deleting the custodian's mapping entry. The pseudonym is random, so that erases the link in
   every copy, including copies already shared, with no new version and no repository verb. It works
   now, with neither #591 nor P4.
3. A de-identified product carries pseudonyms only (§7.4).

**Legal basis.** Recording a person needs a lawful basis (GDPR Art. 6, typically consent or the
research exemption in Art. 89). Art. 17(3)(d) limits erasure for archiving and research, but this ADR
doesn't treat that as automatic, and it isn't legal advice.

**The in-file mapping (P4, deferred).** For an adopter who wants the mapping to travel with the
product but stay erasable: the real `{identity, name}` rides in an `age` envelope, which is ADR-0047's
pattern. Under a crypto-shred DICOM ingest it goes in the same `aux/identity/identity.age` under a new
`actors` key, so one `shred` covers both. Otherwise it goes in `aux/identity/actors.age`.

- **Binding.** The person's sealed actor gains
  `identity_commitment = blake3(JCS({"identity", "name", "salt"}))`. `salt` is 32 random bytes that
  exist **only inside the envelope**. A key holder decrypts the envelope, recomputes the commitment,
  and compares, so a swapped envelope is detected. ADR-0047 §3 leaves its own envelope unbound; this
  closes that gap for actors. Without the salt, the commitment of a public ORCID iD could be confirmed
  by guessing. After a shred the salt is gone and the commitment reveals nothing.
- **Determinism.** The salt and commitment are minted once by the custodian and supplied by the
  caller, as the pseudonym is, so `seal` adds no randomness and S15 holds.
- **Format.** `identity_commitment` is a new optional field, added additively under P0's rule. The
  previous draft claimed "the sealed shape is already the pseudonym form", which was wrong without
  this field, and the claim is withdrawn.

The plain `aux/` members are unencrypted (§4). Only the `age` envelope is confidential.

### 7.6 Software agents

Software agents carry no personal data by construction, and §7.3 keeps it that way for their
strings. Their `brief`/`receipt` *referents* might, and a digest of a low-entropy referent can be
confirmed by guessing. Treat referents with the data's own policy.

## §8 — Schema-driven enforcement: a claim check, shipped with the signer binding (P3)

`ProductSchema` gains `required_actor_roles: Vec<String>`, with
`#[serde(default, skip_serializing_if = "Vec::is_empty")]`. `validate()` then refuses a manifest that
has no actor for a listed role, with `Error::Invalid("schema 'X' requires an actor claiming role
'approver' — ADR-0060 §8")`. That is the same mechanism and placement as `requires_generation`
(`schema.rs:242`, `schema.rs:277-282`). **The built-in schemas ship it empty**, so nothing is
blocked by default and the embedded schema bytes are unchanged, which keeps the corpus neutral.

**It is a claim check, and its documentation, error text and name say so.** It proves the record
*claims* an actor in that role, nothing more. Anyone sealing a version can `--add-actor` an approver.
The carry rule (§10.1) stops only *accidental* inheritance of an approval, not a deliberate false
claim. So:

- **P3a never ships alone.** It lands in one PR with P3b (§6.4's signer binding, which fixes #590),
  as phase P3. A `validate` that passes therefore always has a command next to it that can check the
  claim.
- The book pairs them: `validate` answers "is the claim present?", and
  `verify-sig --require-signer-role approver` (or P3c's `verify-change`, §6.5) answers "did the
  claimed person sign?".
- `inspect` marks the claim `unattested` until a signature binds it (§6.4).

Domain schemas opt in. In anatomist's case, `required_actor_roles = ["implementer", "approver"]` goes on
its **change record** type, and `["operator"]` on its derived products (§6.5), **not** `["approver"]` on
derived products. `["approver"]` is for product types where a person really reviews each product.

- Why yes: ADR-0058 §3 and ADR-0050 already established that "must carry X" is schema data, and the
  engine needs no policy to read a list. anatomist#86 decision 4 uses that same pattern
  (`requires_generation` for derived steps), and the owner's comment of 2026-10-16 lists "approver"
  among the agentic fields to record. Without the check, "every merged step had an approver" can only
  be verified by convention.
- **Rejected: `requires_actors: bool`.** Too coarse; the real requirement is always a role.
- **Rejected: requiring kind or identity form per role** (for example "the approver must be a person
  with an ORCID iD"). That is real policy, but it is domain policy with no second user yet. It can be
  added later the same way. (§12.)
- **Rejected: making `validate` itself check signatures.** `validate` is a pure function of the
  manifest, while trust is the verifier's local store (ADR-0037). Mixing them would make validation
  depend on the machine it runs on.
- An older reader that doesn't know `required_actor_roles` refuses to validate under P0 rather than
  passing silently (§2.2). That is the only safe outcome for a gate.

## §9 — The PROV mapping

### 9.1 What the current idioms are (researched 2026-10-16)

- **RO-Crate** models provenance with **schema.org actions, not raw `prov:` terms.** The provenance
  pages ([1.1](https://www.researchobject.org/ro-crate/specification/1.1/provenance),
  [1.2](https://www.researchobject.org/ro-crate/specification/1.2/provenance)) say: "The software
  SHOULD be associated with the File(s) (or other data entities) it created as an instrument of a
  CreateAction", represented as a `SoftwareApplication` "with a version property". They also say "An
  Action SHOULD have a human agent who was responsible for authorizing the action", and they don't
  use the PROV vocabulary. The current release is 1.3. The 1.2 page shows the same examples.
- **Process Run Crate 0.6** (<https://w3id.org/ro/wfrun/process/0.6>): `instrument` MUST be the
  executed tool, `agent` SHOULD be "a Person or Organization", and `endTime` SHOULD be present.
  `SoftwareApplication` uses `softwareVersion` (consumers "SHOULD prioritize softwareVersion"). It
  doesn't cover multiple agents or roles. The profile is built on **RO-Crate 1.3**: "This profile
  uses terminology from the RO-Crate 1.3 specification"
  ([profile page](https://www.researchobject.org/workflow-run-crate/profiles/0.6/process_run_crate/),
  fetched 2026-10-17). So declaring Process Run Crate conformance means moving the crate to 1.3, not to 1.2.
- **The RO-Crate JSON-LD context** (fetched for
  [1.1](https://w3id.org/ro/crate/1.1/context) and [1.2](https://w3id.org/ro/crate/1.2/context))
  defines the `prov:` prefix and maps `wasDerivedFrom` to `prov#wasDerivedFrom`. It maps
  `CreateAction`, `agent`, `instrument`, `participant`, `object`, `result`, `Role`, `roleName` and
  `softwareVersion` to schema.org. That makes `prov:`-prefixed terms valid compact IRIs in a crate.
  The two context versions map every term this ADR uses identically.
- **PROV-O** (<https://www.w3.org/TR/prov-o/>, namespace `http://www.w3.org/ns/prov#`, ontology document <https://www.w3.org/ns/prov-o>): roles are
  expressed with `prov:qualifiedAssociation` → `prov:Association {prov:agent, prov:hadRole}`, and
  delegation with `prov:actedOnBehalfOf`. `prov:hadActivity`'s domain includes `prov:Delegation`, so
  delegation can be scoped to an activity. **There is no `prov:softwareVersion`**: `grep -c
  softwareVersion` over the PROV-O Turtle returns 0. #564's sketch uses it, and this ADR uses
  schema.org `softwareVersion` instead.
- **DataCite 4.7** ([contributor](https://datacite-metadata-schema.readthedocs.io/en/4.7/properties/contributor/),
  released 2026-03-03): `contributorType` is mandatory when a contributor is given, from a closed list
  of 22 values. The list has no reviewer, approver or software agent. `nameType` is only `Personal` or
  `Organizational`. `nameIdentifier` + `nameIdentifierScheme` carry ORCID or ROR.
  [`descriptionType`](https://datacite-metadata-schema.readthedocs.io/en/4.7/properties/description/)
  includes `TechnicalInfo`.

### 9.2 RO-Crate (P2a)

The exporter keeps everything it emits today. On top of that, the root `Dataset` gains
`"prov:wasGeneratedBy": {"@id": "#generation"}` and one `CreateAction`. Every entity is a direct
`@graph` child, referenced by `{"@id"}` only (RO-Crate 1.1's JSON-LD appendix, the rule PR #541's
`the_rocrate_graph_is_flattened_and_references_are_id_only` asserts over the whole document):

```json
{ "@id": "#generation", "@type": ["CreateAction", "prov:Activity"],
  "result": {"@id": "./"},
  "object": [{"@id": "urn:tessera:source:…"}],
  "instrument": [{"@id": "#producer"}, {"@id": "#actor.impl"}, {"@id": "#actor.rev"}],
  "agent": [{"@id": "https://orcid.org/0000-0002-1825-0097"}],
  "participant": [{"@id": "https://ror.org/0example00"}],
  "prov:qualifiedAssociation": [{"@id": "#assoc.impl"}, {"@id": "#assoc.rev"},
                                {"@id": "#assoc.owner"}, {"@id": "#assoc.lab"}] },
{ "@id": "#producer", "@type": "SoftwareApplication", "name": "anatomist", "softwareVersion": "0.3.0" },
{ "@id": "#actor.impl", "@type": ["SoftwareApplication", "prov:SoftwareAgent"],
  "name": "agent-harness", "softwareVersion": "2.1.0",
  "additionalProperty": [{"@id": "#actor.impl.model"}, {"@id": "#actor.impl.brief"}],
  "prov:actedOnBehalfOf": {"@id": "https://orcid.org/0000-0002-1825-0097"},
  "prov:qualifiedDelegation": {"@id": "#delegation.impl"} },
{ "@id": "#actor.impl.model", "@type": "PropertyValue", "name": "model", "value": "model-x-1" },
{ "@id": "#delegation.impl", "@type": "prov:Delegation",
  "prov:agent": {"@id": "https://orcid.org/0000-0002-1825-0097"},
  "prov:hadActivity": {"@id": "#generation"} },
{ "@id": "#assoc.impl", "@type": "prov:Association",
  "prov:agent": {"@id": "#actor.impl"}, "prov:hadRole": {"@id": "#role.implementer"} },
{ "@id": "#role.implementer", "@type": "prov:Role", "name": "implementer" },
{ "@id": "https://orcid.org/0000-0002-1825-0097", "@type": ["Person", "prov:Person"] }
```

Mapping rules:

- **schema.org first.** RO-Crate tooling and the Run Crate profiles read it. `producer` and every
  `software_agent` are `instrument`s, because schema.org's `agent` range excludes software. `person`
  and `organization` actors with role `approver` are the `agent`, which is RO-Crate's "human agent
  who was responsible for authorizing". When there is no approver, `owner` is the `agent`. Every other
  person or organisation is a `participant`.
- **PROV-O for what schema.org can't say.** A qualified association per actor entry carries the
  exact `role`. `prov:actedOnBehalfOf`, plus a `prov:Delegation` scoped to `#generation` with
  `prov:hadActivity`, carries delegation. Software agents are typed `prov:SoftwareAgent`, people
  `prov:Person`, and organisations `prov:Organization`.
- **What the crate preserves, and what it doesn't.** The previous draft said "lossless for roles and
  delegation", which was too strong. **Roles are exact:** one association per entry. **Delegation is
  exact for software agents**, because each entry is its own `#actor.<id>` node. For a person or
  organisation, **one identity is one node**, so if that identity appears in two entries with
  different `acted_on_behalf_of` (reviewer on behalf of the lab, approver on behalf of nobody), the node
  carries the **union** of the delegations. PROV-O relates delegation between two agents, optionally
  within an activity, never per role. `prov:hadRole`'s narrower `rdfs:domain`, `Association ∪
  InstantaneousEvent`, excludes `prov:Delegation`, so tagging a delegation with a role would be
  outside PROV-O's OWL profile. The exact (role, principal) pairing lives in the sealed manifest,
  which is the record of truth. The crate is a projection of it (RFC §0.6), and the book says so.
- **`@id`s are collision-proof by construction.** The previous draft's `#actor-<id>` collided:
  local ids may contain `-` (§5.4), so actor `impl-model` became `#actor-impl-model`, which is also the
  `@id` of actor `impl`'s `model` property. Every generated id now joins its segments with `.`, a
  character neither local ids (`[a-z0-9_-]`, §5.4) nor roles (`[a-z0-9_]`, §5.3) can contain:
  `#actor.<id>`, `#actor.<id>.<property>`, `#assoc.<id>`, `#delegation.<id>`, `#role.<role>`. The
  `<property>` segment is one of the fixed names (`model`, `provider`, `harness_version`, `brief`,
  `receipt`) or `sampling.<key>`. A `sampling` key is caller data, so it is percent-encoded: every
  byte outside `[A-Za-z0-9_-]` becomes `%XX`, including `.`. So a key can never forge another segment.
  The fixed `#generation` and `#producer` contain no `.`. A `person` or `organization` uses its
  `identity` IRI, which is RO-Crate's ORCID-as-`@id` convention and never starts with `#`. A
  `urn:tessera:actor:` pseudonym is used as-is. One person in two roles is two actor entries (§5.3) but
  a single `@graph` node, so entities are de-duplicated by `@id`, and each entry adds its own
  association pointing at that node. P2a's test asserts that no `@id` appears twice, using the
  reviewer's `impl` / `impl-model` pair as its fixture. `model`, `provider`, `harness_version` (when
  not `softwareVersion`), `sampling.*`, `brief` and `receipt` become flattened `PropertyValue`
  entities. Each digest's description names it as BLAKE3 over the referent's bytes, following PR
  #541's truthful-label rule. schema.org doesn't list `SoftwareApplication` in `additionalProperty`'s
  domain. That is advisory in schema.org, and the JSON-LD stays valid, but a strict schema.org
  validator may warn.
- **Invalid or newer input is refused, not exported.** `verify` checks hashes, not
  `validate_actors` (§10.1), so a product can verify and still carry, say, a dangling
  `acted_on_behalf_of`. Exported, that would be a crate referencing an entity that doesn't exist, and
  the flattening assertion would fail. So both exporters run `validate_actors` first and refuse with
  `Error::InvalidActor`. A manifest whose `newer_fields()` is non-empty (§2.2), which includes an
  unknown actor kind (§5.2), is refused with `Error::NewerFormatFields`. A crate silently missing a
  field it didn't understand would be the same silent loss P0 refuses on re-seal. The user upgrades
  tessera, then exports. Attestation (§6.4) isn't exported: the crate states claims, as the manifest
  does.
- **Sources.** `isBasedOn` is unchanged. Edges with roles `derived_from` and `ingested_from`, and any
  domain derivation role, add `wasDerivedFrom` (already mapped by the context). `supersedes` and
  `snapshot_of` add `prov:wasRevisionOf`. `ingested_via_spec` is a plan, not a derivation, so it adds
  nothing beyond `object`.
- **No time.** `endTime` (a SHOULD in RO-Crate and Process Run Crate) is omitted, because the export
  is a pure function of the sealed manifest (`export.rs:1-5`) and wall-clock is in `aux/`. So the
  crate does **not** claim Process Run Crate conformance (no `conformsTo` profile). Declaring it
  means moving to RO-Crate 1.3 (§9.1) and reading `endTime` from `aux/actors.json`. That is a
  separate decision (§12).
- The root `Dataset` isn't additionally typed `prov:Entity`, which #564 proposes. Everything with an
  `@id` is already a `prov:Entity` by PROV's own definition, and nothing reads the triple.
- `conformsTo` stays `https://w3id.org/ro/crate/1.1`. Moving to 1.2 or 1.3 changes no mapping: the
  [1.3 context](https://w3id.org/ro/crate/1.3/context), fetched 2026-10-17, maps `prov`,
  `wasDerivedFrom` and every schema.org term above exactly as 1.1 and 1.2 do. But the move is its own
  decision (§12).

### 9.3 DataCite (P2b)

- Each `person`/`organization` actor **with a `name`** becomes a `contributors[]` entry
  `{name, nameType: Personal|Organizational, contributorType: "Other", nameIdentifiers: [{nameIdentifier: <identity>,
  nameIdentifierScheme: "ORCID"|"ROR", schemeUri}]}`. `nameIdentifiers` is emitted only for an
  ORCID or ROR identity, because DataCite requires a scheme whenever an identifier is present. `Other` is used because the closed
  list has no reviewer or approver, and mapping `approver` to `Supervisor` or `owner` to
  `RightsHolder` would make a false claim. DataCite requires `name`, so a name-less actor isn't a
  contributor. It still appears in the summary below.
- **Software agents aren't contributors**, since `nameType` has no software value. One
  `descriptions[]` entry `{descriptionType: "TechnicalInfo"}` carries a deterministic one-line-per-actor
  summary: role, kind, harness/version, model, and on-behalf-of. The producer gets its own line
  there too, because DataCite emits no producer today.
- `creators` stay as they are (`metadata.creator` or `(:unav)`, `export.rs:127-133`). An actor is not
  promoted to creator automatically, because authorship is the depositor's call.
- P2b also changes `docs/book/src/provenance-signing.md:21` from "*who built this*" to "`producer` +
  `actors` answer who", with a pointer to the new actors section (#564 P2).

### 9.4 A native PROV export: not now

`tessera export --format prov` (PROV-JSON or PROV-O Turtle) is **out of scope**. P2a's RO-Crate is
already JSON-LD, and therefore RDF. Its `prov:` triples (activity, associations, roles, delegation,
derivation) form a PROV-O graph that any JSON-LD processor extracts. PROV-JSON is a W3C Member
Submission, not a Recommendation. A native exporter would be a third serialiser to keep in agreement,
with no adopter asking for it (Owner decision 7).

### 9.5 Sequencing against PR #541

PR #541 (`fix(export)!`, head `1e0f3d7`, green) rewrites `export.rs` (+601 lines). It introduces the
flattened `PropertyValue` entity pattern and the whole-document flattening assertion that P2a reuses.
**P2a starts after #541 merges** and rebases onto it: there is no stacked PR (devkit#1759, stacked PRs
get no CI) and no parallel edit of the same functions. P0 and P1 don't touch `export.rs`, so they
aren't blocked by it.

## §10 — The surfaces

### 10.1 Rust (`tessera-core`, P1a)

```rust
impl Actor {
    pub fn software_agent(id: impl Into<String>, role: impl Into<String>, harness: impl Into<String>) -> Self;
    pub fn person(id: impl Into<String>, role: impl Into<String>, identity: impl Into<String>) -> Self;
    pub fn organization(id: impl Into<String>, role: impl Into<String>, identity: impl Into<String>) -> Self;
    pub fn on_behalf_of(self, principal_id: impl Into<String>) -> Self;
    // software_agent only; on another kind they are a typed error at seal, not a silent no-op
    pub fn with_model(self, model: impl Into<String>) -> Self;
    pub fn with_harness_version(self, v: impl Into<String>) -> Self;
    pub fn with_provider(self, p: impl Into<String>) -> Self;
    pub fn with_sampling(self, key: impl Into<String>, value: serde_json::Value) -> Self;
    pub fn with_brief(self, digest: impl Into<String>) -> Self;
    pub fn with_receipt(self, digest: impl Into<String>) -> Self;
    pub fn with_name(self, name: impl Into<String>) -> Self;   // person / organization
}
impl ProductBuilder {
    pub fn add_actor(&mut self, actor: Actor) -> &mut Self;     // appends; order is sealed
}
pub fn validate_actors(actors: &[Actor]) -> crate::Result<()>;  // called by seal(); public for writers
```

`seal()` calls `validate_actors` and refuses, as `Error::InvalidActor { index, id, reason }` (a new
variant; `Error` is `#[non_exhaustive]`, `error.rs:13`), any of the following: a duplicate or
malformed `id`; a malformed `role`; a dangling, self-pointing or cyclic `acted_on_behalf_of`; a
`brief`/`receipt` that isn't `blake3:<64 hex>`; a missing kind-required field; a disallowed
`identity` form, including a malformed pseudonym (§7.2); an email-shaped string or `mailto:` in any
actor string, or an `@` in `name` (§7.3); a builder method used on the wrong kind; or an `actors` key in
`id_inputs` (§3). `validate_manifest` runs the same
check, so `tessera validate` reports foreign-written violations. `verify` doesn't, because it is
about hashes.

**The carry rule** (P1a, on top of #574's fix). Actors record the acts that produced **this
version**, and two reserved roles, `reviewer` and `approver`, **attest a version**. If they were
carried, a `commit --set` by anyone would inherit the parent's approval, and §8's
`required_actor_roles = ["approver"]` claim check would pass for a version nobody approved. §8 is a
claim check either way; the carry rule makes sure the engine never manufactures the false claim
itself. So:

- `from_manifest` **does not carry** `actors`. This matches `producer` (re-stamped for each version,
  `product.rs:49-51`). The parent's actors stay reachable through the `supersedes` edge `commit`
  writes. (Its `derived_from` sources are not version edges.)
- `publish` **copies them verbatim**. It is a snapshot of the *same* version, and copying them is
  part of #574's "`publish` reproduces every sealed provenance slot".
- `commit --keep-actors` carries the parent's actors **except** those with an attesting role. A new
  version gets a fresh review or approval only through `--add-actor`. The attesting set is the two
  reserved names. A domain role with attesting meaning isn't known to the engine, so it is carried;
  a schema-declared `attesting_roles` list is the extension point if a second adopter needs one
  (§12).
- **The ingest engine's re-seal** (`apply_spec_metadata`, `engine.rs:1224-1255`) is a re-seal inside
  the *same* act, not a new version. Today it builds through `from_manifest` (`engine.rs:1234`),
  which **drops** the incoming manifest's `producer` and `generation` (`product.rs:49-51`). It then
  attaches the *spec's* `generation`/`producer` (`engine.rs:1249-1254`). So an incoming value is lost
  whenever the spec doesn't restate it; #574 covers that loss. The previous draft said the re-seal
  "already" re-attaches them, which was wrong. For actors, P1b does explicitly what the code doesn't
  do for the other slots: it copies the incoming manifest's actors (from `ingest --actors` or a
  decoder) onto the builder that `from_manifest` returns, then appends the spec's
  `[[product.actors]]`. Its early-return guard (`engine.rs:1230-1233`) must also count
  `!p.actors.is_empty()` as provenance, so a spec whose only provenance is actors isn't skipped. The
  test `ingest_reseal_keeps_incoming_actors` pins the copy. Whether `producer`/`generation` get the
  same treatment is #574's call (Owner decision 9).
- The list is validated at seal, after keeps, removals and additions. If a kept actor is
  `acted_on_behalf_of` a dropped approver, the commit is refused unless that approver is re-added
  under the same local `id`. That is the operator saying, explicitly, that the principal approved
  this version too.

### 10.2 Streaming write and ingest (`tessera-io`, `tessera-ingest`, P1b)

- `WriteSession`'s staging `Header` gains `actors: Vec<Actor>` beside `producer`/`generation`
  (`write.rs:53-79`). Its top-level `deny_unknown_fields` means an older binary resuming a stage
  that has actors fails loudly, which is #462's intended outcome.
- The ingest spec gains `[[product.actors]]`. It is the same shape as §5.1 in TOML and is validated at
  spec load with the same `validate_actors`:

  ```toml
  [[product.actors]]
  id = "impl"
  role = "implementer"
  kind = "software_agent"
  harness = "agent-harness"
  model = "model-x-1"
  acted_on_behalf_of = "owner"
  sampling = { effort = "high" }
  ```

### 10.3 Python (`tessera-py`, P1c — after #562)

The methods are typed keywords per kind, in #562's `with_producer(tool=…, version=…)` style. `add_`
is used because they append, matching the existing `add_source`:

```python
b.add_software_agent(id="impl", role="implementer", harness="agent-harness",
                     harness_version="2.1.0", provider=None, model="model-x-1",
                     sampling={"effort": "high"}, brief="blake3:…", receipt=None,
                     acted_on_behalf_of="owner")
b.add_person(id="owner", role="approver", identity="https://orcid.org/…", name=None)
b.add_organization(id="lab", role="owner", identity="https://ror.org/…", name="Example Lab")
```

**Rejected: one `add_actor(dict)`.** A typo'd key would be a silent no-op or a late seal error,
whereas a keyword argument is a Python `TypeError` at the call. A validation failure at `pack()` is a
`TesseraError` carrying the index and id. The Python `Reader` exposes `actors` as a list of dicts.
`tests/write_example.py` gains an ANCHOR region, `bindings.md` `{{#include}}`s it, and
`tests/api_drift.py` registers the three methods.

**`tessera.digest(data: bytes) -> str`** returns `blake3:<64 hex>`, the same function as
`tessera_core::hash::digest`. Today `tessera-py` exposes only `open` and `verify` as module functions
(`tessera-py/src/lib.rs:375-395`), so a Python caller filling `brief`/`receipt` would need a separate
blake3 package and could get the prefix or case wrong, which `validate_actors` refuses. A test asserts
that `tessera.digest(b"")` equals the Rust digest of the empty input.

**P1c is blocked behind #562**, which is in flight on another branch and edits the same `Builder`.
That isn't optional ordering: without #562's `with_producer`, Python can't write the `producer` that
§6.5's change verification compares.

### 10.4 CLI (P1b, P1d, P1e, P3d)

- `tessera commit REPO LINEAGE` gains three repeatable flags. `--add-actor <JSON | @file.json>` appends.
  `--keep-actors` carries the parent's non-attesting actors (§10.1). `--remove-actor <id>` drops a
  kept actor from the **new** version. It does not erase the old one, which stays reachable through
  `supersedes` (§7.5; that is #591). Each counts as a change for the "nothing to change" check
  (`version.rs:63-67`). The seal-time validation names any delegation left dangling. On a
  de-identified product, `--add-actor` and `--keep-actors` apply §7.4's refusal.
- `tessera actor pseudonym` (P1e) prints a fresh `urn:tessera:actor:<26 base32>` from `getrandom`
  (§7.2). It is the supported way to mint one.
- Every `tessera ingest <format>` subcommand gains `--actors @file.json` (a JSON array in §5.1's
  shape), next to the shared `--meta` (`main.rs:2043-2060`). Each actor has several fields, so a
  `key=value` flag would be the wrong tool.
- `tessera inspect` prints the actors after `generation`, in the style of #417's `producer_lines`
  (`tessera-cli/src/nav.rs:615-690`):

  ```text
  actors        4 (implementer, reviewer, approver, owner)
    impl        software_agent  implementer  agent-harness/2.1.0  model=model-x-1  → owner
    rev         software_agent  reviewer     other-harness        model=model-y-2  → owner
    owner       person          approver     https://orcid.org/0000-0002-1825-0097  attested (key alice)
    lab         organization    owner        https://ror.org/0example00  "Example Lab"  unattested
                (model and harness are self-reported claims; see ADR-0060 §6.3)
                (unattested = claimed by the writer, not signed by a trusted key pinned to that identity; §6.4)
  ```

  Every `person`/`organization` row ends in `attested (key <name>)` or `unattested`, by §6.4's rule.
  With no signature or an empty trust store, every such row is `unattested`. That is the honest default,
  not an error.

  `--full` adds `sampling`, `provider`, and the `brief`/`receipt` digests in full. `--resolve`
  dereferences a digest to a carried block (§5.5). `inspect --json` needs no change, because it is
  the manifest. `tessera provenance` adds one `actors` summary line per hop. Unknown kinds and
  `aux/actors.json` orphans print as warnings (§4, §5.2).
- `aux/actors.json` (P1d): `--actors-aux @file.json` on `ingest`, and `add_aux_members` for library
  writers. `inspect` lists it with the other aux members (`nav.rs:549`, `nav.rs:972`) and joins it
  under `--full`.

## §11 — Phasing

Each step is one PR under ~400 diff lines, lands in this order, and has a test that fails without it.
The pre-change failure is shown once in the PR.

| Step | Scope | Acceptance criteria → proving test |
|---|---|---|
| **#587** (prerequisite, format-wide) | Refuse integers outside ±(2^53−1) in sealed JSON at seal, validate and verify (§2.2) | #587's probe (`9007199254740993` edited to `…992`) now fails to seal on the write side, and the hand-edited manifest fails verify. The 8 corpus goldens are unchanged, because no corpus value exceeds 2^53 |
| **P0a** (#573, after #587) | Object-level verification of `Manifest`; `newer_fields()` by the declared-field-name walk (opaque maps exempt); warnings in `verify`/`inspect`; `inspect --json` prints the object; `from_manifest` becomes fallible with `NewerFormatFields`; the verifier applies #587's integer rule | §2.1 cases B and D now **verify Ok** and list `actors` / `producer.agents` → `forward_compat_newer_field_verifies_and_warns`. Case C now fails `Integrity` → `spliced_unsealed_field_is_tampering`. Case E, `"study": null` and an embedded `requires_generation: false` verify and list nothing → `known_keys_are_never_newer_fields`. `from_manifest` on case B → `NewerFormatFields`. All 8 corpus goldens unchanged (existing conformance test). An unknown field holding a float and a nested object has a pinned RFC 8785 digest that both readers reproduce, and one holding `2^53 + 1` is refused by both → `unknown_field_seal_is_pinned` |
| **P0b** (#573) | The same for `Collection`; `ProductSchema::from_value` reports unknown keys at any depth by the same walk and `validate_manifest` refuses; SPEC §5 missing rows and the §7 forward-reading rule; the reference reader still passes | An embedded schema with an unknown key at the top level, inside `fields[]` and inside `blocks[]` → `validate` = `NewerFormatFields`, while `inherit: false` / `requires_generation: false` validate → `unknown_schema_rule_is_not_validated`. A collection with an unknown key verifies → `collection_forward_compat`. `run.py` exit 0 |
| **#574** | `publish` carries `producer`/`generation`/`ingest_transform`; `commit` carries `ingest_transform` (and `generation`, per Owner decision 9) | #574's probe as a test, one assertion per slot, for both verbs |
| **P1a** | `Actor` types, `validate_actors` (§7.2 forms and pseudonym shape, §7.3 email ban, `id_inputs` ban), `add_actor`, the seal check, the carry rule, `Error::InvalidActor`, the role constants, the SPEC `actors` row, the `actors_chain` corpus fixture, the book section (§6.3, §6.4, §6.5, §7.5's "what is safe today") | The JSON byte form of §5.1 = committed golden → `actor_json_shape_golden`. Each refusal in §10.1 → `invalid_actor_*`, one per rule, including `invalid_actor_email_in_sampling` (a nested `sampling` string), `invalid_actor_at_in_name`, `invalid_actor_pseudonym_shape` and `actors_in_id_inputs_is_refused`. Every semver form in §7.3's table is accepted in `harness_version`, `harness` and a nested `sampling` string: `pkg@1.2.3`, `tool@latest`, `pkg@1.0.0-alpha`, `pkg@1.0.0-alpha.beta`, `pkg@1.0.0-beta.rc`, `pkg@v1.0.0-beta.rc`, `pkg@1.0.0-rc.1+build.5`, `pkg@1.0.0+exp.sha.5114f85`, `npm:@scope/pkg@2.1.0-next.3`, `pkg@^1.2.0-beta.x` and `pkg@~1.2.3-rc.a` → `version_pins_are_not_emails`. The three refused emails in the table are refused, and the two digit-led-domain emails pass, which pins the documented false negative → `email_pattern_matches_its_table`. `tool@next.beta` is refused → `domain_shaped_tag_is_refused`. An empty list leaves all 8 goldens unchanged. Different actors, same `id`, different `manifest_hash` → `actors_are_version_not_lineage`. `commit` without `--keep-actors` carries none, `--keep-actors` drops `reviewer`/`approver`, and `publish` copies verbatim → `carry_rule_never_inherits_an_approval`. A kept delegation to a dropped approver is refused → `kept_delegation_needs_reapproval`. An unknown kind using only known field names (`identity`, `name`) round-trips and is listed, and `publish`/`--keep-actors` refuse it → `unknown_actor_kind_is_opaque` |
| **P1b** | Staging `Header.actors`; `[[product.actors]]`; `ingest --actors`; `commit --add-actor/--keep-actors/--remove-actor` | A spec whose only provenance is `[[product.actors]]` seals them through `apply_spec_metadata` → `spec_with_only_actors_seals_them`. Incoming `--actors` survive the re-seal → `ingest_reseal_keeps_incoming_actors`. A resumed stage with actors is refused by a header without the field → `stage_header_refuses_unknown_actors`. A `--keep-actors` commit keeps the implementer, drops the approver, and accepts a re-added approver → a trycmd case |
| **P1e** | `tessera actor pseudonym`; §7.4's de-identification refusal in `tessera-ingest`, called by ingest, `commit` and `tessera validate` | `dicom --deidentify` and `--recipient` with a ROR organisation, an ORCID operator, a `name` or `--actors-aux` are refused, and with pseudonyms only they seal → `deidentified_ingest_accepts_pseudonyms_only`. `commit --add-actor` of an ORCID person onto that product is refused → `deidentified_commit_refuses_named_actor`. `tessera validate` flags a library-written de-identified product with a named actor. `actor pseudonym` output matches §7.2's regex and two runs differ |
| **P1c** (after #562) | Python `add_software_agent`/`add_person`/`add_organization`; `tessera.digest`; `Reader.actors`; anchor + `bindings.md`; `api_drift.py` | `write_example.py` anchor writes and reads back the §5.1 chain, with `brief` from `tessera.digest`. `tessera.digest(b"")` equals the Rust digest. A bad kwarg is a `TypeError`. A dangling delegation at `pack` is a `TesseraError` naming index and id. `tessera-py-import` check |
| **P1d** | `aux/actors.json`: writer, `--actors-aux`, `inspect --full` join, orphan warning; the writer refuses `host`/`user`/`login`/`ip` and email-shaped values | Adding or removing it leaves both hashes byte-identical (ADR-0042's assertion) → `aux_actors_is_outside_the_seal`. A `host` key → refused → `aux_actors_refuses_personal_keys`. Orphan key → warning in a trycmd case |
| **P2a** (after #541) | RO-Crate `CreateAction`, producer `SoftwareApplication`, qualified associations, roles, delegation (`actedOnBehalfOf` + scoped `prov:Delegation`), `wasDerivedFrom` / `wasRevisionOf`, `PropertyValue`s, the `.`-joined `@id` scheme, refusal of invalid or newer input | The whole-document flattening assertion holds with actors. Actors `impl` and `impl-model` plus a `sampling` key `a.b` produce no duplicate `@id` → `rocrate_ids_never_collide`. Every actor and role is reachable from `#generation`. The approver is `agent`. Software is never `agent`. A dangling `acted_on_behalf_of` that verifies → `InvalidActor`, and a newer field → `NewerFormatFields`, with no crate written → `export_refuses_invalid_or_newer_actors`. JSON-LD expansion against the pinned 1.1 context gives `prov#qualifiedAssociation` → an expansion test on a vendored context. No sealed byte changes |
| **P2b** | DataCite contributors + `TechnicalInfo`; book sentence; the same refusals as P2a | A named person → `Personal` + ORCID scheme. A name-less person → not a contributor, present in the summary. A software agent → never a contributor. Invalid or newer actors → refused. Mandatory-field test still green |
| **P3** (P3a + P3b, **one PR**) | P3a: `ProductSchema.required_actor_roles`, documented and worded as a claim check. P3b: §6.4's pinned-identity rule, `verify-sig --require-signer-role <role>`, and #590's fix to `--require-signer`, sharing one lookup | A schema requiring `approver` refuses a manifest without one and accepts one with it. A `commit --keep-actors` of an approved version fails the claim check until `--add-actor` re-approves → `approval_does_not_survive_commit`. Built-in embedded schema bytes unchanged (8 goldens). A key pinned to A that signs with label B fails `--require-signer-role approver` when the approver is B, and also when it is A (label/pin mismatch) → `signer_role_uses_the_pin_not_the_label`. An unpinned key, or `--pubkey`, refuses → `signer_role_needs_a_pinned_key`. The persona trycmd is extended with the approver chain |
| **P3d** (after P3) | `inspect`'s attested/unattested marks (§6.4, §10.4), reusing P3b's lookup | `inspect` shows `attested (key …)` only for the pinned approver, `unattested` for an `acted_on_behalf_of` principal who didn't sign, and `unattested` for every row with no signature or an empty trust store → a trycmd case |
| **P3c** | `tessera verify-change <product> --change <record> --role <role>` (§6.5); the book's change-record section with `subject_git_repo`/`subject_git_commit` | A host-signed product whose `producer.git_commit` matches an approver-signed change record, with its `organization` operator attested by the host key → Ok, with no approver actor on the product → `change_approval_without_a_person_on_the_product`. A mismatched commit, `dirty = true`, an unattested approver, an unattested operator (host key unpinned or pinned to another identity), a `software_agent` operator (`operator_must_be_attestable`), a record without subject keys, or an `approved_change` edge pinning a different record → typed refusals, one test each |
| **P4** (deferred, filed when an adopter needs it) | `identity_commitment` + the `aux/identity/` envelope (§7.5) | A key holder recomputes the commitment from the decrypted envelope and matches it. A swapped envelope fails the match. A shred removes the mapping and leaves the seal unchanged |

Erasing one version from a repository (§7.5) is **#591**, a repository verb that isn't specific to
actors. Nothing in P0-P4 depends on it, because §7.5's "what is safe today" guidance needs no verb.

## Owner decisions needed

1. **Does P0 ship one release ahead of P1?** *Recommended: yes.* Readers then exist that can verify
   an `actors` product before any is written. *Cost:* `actors` waits one release cycle. The
   alternative (same release) leaves every earlier install reporting new products as tampered, which
   the release notes can only warn about. #587 lands before P0 either way (§2.2).
2. **Accept P0's rule ("verify the object, warn on newer fields, refuse re-seal, validate and
   export") as the format's forward-compatibility policy.** It amends SPEC §4/§7 and corrects
   `inspect --help`. *Recommended: yes.* It is RFC §13 and SPEC §4 as written, and it makes the Rust
   reader agree with the reference reader. *Cost:* verification parses the manifest twice (as an
   object, then as the view), which is cheap at manifest scale. `from_manifest` becomes fallible (a
   public API change). Older binaries can no longer `commit`, `publish` or export newer products:
   they get a typed refusal instead of silent loss (§9.2).
3. **The personal-data rules of §7**: refuse `mailto:` and scheme-less identities; refuse
   email-shaped strings in every actor string and `@` in `name`; random pseudonyms only, and no names,
   in de-identified products. *Recommended: yes.* *Cost:* an adopter who identifies people by email has
   to mint pseudonyms first. Login names in software fields stay possible (§7.3's partial dispute).
4. **Accept §7.5's interim privacy guidance**: seal a real identity only with consent to permanent
   attribution, and use pseudonyms for everyone else, until #591 (redacting commit) exists.
   *Recommended: yes, and schedule #591 independently of this ADR.* *Cost:* a product shows fewer
   ORCID iDs than an adopter might like.
5. **Accept §6.5's change record**: the person who approves a change signs a separate record for the
   code revision, and P3c binds products to it through the sealed `producer.git_commit`. Derived
   products carry no approver. *Recommended: yes.* *Cost:* one more product type per adopter, plus
   a metadata-key convention (`subject_git_repo`/`subject_git_commit`) that P3c reads. The alternative,
   a typed `subject` field in the format, is more format surface for one consumer.
6. **P3b's strictness and #590**: `--require-signer-role` and the fixed `--require-signer` refuse
   when the envelope label differs from the pinned identity, and refuse unpinned keys and `--pubkey`.
   *Recommended: yes, both in P3.* *Cost:* a behaviour change for anyone using `--require-signer` with
   a key trusted without `--signer`. They re-add it with `tessera trust add --signer`.
7. **No native PROV-JSON/PROV-O exporter for now.** *Recommended: no exporter.* *Cost:* a consumer who
   wants only PROV extracts the `prov:` triples from the RO-Crate JSON-LD, and accepts §9.2's union
   of delegations for a person who appears in several roles.
8. **Accept §10.1's carry rule for actors**: `commit` carries none by default, `--keep-actors` never
   carries `reviewer`/`approver`, and `publish` copies verbatim. *Recommended: yes.* *Cost:* an
   operator who edits metadata has to re-add an approval explicitly, which is the point.
9. **Should `generation` be carried on `commit`?** ADR-0058 §1 restamps `producer` and
   `product.rs:49-51` also drops `generation`. #574 argues that blocks carried unchanged keep their
   recipe. *Recommended: carry it unless the caller replaces it.* *Cost:* it amends ADR-0058's
   intent, so the owner should rule on it in #574.

## Consequences

- A `.tsra` can say who acted, in which role and on whose behalf, typed and sealed. A signature binds
  that record with no envelope change, and the exporters finally keep the PROV promise of
  `white-paper.md:1662`.
- The forward-compatibility hole (#573) is fixed for every future additive field, not just this one.
  The two readers agree with SPEC §4 again.
- Corpus-neutral: no golden changes, no `TESSERA_VERSION` bump, one fixture added.
- New permanent format surface: one manifest field, three kinds, a reserved role vocabulary, and one
  schema key. All of them are additive under P0's rule.
- Honest limits are written where users see them: a model name is a claim (§6.3); a person or
  organisation actor is `unattested` unless a pinned key signed it (§6.4); a sealed identity can't be
  erased from copies already shared, and today not even from your own store without dropping the
  lineage (§7.5, #591).
- The audit's spin-offs are filed rather than buried: #587 (integers above 2^53), #590
  (`--require-signer` trusts a label) and #591 (no per-version erasure).

## Non-decisions / deferred

- **Actors on collections.** Collections are assembled, too. The same type can be added to
  `Collection` additively under P0. File it when a collection-level act needs recording.
- **Process Run Crate conformance** (`conformsTo` + `endTime` from `aux/actors.json`), which
  requires the **RO-Crate 1.3** `conformsTo` bump (§9.1): export-only, with no seal impact.
- **Per-version erasure** in a repository: #591, a repository verb that isn't specific to actors (§7.5).
- **Kind or identity-form requirements per role in schemas** (§8).
- **Schema-declared `attesting_roles`** for domain roles that attest a version (§10.1). The engine
  knows only the two reserved ones until a second adopter needs more.
- **Cross-corpus actor queries** ("everything this model generated"): a downstream index concern
  (#297/#299), like ADR-0058's generation-graph queries.
- **Verifiable model attestation** (provider-signed outputs, TEE quotes): no mechanism exists to bind
  to yet. When one does, it rides as a counter-signature or `attestation` record (ADR-0037 §3's
  deferred fork), not as an actor field.
