# ADR-0061 — DICOM de-identification: the PS3.15 Annex E profile, table-driven

**Status:** Proposed (2026-10-17, design for [#579](https://github.com/vig-os/tessera/issues/579), P0: it gates the
first DICOM ingest of MorePET/anatomist). **Amends** ADR-0047 §1 step 4 and its "completeness by
construction" claim (both are false today, see §0). Implements ADR-0040 §1 (PS3.15 seeds the tiers) for DICOM.
Builds on ADR-0042 (sealed vs `aux/`), ADR-0048 (the provenance edge), ADR-0056 §6 (the sealed
`ingest_transform` record) and ADR-0058 §5 (inheritance of `identifying` fields).

> Mechanism, not legal certification (ADR-0040/0047, restated). This ADR decides what Tessera removes,
> replaces and attests, and what it cannot do. Whether a de-identified product is "anonymous" or
> "pseudonymous" under GDPR / FADP / HIPAA is the DPO's call, not a flag.
>
> Design-only. No code changes until the owner accepts this document. Every claim about current behaviour
> below is pinned to a `file:line` on `dev` at `fa581f5` or to a probe whose command and output are quoted.

## §0 — What is wrong today (verified)

`deidentify()` (`tessera/crates/tessera-ingest/src/dicom.rs:686-695`) removes the 12 top-level tags in
`PHI_TAGS` (`dicom.rs:657-670`, self-described as "a pragmatic high-risk subset") and sets
PatientIdentityRemoved = `YES`. Everything else in the header survives into the sealed
`extra["dicom_header"]` (`dicom.rs:799-803`), and the curated UIDs and date go into the sealed metadata
(`dicom.rs:773-782`).

**Probe.** I built `tessera` from `fa581f5` (`cargo build -p tessera-cli`, exit 0) and wrote a synthetic
two-slice veterinary CT with pydicom (invented `FICTUS` names, never real data). I ingested it three ways
with `tessera ingest dicom-series`: plain, `--deidentify`, and `--recipient <age1…>` followed by `shred` +
`verify` (all exit 0). Then I counted the seeded strings in the `.tsra` bytes (`grep -c -a -F`; the container
is STORED, so a byte grep sees every member):

| Seeded value (tag) | plain | `--deidentify` | crypto-shred, after `shred` |
|---|---|---|---|
| PatientName, PatientID, BirthDate, Accession, Referring, Institution, Study/SeriesDescription | 0 | 0 | 0 |
| ResponsiblePerson (0010,2297), ResponsibleOrganization (0010,2299) | 0 | **1** | **1** |
| InstitutionAddress, StationName, DeviceSerialNumber, StudyID, ImageComments (with a phone no.) | 0 | **1** | **1** |
| OperatorsName (0008,1070) | 0 | **1** | **1** |
| RequestedProcedureID / SPS ID inside RequestAttributesSequence (0040,0275) | 0 | **1** | **1** |
| Private (0009,1001) person name + its creator | 0 | **1** | **1** |
| StudyDate (also in metadata `study_date`), StudyTime | 1 | **5** | **5** |
| Study / Series / SOP Instance / Frame of Reference UIDs | 1 | **2-3** | **2-3** |
| Source path (`ingested_from.reference = "dcm/s1.dcm,dcm/s2.dcm"`) | 1 | **1** | **1** |

Seven findings follow. Each one is a design input below:

1. **De-identifying leaks more than not de-identifying.** On the keep-PHI path, the header is blanked
   (`without_raw_header`, `engine.rs:1136-1142`), so the plain product has no `extra` at all (probe: `KeyError:
   'extra'`). The de-identified paths seal the 12-tag-scrubbed header, so `--deidentify` adds 40 attributes
   to the seal.
2. **ADR-0047 §1 step 4 is not implemented.** It says crypto-shred "drop[s] every curated `identifying`-tier
   field from the manifest". The probe's `--deidentify` manifest carries `study_instance_uid` and
   `series_instance_uid`, which the `recon` schema marks `Identifying` (`schema.rs:555-567`), plus
   `study_date`. ADR-0047's "allowlist the few safe fields; encrypt the rest" is also not what the code does: it
   applies a blocklist.
3. **Only top-level elements are touched.** `remove_element` acts on the top-level dataset. `header_to_json`
   recurses into every sequence (`dicom.rs:402-439`), so a PHI attribute nested in a sequence item is sealed
   (probe: RequestAttributesSequence).
4. **Private tags are kept** (probe: (0009,1001)). Annex E removes them by default (§2).
5. **The source path is sealed** on every path unless `--source-label` is given
   (`engine.rs:404-406`, `engine.rs:455-461`). ADR-0040
   names that path as PHI.
6. **The identity envelope holds slice 1 only.** `read_series_crypto_shred` captures the first slice's
   header (`dicom.rs:503-513`), so per-slice identifiers of slices 2..n are destroyed, not encrypted.
   `IdentityDocument.curated_identifying` holds `patient_name`/`patient_id`/the two UIDs (`dicom.rs:470-478`).
   Its doc comment promises `patient_pseudonym` and `study_date` (`identity.rs:37-38`), and neither is there.
7. **The texts claim more than the code does.** The `phi_present` docs say "ingested products are PHI-free
   regardless" (`dicom.rs:674-675`). The ingest warning says `--deidentify` "strip[s] PS3.15 tags"
   (`engine.rs:56-58`). The FEATURE-MATRIX crypto-shred row says "de-identifies the sealed product". The
   tests that could have caught this only assert that PatientName/PatientID are absent
   (`dicom.rs:1606-1666`, `tessera-cli` `crypto_shred_*`). They check what was removed, never that nothing
   else is left.

The defect shipped: `v0.1.0-alpha.1` (`a35c61b`, published 2026-09-24) carries the same 12-tag `PHI_TAGS`
(`git show v0.1.0-alpha.1:tessera/crates/tessera-ingest/src/dicom.rs`, line 657).

## §1 — The decision, in one place

1. **The profile is DICOM PS3.15 Annex E, Basic Application Level Confidentiality Profile, edition 2026d.**
   It is applied from **vendored, generated tables**, never from a hand-picked list (§2). Basic alone is the
   default. The Retain options in §3 can be added. Clean Descriptors / Structured Content / Graphics / Pixel
   Data / Recognizable Visual Features are **not supported**, and they are never attested.
2. **Recursive and fail-closed.** Actions apply at every sequence depth. All private tags are removed
   (Retain Safe Private is opt-in, from the standard's own table). A public tag that the vendored PS3.6 2026d
   dictionary does not know is removed and counted.
3. **UIDs are replaced by keyed derivation**: `2.25.<UUIDv8(BLAKE3-keyed(K, uid))>`. This is deterministic,
   so S15 holds. It keeps referential integrity inside its keyspace, and nobody without K or the original files
   can link a replacement to its original. By default K is per ingest run, derived from the raw source bytes.
   A custodian key file widens the keyspace across runs (§4).
4. **Dates and times are removed by default.** Retain Full Dates keeps them. Retain Modified Dates shifts every
   date by a whole-day, per-subject, keyed offset. The sealed `study_date` follows the same rule (§5).
5. **The seal carries the de-identified header, never the original.** The originals of every attribute the
   profile touched, on **every slice**, go into the ADR-0047 envelope with the UID map, the date offset and the
   source paths. Without recipients there is no envelope and that data is **dropped**. `header = "none"`
   seals no header at all (§6).
6. **Attestation tells the truth.** (0012,0062) = `YES`, (0012,0063) names the profile and edition,
   (0012,0064) holds exactly the CID 7050 codes that were applied, and (0028,0303) records what happened to
   dates. A sealed `ingest_transform` record named `dicom_deidentify` binds the edition, the table digest, the
   options and the keyspace into `manifest_hash` (§7).
7. **Pixel data is out of scope.** BurnedInAnnotation (0028,0301) = `YES` or RecognizableVisualFeatures
   (0028,0302) = `YES` makes a de-identifying ingest **refuse** (§8).
8. **New seal-time gates.** No schema `Identifying` field may appear in a de-identified seal unless the profile
   produced it or the operator declared it a pseudonym. The source reference is opaque. The operator's
   `timestamp` must not reproduce a date the profile removed (§9).
9. **Back-compat.** There is no DICOM product in the conformance corpus, so no corpus regen. Products from the
   old path are a disclosure matter: a docs security note, detection in `inspect`/`verify`, and an advisory (§10).

## §2 — The profile is data: vendored tables with provenance

**Source.** NEMA publishes the standard as DocBook XML at
`https://dicom.nema.org/medical/dicom/current/source/docbook/part{15,06}/part{15,06}.xml`. When this ADR was
written, both were edition **2026d** (the `DICOM PS3.15 2026d` / `DICOM PS3.6 2026d` subtitle in the files).

| File | Table used | Rows |
|---|---|---|
| `part15.xml` | E.1-1 Application Level Confidentiality Profile Attributes | 657 |
| `part15.xml` | E.3.10-1 Safe Private Attributes | 479 |
| `part06.xml` | Registry of DICOM Data Elements (tag, VR, retired) | — |

Source digests (sha256, fetched 2026-10-17):

```text
part15.xml  1eb2a9724822476b3f1f3de23d15ae655a4c7f6e9d32bf16f2d9ef76d7d8437c
part06.xml  fa9f0f8d67eba692dcf95cfeae0a8f4e071beeb7844215a860bc4b9d5dfaad25
```

Table E.1-1 has 15 columns: name, tag, retired, in-standard-IOD, then **Basic** plus the ten option columns
Safe Private, UIDs, Device Identity, Institution Identity, Patient Characteristics, Full Dates, Modified Dates,
Clean Descriptors, Clean Structured Content and Clean Graphics. The Basic column is measured at
`X` 417 · `D` 93 · `U` 55 · `Z` 42 · `X/D` 23 · `X/Z` 11 · `X/Z/D` 8 · `Z/D` 6 · `X/Z/U*` 2. Option cells are
only `K` or `C`. Three rows are tag patterns: `(50xx,xxxx)`, `(60xx,3000)`, `(60xx,4000)`. One row is the
odd-group private rule.

**Vendored form.** A stdlib Python generator, `scripts/dicom-deid-tables.py`, reads the two XML files and emits
compact TSVs under `tessera/crates/tessera-ingest/data/ps3.15/`. They are about 17 KB for E.1-1, plus E.3.10-1
and a tag/VR/retired list from PS3.6. Each TSV has a header comment with the edition, the source URL, the
source sha256 and the generator version. The XML itself is **not** vendored (3.5 MB and 9.7 MB). The TSVs are
embedded with `include_str!` and parsed once into a static table. This adds **no new dependency**.

**Resolving the compound codes.** Most of the table's IOD-conformance qualifiers (`X/Z`, `X/D`, `X/Z/D`, `Z/D`,
`X/Z/U*`) exist so that a de-identifier can **re-emit a valid DICOM instance**. Tessera does not emit DICOM.
The sealed header is a JSON projection, so no Type 1/2 constraint applies. Every non-`K`, non-`U`, non-`C`
action therefore resolves to **remove** (`X`). A `D` dummy or a `Z` empty value adds no utility and only
widens the surface. `X/Z/U*` also resolves to remove. A future DICOM egress (`export dicom`) must resolve the
qualifiers against PS3.3 IOD types. That is a separate design, filed as a follow-up.

**Tags the table does not list.** Annex E protects the listed attributes and retains the other standard ones.
Tessera follows that, with one fail-closed addition. A **public** tag that the PS3.6 2026d dictionary does not
know (one added in a later edition, or malformed) is **removed** and counted in the record (§7). It is not
retained, because a classification that does not exist cannot be trusted. Retired attributes are in the
dictionary, and E.1-1 lists 102 of them, so they are handled like any other attribute.

**Veterinary tags.** E.1-1 lists ResponsiblePerson (0010,2297) and ResponsibleOrganization (0010,2299) as `X`,
and PatientSexNeutered (0010,2203) as `X/Z`, which resolves to remove and is kept under Retain Patient
Characteristics. **ResponsiblePersonRole (0010,2298), PatientSpeciesDescription (0010,2201) and
PatientBreedDescription (0010,2292) are not in E.1-1**, so the standard retains them. The probe's
`Canis lupus familiaris` species string stays in the seal under this ADR, which is the intended outcome. A
consumer that wants the role gone uses `also_remove` (§3.2).

**Edition drift.** Two checks:

- *hermetic* (in the flake): the embedded TSVs parse, every action cell is one of the measured codes, the row
  counts match the header (657 / 479), and the header's source sha256 equals the one this ADR records. A
  hand edit to a TSV fails because the generator also writes a BLAKE3 digest of the TSV body into the header.
- *scheduled* (non-hermetic, weekly CI, report-only): fetch `current`, re-run the generator, and open an issue
  when the edition or a table changes. Moving to a new edition is a deliberate PR. It changes the record's
  `edition` and `table_digest`, and therefore the `manifest_hash` of later de-identified ingests, as it should.

**Rejected: the `dicom-anonymization` crate** (crates.io 0.3.1, carecoders). Its README documents `HashUID`
("generates new UIDs from hashes") and `HashDate` ("shifts dates based on hash of the `PatientID`"). Those are
unkeyed hashes, and PS3.15 E.1.1 warns about exactly them: "unencrypted hashes are considerably less secure and
should be avoided, since they are vulnerable to trivial dictionary based attacks". The crate also records no
edition provenance, and it would be a new dependency, which trips `feature-snapshots`.

**Rejected: an allowlist-only seal as the default.** ADR-0047 proposed this. It is offered as `header = "none"`
(§6) but is not the default. Annex E is the field-accepted, auditable line. An allowlist is a Tessera-private
judgement that no reader can check against a standard, and it would drop the acquisition parameters
(reconstruction, dose, radiopharmaceutical) that make a DICOM-derived product reusable.

## §3 — Options: which are supported, the default, and how they are selected

| Option (PS3.15 E.3) | CID 7050 code | Supported | Notes |
|---|---|---|---|
| Basic Profile | DCM 113100 | **always** | the base of every de-identifying ingest |
| Retain Safe Private | DCM 113111 | yes | keeps only (creator, element) pairs listed in E.3.10-1 |
| Retain UIDs | DCM 113110 | yes | `U` → `K`; originals sealed (linkable to the source PACS); explicit only |
| Retain Device Identity | DCM 113109 | yes | `K` rows kept; its 11 `C` rows (AE titles etc.) keep the Basic action |
| Retain Institution Identity | DCM 113112 | yes | 10 `K` rows |
| Retain Patient Characteristics | DCM 113108 | yes | 14 `K` rows (sex, age, weight, size, …); 5 `C` rows stay Basic |
| Retain Longitudinal Full Dates | DCM 113106 | yes | 169 date/time rows `K`; exclusive with Modified Dates |
| Retain Longitudinal Modified Dates | DCM 113107 | yes | the 169 rows shifted (§5) |
| Clean Descriptors | DCM 113105 | **no** | needs a vetted text-cleaning dictionary |
| Clean Structured Content | DCM 113104 | **no** | SR content-item cleaning |
| Clean Graphics | DCM 113103 | **no** | overlays and curves are removed (Basic) |
| Clean Pixel Data / Recognizable Visual Features | 113101 / 113102 | **no** | §8 |

The codes and meanings were checked against PS3.16 2026d, CID 7050 ("De-identification Method", version
20170914).

**Where an option cell is `C`, Tessera keeps the Basic action (remove).** For example, Retain Device Identity
cleans Station AE Title (0008,0055). Removing it is stricter than cleaning, so claiming the option stays
truthful. This is documented beside the option list in the book.

**Default = Basic alone.** It is the most protective setting and needs no key. PET quantitation (SUV) needs body
weight (`X` under Basic, `K` under Patient Characteristics) and radiopharmaceutical start time and series time
(removed under Basic, time-of-day kept under Modified Dates). The book documents that recipe as
`options = ["retain-patient-characteristics", "retain-longitudinal-modified-dates"]`. It is not a default,
because the owner of the data has to choose it.

### §3.1 — Selection surface

In the ingest spec, the existing `deidentify = true` keeps working and means Basic. A new optional table
configures the rest:

```toml
[[product]]
format = "dicom-series"
inputs = ["…"]
recipients = ["age1…"]          # optional: also writes the envelope
[product.deidentify_profile]
options = [
  "retain-patient-characteristics",
  "retain-longitudinal-modified-dates",
]
key_file = "keys/site.deidkey"  # optional custodian key (§4)
also_remove = ["0010,2298"]     # additive only
header = "deidentified"         # or "none" (§6)
pseudonym_fields = ["patient_pseudonym"]  # §9
```

`key_file` is a path, never an inline secret. `pseudonym_fields` names operator-supplied `--meta` values that
are declared pseudonyms.

If the table is present, or `recipients` is non-empty, de-identification is on. An unknown option name,
`also_remove` on an `X` tag (a no-op) and the two exclusive date options together are each a **hard error**.
The CLI mirrors the spec: `--deid-option <name>` (repeatable), `--deid-key <file>`, `--deid-also-remove
<GGGG,EEEE>` (repeatable), `--deid-header none`. `tessera keygen --deid OUT` writes a 32-byte random custodian key
(hex, mode 0600), on the same model as `keygen --age`.

### §3.2 — Extending the set declaratively (the anatomist ask)

The #579 thread asks that the de-identification set be "a full PS3.15 baseline ∪ every field the product
schema marks `identifying`". That needs a binding from a schema field to a DICOM tag. **None exists today:**
`FieldSpec` has no tag field (`schema.rs:71-111`), and ingest only ever loads `SchemaRegistry::builtin()`
(`engine.rs:203`, `engine.rs:1198`), so a consumer schema cannot even be registered. So:

- **now**, `also_remove` in the spec. It is sealed in the record (§7), it is additive only, and it needs no code
  change per consumer;
- **later**, when consumer product schemas become registrable, an optional `dicom_tag` on an `Identifying`
  `FieldSpec` feeds the **same** set. That is a follow-up issue gated on schema registration, and the
  owner decision D3 below.

There is deliberately **no `also_keep`**. A caller cannot weaken the standard profile except through a named
PS3.15 option, because only those are attestable.

## §4 — UIDs: keyed, deterministic replacement and the keyspace

`U` attributes (55 Basic rows, plus UIDs inside `X/Z/U*` sequences, which are removed whole) are replaced with

```text
h    = BLAKE3-keyed(K, "tessera/dicom-deid/uid/v1" ‖ uid)
uid' = "2.25." + decimal(UUIDv8(h[0..16]))
```

`2.25.` is the PS3.5 B.2 UUID-derived root. Setting the RFC 9562 version (8) and variant bits makes the 128 bits
a valid custom UUID, so the result is a valid UID of at most 64 characters. The same `uid` maps to the same
`uid'` everywhere the profile meets it: top level, nested items, every slice. That keeps referential integrity
across a series, and keeps a shared FrameOfReferenceUID shared between PET and CT.

**The key K, and why it is never derivable from the seal.**

- *Per run (default).* K is BLAKE3 `derive_key` with the context `tessera 2026-10 dicom-deid per-run key v1`
  over the raw source bytes of every DICOM member of the spec run, length-prefixed, in spec order. It is
  computed in the same streaming pass as `source_digest`. The context string differs from the sealed
  `ingested_from.content_hash`, so the seal reveals nothing about K. Computing K requires **the original files**, and a holder of those can already link
  the product through the bit-identical pixels. Products in one spec run share a keyspace, so PET and CT in one
  collection stay consistent. Separate runs get unlinkable UIDs.
- *Custodian.* `K` = the 32 bytes of `key_file`. Every run with the same key gets the same mapping, which gives
  longitudinal linkage across products for whoever holds the key. The record carries
  `key_id`: the first 8 bytes, in hex, of BLAKE3 `derive_key` with the context `tessera dicom-deid key id v1`
  over K. That reveals only what equal
  replacement UIDs already reveal: that the products share a keyspace.

Determinism (S15): the same inputs, options and key give the same bytes. There is no randomness anywhere.

**Rejected:** random UIDs, because they break S15 and the content-addressed identity. An unkeyed hash of the
UID, because of the dictionary/confirmation attack above. A key derived from sealed values (the spec hash or the
source merkle root), because that is an unkeyed hash with extra steps.

**Where the mapping lives.** `original → replacement` for every replaced UID on every slice goes into the
envelope (§6). Without recipients, the map is not stored. A custodian with `key_file` can recompute the
replacement of a **known** original UID, but can never invert it. Without a custodian key, nobody can, short of
holding the originals.

`ReferencedSOPInstanceUID` and friends under `K` sequences are covered by the same rule, because E.1-1 lists
them as `U` wherever they appear.

## §5 — Dates and times

| Mode | E.1-1 date/time rows (169) | (0028,0303) LongitudinalTemporalInformationModified | sealed `study_date` |
|---|---|---|---|
| Basic (default) | removed | `REMOVED` (E.2 requires adding it) | **absent** |
| Retain Full Dates | kept | `UNMODIFIED` | original |
| Retain Modified Dates | DA/DT shifted by −Δ days; TM kept | `MODIFIED` | shifted |

**The shift.** `Δ = 1 + (BLAKE3-keyed(K, "tessera/dicom-deid/date-shift/v1" ‖ PatientID) mod 3652)` days, so
between 1 and 3652 (about 10 years) and never 0. Shifting by whole days keeps time-of-day and all
intervals inside the subject exactly. It also keeps SUV decay correction, which subtracts
times. Δ is consistent per subject within the keyspace (§4): per run by default, across runs with a custodian
key. DT values lose their UTC-offset suffix under Modified Dates. TimezoneOffsetFromUTC (0008,0201) keeps its
Basic action, because its `C` cell resolves to remove. Δ itself goes into the envelope.

**Schema fields.** `study_date` is `Sensitive` (`schema.rs:569-575`) and follows the table above. Any field the
schema marks `Identifying` is subject to the §9 gate. The `timestamp` gate (§9) covers the operator's sealed
`Manifest.timestamp`.

**Rejected:** coarsening to year or month as a third mode. It is not a PS3.15 option, so it cannot be attested
in (0012,0064), and Modified Dates protects better while keeping the intervals.

## §6 — Sealed vs envelope

| Material | Sealed (`manifest_hash`) | Envelope (`aux/identity/identity.age`, recipients only) |
|---|---|---|
| De-identified header of the first slice (`extra["dicom_header"]`) | yes, or no with `header = "none"` | — |
| Curated non-identifying metadata (modality, rescale, kVp, geometry, manufacturer, model) | yes | — |
| Replaced UIDs in curated metadata (`study_instance_uid`, …) | yes (pseudonyms, §9) | originals |
| `study_date` | per §5 | original |
| Original values of **every attribute the profile acted on, per slice** | never | yes |
| UID map, date offset Δ, original source paths | never | yes |
| The `dicom_deidentify` record (§7) | yes | — |

**Identity document v2.** The new shape is `{ v: 2, slices: [{ modified_attributes: { "GGGG,EEEE": {vr, value} } }],
uid_map, date_shift_days, source_references, curated_identifying }`. `modified_attributes` holds the original
of every element whose action was not `K`, at its full sequence path. This is the content PS3.15 puts in the
Modified Attributes Sequence (0400,0550) of an Encrypted Attributes Data Set. Tessera does not use DICOM's
RSA/AES encoding for it, because the sealed product is not a DICOM instance, and it keeps ADR-0047's `age`
X25519 envelope, which sits outside the seal for determinism. Storing the touched attributes only, instead of N
full headers, bounds the envelope to the identifying content. v1 envelopes still decrypt: `reidentify` reads
both versions.

**Without recipients** (`--deidentify` alone) there is no envelope. The originals, the UID map and Δ are
**dropped**, and that is the documented meaning of destructive de-identification. A custodian key does not
change this. It only makes forward mapping reproducible.

**The keep-PHI default is unchanged by this ADR.** It warns, seals no header, and seals the curated fields in
the clear. Making de-identification the default is the owner decision D1.

## §7 — Attestation: what the product says about itself

In the sealed header (unless `header = "none"`):

- PatientIdentityRemoved (0012,0062) = `YES`;
- DeidentificationMethod (0012,0063) = `"Tessera <version>: DICOM PS3.15 2026d Basic Application Level
  Confidentiality Profile[; <option names>]"`;
- DeidentificationMethodCodeSequence (0012,0064): one item `(113100, DCM, "Basic Application Confidentiality
  Profile")` plus one item per applied option, using the code meanings verbatim from CID 7050;
- LongitudinalTemporalInformationModified (0028,0303) per §5.

These four attributes are set **after** the profile runs, so the profile cannot remove them. An attribute with
these tags in the source is overwritten. (0012,0064) never lists an option this ADR does not support.

**In the seal, whatever the header mode:** one `IngestTransform` (ADR-0056 §6, `Manifest.ingest_transform`,
already sealed by `ProductBuilder::with_ingest_transform`, `product.rs:141-147`) named `dicom_deidentify`, with
params

```json
{
  "profile": "PS3.15-annex-e-basic",
  "edition": "2026d",
  "table_digest": "blake3:…",
  "options": ["…sorted…"],
  "private": "removed | safe-table",
  "keyspace": "per-run | custodian",
  "key_id": "…custodian only…",
  "also_remove": ["…"],
  "header": "deidentified | none",
  "counts": {
    "removed": 0,
    "replaced_uids": 0,
    "shifted": 0,
    "unknown_public_removed": 0
  }
}
```

This is the machine-checkable claim. `inspect` prints it, and the legacy detector (§10) looks for its absence.
The counts are deterministic for fixed inputs. **Rejected:** stating the claim only in (0012,0063), because
free text cannot be checked, and with `header = "none"` there is no header to put it in.

## §8 — Pixel data and burned-in annotation: out of scope

This ADR does not inspect or alter pixels. Clean Pixel Data and Clean Recognizable Visual Features are not
offered. **Honoured flags:** if any slice has BurnedInAnnotation (0028,0301) = `YES` or RecognizableVisualFeatures
(0028,0302) = `YES`, a de-identifying ingest **refuses**. The error names the flag and says that pixel
de-identification is not implemented. If the flags are absent, the profile proceeds, and Tessera never sets
either flag itself, because setting one would be a claim about pixels. Overlay data (60xx,3000) and curve data
(50xx,xxxx) are removed by Basic, because they are header elements, not pixel data. This is a known limit for
ultrasound and secondary capture, and the book states it.

## §9 — Seal-time gates (fail-closed)

1. **Identifying fields.** On a de-identifying ingest, the current warning (`engine.rs:224-245`) becomes a
   **refusal**. The ingest stops if a field the schema marks `Identifying` is in sealed metadata, unless its
   value was produced by the profile (a replaced UID) or is listed in `pseudonym_fields` (an operator-issued
   pseudonym such as `patient_pseudonym`). Both kinds are listed in the record. This also covers the ADR-0058
   §5 inheritance: what a derived product inherits is the pseudonym.
2. **Opaque source reference.** On a de-identifying ingest without `--source-label`, `ingested_from.reference` is
   `urn:tessera:source:<content_hash>`, the ADR-0048 export form. The paths go into the envelope.
   `content_hash` (the merkle root over the raw files) stays as ADR-0048 decided. Only a holder of the original
   bytes can confirm it, and that holder can already compare pixels.
3. **Timestamp.** Unless Full Dates is selected, the operator's `timestamp` must not fall on a calendar date
   that the profile removed or shifted (StudyDate, SeriesDate, AcquisitionDate, ContentDate, on any slice). The
   error suggests a non-identifying timestamp, or `timestamp = "from-source"`, which takes the
   (post-profile) StudyDate+StudyTime and so works under Modified Dates and fails under Basic. The CLI help
   for `--timestamp` ("Acquisition timestamp (ISO-8601), recorded verbatim", `main.rs:987-988`) is rewritten
   to say this.
4. **`name` and `study`** are operator text. The book says they must not carry identity. They cannot be
   checked mechanically.

## §10 — Back-compat and disclosure

- **Corpus.** `tessera/corpus/ingest-corpus.json` and `corpus.json` contain no DICOM fixture (a search for
  `dicom` in both returns nothing), so **no corpus regen**. The golden DICOM pin
  (`dicom.rs:988-1023`) pins `content_hash`, which covers the pixel block only, so it does not move.
  `manifest_hash` of de-identified DICOM ingests changes, by design. The only tests that observe it are the
  `crypto_shred_*` pair, which compare two runs and keep holding.
- **Products already sealed by `--deidentify` / `--recipient`** (since `5313975`, 2026-06-26, and in
  `v0.1.0-alpha.1`) carry PatientIdentityRemoved = `YES` while leaking the §0 classes. A seal cannot be fixed in
  place. `shred` does not help, because the leak is inside the seal. **Remediation:** re-ingest from source with
  this profile, and tell every recipient of an old copy.
- **Detection:** `inspect` and `verify` print a `legacy de-identification` warning when a `recon` product's
  `extra.dicom_header` has (0012,0062) = `YES` and no `dicom_deidentify` record exists. This is a warning, not a
  failure, because the seal is still valid.
- **Disclosure:** a "DICOM de-identification before ADR-0061" security note in the book and FEATURE-MATRIX, and
  a GitHub security advisory for the published pre-release (owner decision D4).
- **API.** `PHI_TAGS`, `phi_present` and `deidentify` are public in `tessera-ingest` (pre-1.0). They are
  **removed**, not deprecated, because keeping a public "PHI tag list" that is wrong is the defect. Their
  replacements are `deid::Profile`, `deid::apply` and `deid::Record`. The `CHANGELOG` is release-plz's, so the
  PR uses `feat(ingest)!`.

## §11 — Tests (each must fail on `fa581f5`; the first PR shows that failure once)

1. **Every row, generated.** For each E.1-1 row whose resolved Basic action is not `K`, generate an element with
   the PS3.6 VR and a unique marker value (`PHI-<gggg><eeee>`, a unique date, a unique `1.2.826.0.1.3680043.99.*`
   UID). Also seed: the veterinary tags, a private block with a person name, nested copies inside a sequence two
   levels deep, a File Meta SourceApplicationEntityTitle, and a PHI-bearing input path. Ingest a 3-slice series
   through the engine three ways: `--deidentify`, crypto-shred, and crypto-shred then `shred`. **Assert that no
   marker occurs anywhere in the `.tsra` bytes** outside `aux/identity/identity.age`, and after `shred` nowhere.
   The test asserts the container is STORED, so a byte search is valid. It also asserts the marker count it
   generated equals the table's non-`K` row count, so an empty fixture cannot pass.
2. **Option matrix.** For each supported option alone, and for the PET recipe: the rows that option turns into
   `K` keep their marker, every other marker is absent, and (0012,0064) lists exactly the applied codes.
3. **UIDs.** Same input + same K → same bytes (S15). Per-run key differs from custodian key. One original UID
   on two slices and in a nested reference → one replacement. PET+CT sharing FrameOfReferenceUID in one spec →
   still shared. The replacement is valid (`2.25.` prefix, at most 64 chars, UUIDv8 bits).
4. **Dates.** Basic → no source date occurs in the seal. Modified → every date shifted by the same Δ, intervals
   preserved, Δ ≠ 0, Δ recoverable from the envelope. The timestamp gate refuses a removed date.
5. **Gates.** BurnedInAnnotation `YES` → refused. An unregistered `Identifying` metadata field → refused. A
   declared pseudonym → allowed and recorded.
6. **Envelope v2.** `reidentify` returns the originals of every slice's touched attributes, the UID map and Δ.
   v1 envelopes still decrypt.
7. **Conformance-table drift** (§2), hermetic.
8. **Legacy detector.** A product built the old way is flagged by `inspect`/`verify`.

## §12 — Phasing

Ordered sub-issues, each about 400 hand-written lines or fewer (the generated TSVs are not counted).

- **P1 — Vendored tables.** The generator script, the three TSVs with provenance headers, a static parser and
  the hermetic drift test. *Acceptance:* §11.7 green; row counts 657 / 479; source sha256 equal to §2.
- **P2 — Profile engine** (`tessera-ingest::dicom::deid`). `Profile { edition, options, also_remove, keyspace }`
  and `apply(obj) -> (obj', Record, Modified)`: recursive, private and unknown-public removal, UID derivation,
  date shift, attestation attributes. Pure, no I/O. *Acceptance:* §11.1-3 at object level; §11.4 shift unit
  tests.
- **P3 — Ingest wiring.** Replace `PHI_TAGS`/`deidentify`; curated fields from `obj'`; the `ingest_transform`
  record; the spec table and CLI flags; `keygen --deid`; the per-run key in the `source_digest` pass; the
  opaque source reference. *Acceptance:* §11.1-2 end to end through `engine::run` and the CLI; S15 re-ingest
  byte-identical.
- **P4 — Gates and envelope v2.** The §9 gates, the §8 refusal, `IdentityDocument` v2 (per-slice modified
  attributes, UID map, Δ, paths), and `reidentify` reading v1 and v2. *Acceptance:* §11.4-6.
- **P5 — Legacy detection and docs.** The `inspect`/`verify` warning; the book (`ingest.md`,
  `provenance-signing.md`, a de-identification page with the option table and the PET recipe); the
  FEATURE-MATRIX rows rewritten; the security note; an amendment banner on ADR-0047; the scheduled
  edition-drift workflow. *Acceptance:* §11.8; `guardrails-gates` green.

P1 → P2 → P3 → P4 is strict. P5's detector can land after P3. **Follow-up issues** (filed with the plan, not
built here): Clean Descriptors; Clean Pixel Data / Recognizable Visual Features; DICOM egress resolving the
IOD-conformance qualifiers; the schema `dicom_tag` binding (blocked on consumer-schema registration);
`header = "none"` as a default (if D2 goes that way).

## Owner decisions needed

- **D1. Is de-identification the default for DICOM ingest?** *Recommended: yes from P3 (Basic). Keep-PHI becomes
  an explicit `--keep-identity`.* Cost: a breaking CLI change, and every internal script that relied on PHI in
  the clear must add the flag. Today the unsafe path is the default, and a forgotten flag ships identity.
- **D2. Default sealed header: de-identified header (recommended) or `none`.** The de-identified header keeps
  the acquisition parameters reusable and follows an auditable standard. `none` is strictly safer against
  unknown-unknowns (free text in attributes that Annex E retains), but discards most DICOM-derived context.
  The honest cost of the recommendation: Tessera's privacy is exactly Annex E's, no stronger.
- **D3. The anatomist extension:** `also_remove` in the spec now (recommended) and a schema `dicom_tag` binding
  once schemas are registrable, or wait and do only the schema route. Waiting blocks anatomist's first ingest
  on an unbuilt feature (schema registration).
- **D4. Disclosure:** a GitHub security advisory for `v0.1.0-alpha.1`, plus a book security note
  (recommended), or the note only. The cost of the advisory is public acknowledgement. Not filing one leaves
  pre-release users unaware that their "de-identified" products are not.
- **D5. Burned-in / recognizable flags:** refuse (recommended), or allow with a sealed acknowledgement that
  drops PatientIdentityRemoved to `NO`. Refusing blocks de-identifying ingest of ultrasound/secondary capture
  until Clean Pixel exists. Allowing makes the attestation conditional, and readers will miss that.

## Consequences

- One coherent, edition-pinned, testable claim replaces a 12-tag list. Every de-identified product says, inside
  its seal, which standard edition and options produced it.
- No new crate dependency. About 17 KB of generated, provenance-stamped data, and one stdlib Python generator.
- The de-identified seal now leaks less than the keep-PHI seal, not more (§0 finding 1).
- Determinism is unchanged: every derived value is a keyed function of the inputs.
- `manifest_hash` changes for de-identified DICOM ingests. There is no corpus regen.
- Residual risk, stated: free text in attributes Annex E retains; pixel content (§8); linkage by anyone holding
  the original files (pixels and `content_hash`); the operator's `name`/`study`; and the ADR-0047 §4 limits
  (no revocation, no audit) for the envelope.
