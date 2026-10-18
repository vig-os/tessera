# ADR-0061 — DICOM de-identification: the PS3.15 Annex E profile, table-driven

**Status:** Proposed (2026-10-17, design for [#579](https://github.com/vig-os/tessera/issues/579), P0: it gates the
first DICOM ingest of MorePET/anatomist; revised 2026-10-18 after the round-1, round-2 and round-3 audits of PR #592,
see the revision log at the end). **Amends** ADR-0047 §1 step 4, its "completeness by construction" claim and its §4
erasure claim (§0, §4.3), and ADR-0035's spec-hash pre-image for de-identifying runs (§9.5). Implements ADR-0040
§1 (PS3.15 seeds the tiers) for DICOM. Builds on ADR-0042 (sealed vs `aux/`), ADR-0048 (the provenance edge),
ADR-0056 §6 (the sealed `ingest_transform` record) and ADR-0058 (sealed `producer`, inheritance of `identifying`
fields).

> Mechanism, not legal certification (ADR-0040/0047, restated). This ADR decides what Tessera removes,
> replaces and attests, and what it cannot do. Whether a de-identified product is "anonymous" or
> "pseudonymous" under GDPR / FADP / HIPAA is the DPO's call, not a flag.
>
> Design-only. No code changes until the owner accepts this document. Every claim about current behaviour
> below is pinned to a `file:line` on `dev` at `fa581f5` or to a probe whose command and output are quoted.
> Every claim about the standard cites PS3.6 / PS3.15 / PS3.16 edition 2026d.

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
is STORED, so a byte grep sees every member, but only text: see finding 9):

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

Nine findings follow. Each one is a design input below:

1. **De-identifying adds a header that keep-PHI does not seal.** On the keep-PHI path, the header is blanked
   (`without_raw_header`, `engine.rs:1136-1142`), so the plain product has no `extra` at all (probe: `KeyError:
   'extra'`). The de-identified paths seal the 12-tag-scrubbed header, 40 attributes the plain product does not
   carry. So the owner, address, comment, device, nested and private rows of the probe table appear **only** with
   `--deidentify`. Both paths seal the curated UIDs, `study_date` and the source path. Keep-PHI is not safer
   overall: it seals PatientName-family curated fields in the clear by design, and warns about it.
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
   (`engine.rs:404-406`, `engine.rs:455-461`). ADR-0040 names that path as PHI.
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
8. **The spec-run provenance edge seals PHI** (found by the round-1 audit, re-verified). `run_into` computes
   `spec_hash(spec)` (`engine.rs:160`): an **unkeyed** BLAKE3 over the canonical JSON of the whole parsed spec
   (`spec.rs:599-608`), input paths included. It takes `spec_ref = spec_path.display()` verbatim
   (`engine.rs:161`) and seals both as the `ingested_via_spec` edge of every member (`engine.rs:184`,
   `engine.rs:322-324`). A guessed path set can be confirmed against the hash, and the spec path itself is
   sealed in the clear.
9. **The byte grep in the probe is blind to binary VRs** (found by the round-1 audit). `value_to_json` renders
   every primitive through `to_multi_str` (`dicom.rs:406-409`), so an OB/OW/UN value is sealed as decimal
   numbers. A private OB marker and an implicit-VR private PN were sealed and the grep counted 0. The audit
   probed this at `fa581f5`. A text grep is therefore not an oracle, and §11 replaces it.

The defect shipped: `v0.1.0-alpha.1` (`a35c61b`, published 2026-09-24) carries the same 12-tag `PHI_TAGS`
(`git show v0.1.0-alpha.1:tessera/crates/tessera-ingest/src/dicom.rs`, line 657).

## §1 — The decision, in one place

1. **The profile is DICOM PS3.15 Annex E, Basic Application Level Confidentiality Profile, edition 2026d.**
   It is applied from **vendored, generated tables**, never from a hand-picked list (§2). Basic alone is the
   default. The Retain options in §3 can be added. Clean Descriptors / Structured Content / Graphics / Pixel
   Data / Recognizable Visual Features are **not supported**, and they are never attested.
2. **Rules beyond the table, generated from PS3.6 and keyed by VR** (§2.1). Actions apply at every sequence
   depth. All private tags are removed (Retain Safe Private is opt-in). Groups 0000, 0002 and 0004 are removed
   from the dataset. A public tag the PS3.6 2026d dictionary does not know is removed. **Every UI value is
   replaced** unless its attribute is on a generated class/syntax/coding allowlist or the value is under the
   DICOM root `1.2.840.10008.`. **Every DA/DT/TM value follows the date rule**, listed in E.1-1 or not, as do
   the two alternative-calendar date fields. URI attributes that can carry instance UIDs are removed. **No UN
   element is ever sealed**, so nothing reaches the seal as opaque bytes. **Every PN value** is removed, as are
   person and organization names held in text VRs, text-VR copies of instance or document identifiers, and
   unlisted free-text comment fields (R9-R11).
3. **UIDs are replaced by keyed derivation**: `2.25.<UUIDv8(BLAKE3-keyed(K, uid))>`. This is deterministic,
   so S15 holds. It keeps referential integrity inside its keyspace, and nobody without K or the original files
   can link a replacement to its original. By default K is per ingest run, derived from the raw source bytes.
   A custodian key file widens the keyspace across runs, at a cost to erasure that is stated (§4).
4. **Dates and times are removed by default**, which now includes the 15 DA/DT/TM attributes E.1-1 does not
   list and the 2 alternative-calendar fields. Retain Full Dates keeps them. Retain Modified Dates shifts
   **every** surviving DA and DT by a whole-day offset keyed on (IssuerOfPatientID, PatientID), and refuses
   without a subject identity. The date mode **alone** governs every date-valued attribute: no other option can
   leave a date unshifted next to shifted ones (§3.0, §5).
5. **The seal carries the de-identified header, never the original.** The originals of every attribute the
   profile touched, on **every slice**, go into the ADR-0047 envelope with the UID map, the date offset and the
   source paths. Without recipients there is no envelope and that data is **dropped**. `header = "none"`
   seals no header at all (§6).
6. **Attestation tells the truth.** (0012,0062) = `YES`. (0012,0063) is multi-valued, one value of at most 64
   characters each, with no software version. (0012,0064) holds exactly the CID 7050 codes that were applied,
   and (0028,0303) records what happened to dates. A sealed `ingest_transform` record named `dicom_deidentify`
   binds the edition, the table digest, the options and the keyspace into `manifest_hash` (§7).
7. **Pixel data is out of scope.** A de-identifying ingest **refuses** when BurnedInAnnotation (0028,0301) or
   RecognizableVisualFeatures (0028,0302) is `YES`. It also refuses when BurnedInAnnotation is absent on a
   modality that routinely burns in text, unless the operator records a pixel review (§8).
8. **New seal-time gates** (§9). No schema `Identifying` field may appear in a de-identified seal unless the
   profile produced it or the operator declared it a pseudonym. Both provenance edges are opaque: the source
   reference, and the spec edge, whose hash pre-image is redacted. The operator's `timestamp` must not reproduce
   a date the profile removed.
9. **Back-compat.** There is no DICOM product in the conformance corpus, so no corpus regen. Products from the
   old path are a disclosure matter: a docs security note, detection in `inspect`/`verify`, and an advisory (§10).
10. **Phasing**: no phase ships an attestation stronger than what that phase enforces (§12).

## §2 — The profile is data: vendored tables with provenance

**Source.** NEMA publishes the standard as DocBook XML at
`https://dicom.nema.org/medical/dicom/current/source/docbook/part{15,06}/part{15,06}.xml`. When this ADR was
written, both were edition **2026d** (the `DICOM PS3.15 2026d` / `DICOM PS3.6 2026d` subtitle in the files).

| File | Table used | Rows |
|---|---|---|
| `part15.xml` | E.1-1 Application Level Confidentiality Profile Attributes | 657 |
| `part15.xml` | E.3.10-1 Safe Private Attributes | 479 |
| `part06.xml` | 6-1 Registry of DICOM Data Elements | 5268 |
| `part06.xml` | 7-1 File Meta Elements · 8-1 Directory Structuring Elements | 22 · 19 |

Source digests (sha256, fetched 2026-10-17, re-checked 2026-10-18):

```text
part15.xml  1eb2a9724822476b3f1f3de23d15ae655a4c7f6e9d32bf16f2d9ef76d7d8437c
part06.xml  fa9f0f8d67eba692dcf95cfeae0a8f4e071beeb7844215a860bc4b9d5dfaad25
```

Table E.1-1 has 15 columns: name, tag, retired, in-standard-IOD, then **Basic** plus the ten option columns
Safe Private, UIDs, Device Identity, Institution Identity, Patient Characteristics, Full Dates, Modified Dates,
Clean Descriptors, Clean Structured Content and Clean Graphics. The Basic column is measured at
`X` 417 · `D` 93 · `U` 55 · `Z` 42 · `X/D` 23 · `X/Z` 11 · `X/Z/D` 8 · `Z/D` 6 · `X/Z/U*` 2, so every one of the
657 rows has a non-`K` Basic action. Option cells are only `K` or `C`. Three rows are tag patterns: `(50xx,xxxx)`,
`(60xx,3000)`, `(60xx,4000)`. One row is the odd-group private rule.

**Vendored form.** A stdlib Python generator, `scripts/dicom-deid-tables.py`, reads the two XML files and emits
compact TSVs under `tessera/crates/tessera-ingest/data/ps3.15/`: E.1-1 (about 17 KB), E.3.10-1, the PS3.6
tag/keyword/VR/retired registry (Tables 6-1, 7-1 and 8-1), and the two generated policy lists of §2.1. Each TSV has
a header comment with the edition, the source URL, the source sha256, the generator version and a BLAKE3 digest
of the TSV body. The XML itself is **not** vendored (3.5 MB and 9.7 MB). The TSVs are embedded with
`include_str!` and parsed once into a static table. This adds **no new dependency**.

**Resolving the compound codes.** Most of the table's IOD-conformance qualifiers (`X/Z`, `X/D`, `X/Z/D`, `Z/D`,
`X/Z/U*`) exist so that a de-identifier can **re-emit a valid DICOM instance**. Tessera does not emit DICOM.
The sealed header is a JSON projection, so no Type 1/2 constraint applies. Every non-`K`, non-`U`, non-`C`
action therefore resolves to **remove** (`X`). A `D` dummy or a `Z` empty value adds no utility and only
widens the surface. `X/Z/U*` also resolves to remove. A future DICOM egress (`export dicom`) must resolve the
qualifiers against PS3.3 IOD types. That is a separate design, filed as a follow-up.

**Veterinary tags.** E.1-1 lists ResponsiblePerson (0010,2297) and ResponsibleOrganization (0010,2299) as `X`,
and PatientSexNeutered (0010,2203) as `X/Z`, which resolves to remove and is kept under Retain Patient
Characteristics. **ResponsiblePersonRole (0010,2298), PatientSpeciesDescription (0010,2201) and
PatientBreedDescription (0010,2292) are not in E.1-1**, so the standard retains them. The probe's
`Canis lupus familiaris` species string stays in the seal under this ADR, which is the intended outcome. A
consumer that wants the role gone uses `also_remove` (§3.2).

### §2.1 — Rules beyond the table (generated, keyed by VR)

Annex E protects the attributes it lists and retains the other standard ones. Five rules close the gaps that the
round-1 audit found between that list and the requirements of E.1.1 and E.3.6. The generator applies each rule
mechanically to PS3.6 2026d, and the result is vendored and digested like the table itself.

**R1 — groups 0000, 0002, 0004.** E.1.1: "All Data Elements with a Group Number of 0004 shall be removed from
any SOP Instance or DICOM File other than a DICOMDIR File". The File Meta Information "shall be replaced with a
description of the de-identifying application". Tessera removes **every** element of groups 0000, 0002 and 0004
found in the dataset body, at any depth. The real File Meta group is **never** sealed. Today that holds only
because `header_to_json` iterates the dataset (`dicom.rs:379`). In the probe, File Meta
SourceApplicationEntityTitle `FICTUSAE` occurred 0 times. §11.6 pins it so that it cannot regress silently.

**R2 — unknown public tags.** A public tag that is not in PS3.6 Tables 6-1, 7-1 or 8-1 is **removed** and
counted (`counts.unknown_public_removed`). Such a tag was added in a later edition or is malformed, and a
classification that does not exist cannot be trusted. Retired attributes are in the registry. E.1-1 lists 102 of
them, and they are handled like any other attribute.

**R3 — every UI value is protected.** E.1.1: a de-identifier "shall always protect … the SOP Instance UID
(0008,0018) Attribute as well as all references to other SOP Instances, whether contained in the main Data Set
or embedded in an Item of a Sequence of Items". E.1-1 has 55 UI-VR rows: 54 `U`, which are replaced, and
Annotation Group UID (006A,0003) as `D`, which §2 resolves to **remove**. PS3.6 Table 6-1 has **92** UI-VR attributes
(10 retired). **39** of them are not in E.1-1, and Basic, read literally, would keep them. They include Multi-frame
Source SOP Instance UID (0008,1167), Radiopharmaceutical Administration Event UID (0008,3012), which every PET
acquisition carries, and the Volume/Table Frame of Reference UIDs (0020,9312)/(0020,9313). The generator splits those 39
with one keyword rule. The attribute is **kept** only if its PS3.6 keyword matches

```text
SOPClass|TransferSyntaxUID$|^CodingSchemeUID$|^ContextUID$|^MappingResourceUID$
```

That rule matches attributes that name a standard-defined class, syntax or coding resource and cannot
identify a patient. Every other unlisted UI attribute is **replaced** by the §4 derivation. The lists below were
generated on 2026-10-18 by the probe in the appendix, run over the two 2026d files whose digests §2 records.
P1 moves the same rule into `scripts/dicom-deid-tables.py`. The output is:

- **keep (16):** (0008,0016) SOPClassUID · (0008,001A) RelatedGeneralSOPClassUID · (0008,001B)
  OriginalSpecializedSOPClassUID · (0008,0062) SOPClassesInStudy · (0008,010C) CodingSchemeUID · (0008,0117)
  ContextUID · (0008,0118) MappingResourceUID · (0008,040E) StoredInstanceTransferSyntaxUID · (0008,1150)
  ReferencedSOPClassUID · (0008,115A) SOPClassesSupported · (0008,3002) AvailableTransferSyntaxUID · (0034,0003)
  FlowTransferSyntaxUID · (0400,0010) MACCalculationTransferSyntaxUID · (0400,0510)
  EncryptedContentTransferSyntaxUID · (3010,0052) PertinentSOPClassesInStudy · (3010,0053)
  PertinentSOPClassesInSeries.
- **replace (23):** (0008,010D) ContextGroupExtensionCreatorUID · (0008,1167) MultiFrameSourceSOPInstanceUID ·
  (0008,3012) RadiopharmaceuticalAdministrationEventUID · (0008,9123) CreatorVersionUID · (0018,991E)
  TargetFrameOfReferenceUID · (0020,0242) SOPInstanceUIDOfConcatenationSource · (0020,9312)
  VolumeFrameOfReferenceUID · (0020,9313) TableFrameOfReferenceUID · (0028,0304) ReferencedColorPaletteInstanceUID
  · (0040,A021) FindingsGroupUIDTrial (retired) · (0040,A022) ReferencedFindingsGroupUIDTrial (retired) ·
  (0040,E011) RetrieveLocationUID · (0040,E030) RepositoryUniqueID · (0040,E031) HomeCommunityID · (0044,0102)
  AssertionUID · (0044,0108) ReferencedAssertionUID · (0068,7004) ModelGroupUID · (0070,031B)
  ReferencedFiducialUID · (0070,1209) VolumetricPresentationInputSetUID · (0070,1904) ReferencedContentItem ·
  (0072,007F) SelectorUIValue · (300A,0675) EquipmentFrameOfReferenceUID · (3010,001F) SegmentationTemplateUID.

**A value guard applies on top of the attribute rule.** A UI value under the DICOM root `1.2.840.10008.` is never
replaced, wherever it appears. Such a value is a standard-defined class, syntax, frame or well-known instance,
such as a standard color palette, so it cannot identify a patient. Replacing it would only break meaning. The
guard is value-based, so it is exact. UI values inside retained Safe Private elements are covered too: E.3.10-1
has 7 UI rows, for example NQHeader (0099,xx02), and they are replaced by the same map (§3). Retain UIDs turns
`replace` into `keep` for the generated set as well as for the `U` rows, so the option means one thing.

**R4 — every DA/DT/TM value follows the date rule.** PS3.6 has **15** public DA/DT/TM attributes that E.1-1 does
not list, generated by the same script: DA (0014,0102) (0014,1020) (0014,3076) (0014,4076) (0014,4078)
(0014,407A) (all DICONDE); DT (0008,0404) ItemInventoryDateTime, (0008,0416) ExpirationDateTime, (0008,041F)
StudyUpdateDateTime, (4010,1025) (4010,1026) (4010,102B) (4010,1041) (DICOS); TM (0014,0103) (0014,3077)
(DICONDE). Under Basic, a literal reading keeps them. Under Modified Dates they would stay **unshifted** next to
shifted ones, and one known pair reveals Δ. E.3.6 scopes its option to "the Attributes listed in" the table.
Tessera is stricter on purpose and treats these 15 exactly like a listed date row (§5). So the invariant is
VR-keyed, not list-keyed. Under Basic, no DA/DT/TM element survives anywhere in the sealed header. Under
Modified Dates, every surviving DA/DT value is shifted.

**R5 — VRs for every table row.** The fixture (§11) needs a VR for every E.1-1 row. Six rows have none in Table
6-1. (0002,0003) and (0004,1511) resolve from Tables 7-1 and 8-1. (0000,1000) and (0000,1001) are command-group
elements (PS3.7, UI). The generator declares them with that citation, and R1 removes them anyway. The two
pattern rows are expanded into concrete member tags, each with the VR that Table 6-1 gives its pattern.

**R6 — dates held in text VRs.** PS3.6 defines two public attributes that hold a date as text and are not in
E.1-1: (0010,0033) Patient's Birth Date in Alternative Calendar and (0010,0034) Patient's Death Date in
Alternative Calendar, both LO (PS3.6 2026d Table 6-1). The round-2 audit sealed (0010,0033) unchanged under every
mode. The generator finds them with one rule: a public attribute not in E.1-1, whose VR is a text VR (LO SH ST LT
UT UC), and whose keyword contains `Date`. In 2026d that rule matches exactly these two. A wider scan over every
text-class VR with `Date|Time` in the keyword matches 56 attributes, and the other 54 are durations, counters,
flags or names (for example (0018,0080) RepetitionTime DS, (0008,2132) EventTimerNames LO), not calendar dates.
R6 attributes are **date-valued**. They are kept under Full Dates and **removed** under Basic and under Modified
Dates, because free text in an arbitrary calendar cannot be shifted reliably. The calendar name (0010,0035) is CS
and stays.

**R7 — URI attributes.** E.1.1's "all references to other SOP Instances" includes references written as URLs:
the round-2 audit sealed (0008,1190) RetrieveURL `https://pacs.invalid/studies/1.2.826…` verbatim. PS3.6 2026d has
**15** UR attributes. (0010,0047), (0040,E012) and (0072,0071) are in E.1-1 (`X`, `X`, `D`, all removed). Of the
**12** that are not, the generator **keeps** only those whose keyword matches `^CodingSchemeURL$|^URNCodeValue$`.
Both name coding resources, which E.1.1 treats as safe ("coded sequence entries … do not contain identifying
information"). It **removes** the other 10: (0008,0407) StoredInstanceBaseURI · (0008,0408) FolderAccessURI ·
(0008,0409) FileAccessURI · (0008,040B) FilenameInContainer · (0008,1190) RetrieveURL · (0028,7FE0)
PixelDataProviderURL · (0040,4073) StorageURL · (0040,E010) RetrieveURI · (0068,7005)
RelativeURIReferenceWithinEncapsulatedDocument · (0074,100A) ContactURI. No UT or LT attribute in 2026d has a
keyword naming a URI or URL. The scan for `UR[IL]` in non-UR keywords finds only (0008,010A) CodingSchemeURLType
(CS, an enumeration) and (0074,1008) ProcedureStepCommunicationsURISequence (SQ, whose UR items R7 removes).
**Remove, not rewrite:** rewriting would mean parsing every URL scheme and path layout (DICOMweb, WADO, XDS,
vendor) to find embedded UIDs, and a URL also carries a host and path (institution identity, sometimes names)
that a rewrite would keep. A retrieve location is also meaningless inside a sealed, offline product. URLs in
free text remain a stated residual (§Consequences).

**R8 — no UN element is ever sealed.** An implicit-VR file read without a dictionary entry yields UN. A private SQ
then arrives as raw bytes, so a check for "absent by tag" cannot see inside it (round-2 audit). The rule:

1. a public UN element: R2 removes it if the tag is unknown, otherwise it is re-typed to its PS3.6 VR;
2. a private UN element under Basic: removed with all private tags;
3. a private UN element under Retain Safe Private: if its (creator, element) is in E.3.10-1 with a concrete VR,
   the bytes are re-parsed as that VR. An SQ is parsed as Implicit VR Little Endian: PS3.5 2026d §6.2.2 says an
   application that knows the actual VR of a UN element "can assume that the Value Field … is encoded in Little
   Endian byte ordering with Implicit VR encoding". It is then processed recursively like any other sequence.
   Elements inside a re-parsed item are themselves UN (no dictionary is consulted for private tags in Implicit
   VR), so each one goes back through rules 1-3. Its private creator is the (gggg,00xx) creator element found
   **inside that same item**. PS3.5 2026d §7.8.1: "Items within a sequence are self contained Data Sets …, any Item
   in the sequence that contains Private Data Elements shall also have Private Creator Data Element reserving a
   block".
   An item element whose creator is missing from the item is removed. E.3.10-1's `OW/OB` rows ((2001,xx7d), (2001,xxff))
   are typed OB. Its 5 rows with no VR ((0119,xx11), (0119,xx12), (0129,xx03-05)) and every pair not in E.3.10-1 are
   removed;
4. after the profile runs, `apply` checks that no UN remains at any depth, and **fails** the ingest if one does
   (an internal invariant, not a warning).

So UI values in re-typed Safe Private elements reach R3 as UI and are replaced. That is how replacement inside
formerly-UN bytes is defined (N6).

**R9 — person and organization names** (round-3 audit B2). Two parts:

1. **every PN element is removed**, public or private, at any depth, unless an applied option marks that
   attribute `K`. Generated: PS3.6 2026d Table 6-1 has 32 PN attributes. The 29 in E.1-1 all have a non-`K` Basic
   action, and **none has any option cell** (computed over all ten option columns), so in 2026d no option keeps
   a PN at all. The rule newly covers the **3** PN attributes E.1-1 does not list, which the audit sealed in both
   explicit- and implicit-VR runs: (0014,0104) SecondaryReviewerName and (0014,2006) EvaluatorName (DICONDE), and
   (0040,A067) DocumentAuthorTrial (retired). E.3.10-1 has no PN row, so under Safe Private a PN is removed too.
2. **person and organization names in text VRs** (LO SH ST LT UT UC), not in E.1-1. These are found in two
   recorded passes, and the candidates are classified by one criterion: *does the value name a person, or an
   organization involved in this subject's care or in creating this data?* Pass A takes PS3.6 keywords containing
   `Name`: 59 candidates. Pass B takes PS3.3 2026d module-table descriptions matching pattern B (below): 23
   candidates. **Removed (6):** (0074,100C) ContactDisplayName ("Name of the person, department or
   organization to contact", left behind when R7 removes ContactURI) · (0072,0008) HangingProtocolCreator ·
   (0072,0010) HangingProtocolUserGroupName ("Group or site …") · (0076,0003) ImplantAssemblyTemplateIssuer ·
   (0078,0020) ImplantTemplateGroupIssuer ("Person or Organization …") · (2100,0160) OwnerID ("owner of the film
   session"). **Kept, with the reason recorded:** (0008,0116) CodingSchemeResponsibleOrganization (a coding
   authority); (0010,0217) StrainSource (a strain supplier, shared by many animals); the four private-creator
   reference attributes (0008,0302), (0020,9213), (0020,9238), (0072,0054)/(0072,0056) (vendor names); and the
   comment fields among the pass-B hits, which R11 removes. All 59 pass-A hits other than (0074,100C) and
   (0072,0010) name an object, not a person: models, coils, protocols, algorithms, fonts, beams, wedges, printers,
   machines.

**R10 — identifiers copied into text VRs** (round-3 audit B1). An instance or document identifier written into
an ST/LT/UT/LO/SH/UC value escapes R3, which acts on the UI VR. The audit's case: (0040,E001)
HL7InstanceIdentifier holds "a UID (OID or UUID), concatenated with a caret" (PS3.3 2026d), next to the
ReferencedSOPInstanceUID of the same (0040,A390) item, so the simulated seal kept `…99.777^PHIEXT` beside the
replacement. **How the class was searched (so it is closed, not patched):** every PS3.3 2026d module-table row
was parsed (4371 tags with descriptions) and joined with its PS3.6 VR. Two description searches were run over
the text-VR attributes not in E.1-1: pattern C (7 hits), then the broad pattern D (91 hits). The patterns, all
case-insensitive:

```text
B  \b(person|persons|individual|organi[sz]ation|institution|operator|
     physician|user group|owner|creator|author|reviewer|contact)\b
C  \bUID\b|Unique Identifier|Instance (UID|Identifier)|
     Document (ID|Identifier)|HL7|XDS|OID|Repository
D  identif|\bID\b
```

(Each pattern is one line in the generator; the breaks here are for width.) Each hit was classified by one
criterion: *can the value identify, or copy the identifier of, a specific SOP instance, document, subject, person
or issuing organization?* **Removed (10 tags):** (0040,A390) HL7StructuredDocumentReferenceSequence (whole) and
(0040,E001) HL7InstanceIdentifier (in every context: it also occurs in encapsulated-document references) ·
(0040,0031) LocalNamespaceEntityID and (0040,0032) UniversalEntityID (issuing-authority identifiers) ·
(0062,0020) TrackingID (a cross-report tracking label; its UI twin (0062,0021) TrackingUID is `U` in E.1-1) ·
(0088,0130) StorageMediaFileSetID (a human-readable media label) · (0054,0400) ImageID ("user or equipment
generated Image identifier") · (0044,0001) ProductPackageIdentifier (a package bar code, linkable to dispensing
records) · **(0010,2294) BreedRegistrationSequence with (0010,2295) BreedRegistrationNumber**: "Identification
number of a non-human organism within the registry", a direct identifier of the animal itself, which matters
for the veterinary case and which E.1-1 does not list. **Kept, by class, with each tag's class recorded in the
TSV:** codes and coding schemes (CodeValue, LongCodeValue, CodingSchemeDesignator/Version/Registry/ExternalID);
private-creator attributes; device, accessory and equipment identifiers (DeviceID, GridID, WedgeID, BolusID,
CompensatorID, SnoutID, …: PS3.15 lists the device identifiers it protects, and Retain Device Identity governs
those); labels unique only within the instance (StackID, OpticalPathIdentifier, FiducialIdentifier, SegmentLabel,
LUTLabel, TextureLabel, GraphicGroupLabel, AssessmentLabel/AssessmentSetID, TrackSetLabel, WorklistLabel,
FilmSessionLabel); software and algorithm versions; product, lot and part numbers shared by many subjects
(ProductLotIdentifier, ImplantPartNumber); strain-level identifiers (StrainStockNumber, StrainSource). Free-text
*descriptions* among the hits, such as ContentDescription (0070,0081) and LensDescription (0046,0012), are Clean
Descriptors territory, so they stay a stated residual (§Consequences).

**R11 — unlisted free-text comment fields.** E.1-1 lists 23 comment/notes attributes (keyword matching
`Comment|Notes?$|Remark`). It gives all 23 a remove action (`X` 20, `Z` 3) and no option keeps any of them, so
the standard treats operator comments as identifying. PS3.6 2026d has **23 more** text-VR attributes with such a
keyword that E.1-1 does not list (generated), for example (0040,0400) CommentsOnTheScheduledProcedureStep,
(0044,0011) SubstanceAdministrationNotes, (0024,0044) CommentsOnPatientPerformanceOfVisualField and (3004,0006)
DoseComment. The search for R9 surfaced them, and R11 removes them all in every mode, like their listed
siblings.

R1-R11 are the whole list of hand-written policy, and each sits in the generator with its citation. R1-R8 and
R11 are mechanical rules over PS3.6. R9 part 2 and R10 are mechanical **searches** whose candidates are then
classified by hand against the stated criterion, with the class of every candidate recorded in the vendored TSV.
A reviewer can therefore audit each decision, and an edition bump re-runs the searches and shows the new
candidates.

**Edition drift.** Two checks:

- *hermetic* (in the flake): the embedded TSVs parse, every action cell is one of the measured codes, the row
  counts match the header (657 / 479 / 5268 / 22 / 19; the §2.1 lists R3 16 / 23, R4 15, R6 2, R7 2 / 10, R9 3
  PN + 6 text, R10 10, R11 23; the §3.0 override set 11), and the header's source
  sha256 equals the one this ADR records. A hand edit to a TSV fails, because the body digest no longer matches.
- *scheduled* (non-hermetic, weekly CI, report-only): fetch `current`, re-run the generator, and open an issue
  when the edition, a table or a generated list changes. Moving to a new edition is a deliberate PR. It changes
  the record's `edition` and `table_digest`, and therefore the `manifest_hash` of later de-identified ingests, as
  it should.

**Rejected: the `dicom-anonymization` crate** (crates.io 0.3.1, carecoders). Its README documents `HashUID`
("generates new UIDs from hashes") and `HashDate` ("shifts dates based on hash of the `PatientID`"). Those are
unkeyed hashes, and PS3.15 E.1.1 warns about exactly them: "unencrypted hashes are considerably less secure and
should be avoided, since they are vulnerable to trivial dictionary based attacks". The crate also records no
edition provenance, and it would be a new dependency, which trips `feature-snapshots`.

**Rejected: an allowlist-only seal as the default.** ADR-0047 proposed this. It is offered as `header = "none"`
(§6) but is not the default. Annex E plus §2.1 is a field-accepted, auditable line. An allowlist is a
Tessera-private judgement that no reader can check against a standard, and it would drop the acquisition
parameters (reconstruction, dose, radiopharmaceutical) that make a DICOM-derived product reusable.

## §3 — Options: which are supported, the default, and how they are selected

| Option (PS3.15 E.3) | CID 7050 code | Supported | Notes |
|---|---|---|---|
| Basic Profile | DCM 113100 | **always** | the base of every de-identifying ingest |
| Retain Safe Private | DCM 113111 | yes | only E.3.10-1 (creator, element) pairs; their UI values replaced (§2.1 R3) |
| Retain UIDs | DCM 113110 | yes | `U` rows **and** the R3 replace set kept; originals sealed; explicit only |
| Retain Device Identity | DCM 113109 | yes | `K` rows kept; its 11 `C` rows (AE titles etc.) keep the Basic action |
| Retain Institution Identity | DCM 113112 | yes | 10 `K` rows |
| Retain Patient Characteristics | DCM 113108 | yes | 14 `K` rows (sex, age, weight, size, …); 5 `C` rows stay Basic |
| Retain Longitudinal Full Dates | DCM 113106 | yes | its 169 `K` rows + R4 15 + R6 2 kept (§5); excl. Modified |
| Retain Longitudinal Modified Dates | DCM 113107 | yes | 169 `C` rows + R4 15: DA/DT shifted, TM kept, rest gone |
| Clean Descriptors | DCM 113105 | **no** | needs a vetted text-cleaning dictionary |
| Clean Structured Content | DCM 113104 | **no** | SR content-item cleaning |
| Clean Graphics | DCM 113103 | **no** | overlays and curves are removed (Basic) |
| Clean Pixel Data / Recognizable Visual Features | 113101 / 113102 | **no** | §8 |

The codes and meanings were checked against PS3.16 2026d, CID 7050 ("De-identification Method", version
20170914). The longest code meaning, for 113107, is 62 characters, which fits LO.

**Where an option cell is `C`, Tessera keeps the Basic action (remove)**, except for Modified Dates, whose `C`
means shift (§5). For example, Retain Device Identity cleans Station AE Title (0008,0055). Removing it is stricter
than cleaning, so claiming the option stays truthful. This is documented beside the option list in the book.

### §3.0 — Option precedence

174 E.1-1 rows carry a cell in more than one option column (generated): 158 are Full `K` + Modified `C`; 11 are
Device Identity `K` + Full `K` + Modified `C`; 3 are Clean Descriptors `C` + Patient Characteristics `C`; and 2
are Device Identity `K` + UIDs `K`. The resolution of one row, in order:

1. **date-valued rows are governed by the date mode alone.** A row is date-valued if its VR is DA, DT or TM, if it
   is a Full Dates `K` row (which adds the 2 OB timestamps (0400,0310) and (0034,0007) and the SH
   TimezoneOffsetFromUTC (0008,0201)), or if it is an R4 or R6 attribute. Under Basic it takes the Basic action
   (remove). Under Full Dates it is kept if Full marks it `K` or it is R4/R6, and otherwise takes its Basic action.
   Under Modified Dates it is shifted if it is a DA/DT that Modified marks `C` or that R4 lists, and kept if it is
   a TM that Modified marks `C` or that R4 lists. Anything else is removed: the OB timestamps, the SH offset, R6
   text, and the unmarked date-VR rows such as (0010,0032) PatientBirthTime. **No non-date option's `K` applies to a
   date-valued row**;
2. otherwise, `K` if any active option marks the row `K`;
3. otherwise, the Basic action (an unsupported or non-date `C` resolves to it).

Rule 1 overrides Retain Device Identity's `K` on 11 device dates, including (300C,0127) Beam Hold Transition
DateTime (a treatment time, so a subject date), (0018,1200) Date of Last Calibration and (0018,1204) Date of
Manufacture. Without a date option they are removed. Under Modified Dates they are shifted, and under Full Dates
they are kept. This is stricter than E.3.8 read alone, which would retain them, so the attestation says so: when
Device Identity is applied without Full Dates, (0012,0063) carries the extra value `Device dates follow the
date option` (39 characters), and the record lists the 11 rows under `overridden`. Three E.1-1 date-VR rows have
no Full/Modified cell at all, (0010,0030) PatientBirthDate, (0010,0032) PatientBirthTime and (0016,0077) GPS Time
Stamp, so they are removed under every mode. This is the table's intent: age, not birth date, is what Patient
Characteristics retains.

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
```

`key_file` is a path, never an inline secret. Two keys sit on the **product** next to `deidentify`, not in the
profile table, because they land in earlier phases (P3a, P3b) than the table does (P4b):

```toml
pixel_review = "acknowledged"             # §8; CLI --deid-pixel-review acknowledged
pseudonym_fields = ["patient_pseudonym"]  # §9.1; CLI --pseudonym-field <FIELD>
```

**How the CLI declares a pseudonym.** `patient_pseudonym` is `Identifying` (`schema.rs:535-541`), so a plain
`--meta patient_pseudonym=P-0042` on a de-identifying ingest is refused by the §9.1 gate. The operator passes it
together with `--pseudonym-field patient_pseudonym` (repeatable). That declares the value operator-issued and
lands it in the record's `pseudonym_fields`. Declaring a field that no `--meta` supplies, or a field the schema
does not mark `Identifying`, is a hard error. That keeps the flag from turning into a blanket waiver.

If the table is present, or `recipients` is non-empty, de-identification is on. An unknown option name,
`also_remove` on an `X` tag (a no-op) and the two exclusive date options together are each a **hard error**.
The CLI mirrors the spec: `--deid-option <name>` (repeatable), `--deid-key <file>`, `--deid-also-remove
<GGGG,EEEE>` (repeatable), and `--deid-header none`. `tessera keygen --deid OUT`
writes a 32-byte random custodian key (hex, mode 0600), on the same model as `keygen --age`.

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

### §4.1 — Coverage and derivation

The protected UI values are those of the **54 `U` rows** of E.1-1 (Annotation Group UID (006A,0003) is `D` and
is removed, §2.1 R3), the 23 R3 `replace` attributes, and the 7 E.3.10-1 UI rows under Retain Safe Private
(re-typed from UN where needed, R8). That applies at any depth, on every slice, minus the DICOM-root guard. The
UIDs inside the two `X/Z/U*` sequences are removed with their sequence. UIDs embedded in URLs are removed with
their attribute (R7). Each value is replaced, byte-exactly:

```text
u    = the value as ASCII, trailing NUL/space padding removed
h    = BLAKE3 keyed hash, key K (32 bytes),
       input "tessera/dicom-deid/uid/v1" ‖ u; first 16 bytes
h[6] = (h[6] & 0x0F) | 0x80        # UUID version 8
h[8] = (h[8] & 0x3F) | 0x80        # RFC 9562 variant
uid' = "2.25." + decimal(h as a 128-bit big-endian integer)
```

`2.25.` is the PS3.5 B.2 UUID-derived root, and the result is a valid UID of at most 44 characters. **Test
vector**, with K = bytes `00 01 02 … 1f`: `1.2.826.0.1.3680043.99.4242.100` → raw `h[0..16]` =
`7b899870689bb727963b2f3185ad639c` → after the version/variant mask (byte 6 `b7`→`87`; byte 8 `96` already has
variant bits `10`) `7b899870689b8727963b2f3185ad639c` → `2.25.164209479983352343176970855835994252188`. It was computed
with the `blake3` Python binding, which wraps the reference Rust crate and gives `6437b3ac…5bd9d85` for unkeyed `abc`,
the published value. P2c pins it as a unit test. Multi-valued UI elements are mapped per value. The same `uid` maps to
the same `uid'` everywhere: top level, nested items, every slice, public or private. That keeps referential integrity
across a series, and keeps a shared FrameOfReferenceUID shared between PET and CT.

### §4.2 — The key K, and why it is never derivable from the seal

- *Per run (default).* K is BLAKE3 `derive_key` with the context `tessera 2026-10 dicom-deid per-run key v1`
  over the raw source bytes of every DICOM member of the spec run: for each member in spec order, and each
  input file in its listed order, `u64le(len) ‖ bytes`. It is
  computed in the same streaming pass as `source_digest`. The context string differs from the sealed
  `ingested_from.content_hash`, so the seal reveals nothing about K. Computing K requires **the original files**,
  and a holder of those can already link the product through the bit-identical pixels. Products in one spec run
  share a keyspace, so PET and CT in one collection stay consistent. Separate runs get unlinkable UIDs.
- *Custodian.* K is the 32 bytes of `key_file`. Every run with the same key gets the same mapping, which gives
  longitudinal linkage across products for whoever holds the key. The record carries `key_id`, the first 8 bytes
  in hex of BLAKE3 `derive_key` with the context `tessera dicom-deid key id v1` over K. That reveals only what
  equal replacement UIDs already reveal: that the products share a keyspace.

Determinism (S15): the same inputs, options and key give the same bytes. There is no randomness anywhere.

**Rejected:** random UIDs, because they break S15 and the content-addressed identity. An unkeyed hash of the
UID, because of the dictionary/confirmation attack above. A key derived from sealed values (the spec hash or the
source merkle root), because that is an unkeyed hash with extra steps.

**Where the mapping lives.** `original → replacement` for every replaced UID on every slice goes into the
envelope (§6). Without recipients, the map is not stored. A custodian with `key_file` can recompute the
replacement of a **known** original UID, but can never invert it. Without a custodian key, nobody can, short of
holding the originals.

### §4.3 — Custodian keyspace versus crypto-shred erasure

ADR-0047 §4 claims that destroying the recipient key "erases [identifying material] from every copy". That
holds for the **per-run** keyspace: once the envelope and the age key are gone, linking a replacement UID or
recovering Δ needs the original files. It does **not** hold for a **custodian** keyspace. Whoever holds K can
still confirm any known original UID against its replacement, and can recompute Δ from a known PatientID (§5).
That is the linkage the key exists for, and per-subject destruction inside one key is impossible. So:

- a custodian keyspace is recorded (`keyspace: "custodian"`, `key_id`) and printed by `inspect`, including in
  `inspect --json` (the round-2 reviewer's ask);
- `tessera shred` on such a product succeeds, but warns that erasure also requires destroying the custodian key,
  which affects every product in that keyspace;
- ADR-0047 §4's claim is amended to say "in the per-run keyspace" (the P5 banner);
- whether a custodian key may be combined with crypto-shred at all is owner decision D6. Per-subject custodian
  keys, which would make per-subject erasure possible, are a follow-up.

## §5 — Dates and times

The date-valued set (§3.0 rule 1, regenerated): the 169 E.1-1 rows with a DA/DT/TM VR (DA 55, DT 61, TM 53), the
3 non-date-VR Full `K` rows (2 OB, 1 SH), the 15 R4 attributes and the 2 R6 attributes. The Full and Modified
columns mark the same 169 rows: 166 of the date-VR rows plus the 3 non-date ones. The 3 date-VR rows they do not
mark (birth date, birth time, GPS time stamp) are always removed.

| Mode | the date-valued set | (0028,0303) | sealed `study_date` |
|---|---|---|---|
| Basic (default) | all removed | `REMOVED` (E.2 requires adding it) | **absent** |
| Retain Full Dates | 166 + 3 + 15 + 2 kept; the 3 unmarked removed | `UNMODIFIED` | original |
| Retain Modified Dates | DA/DT shifted by −Δ days; TM kept; OB, SH, R6, unmarked removed | `MODIFIED` | shifted |

**The subject key.** Δ is keyed on the subject, not on PatientID alone:

```text
i, p = IssuerOfPatientID, PatientID: the original values, decoded per
       SpecificCharacterSet, leading/trailing spaces and NULs removed,
       UTF-8 encoded; an absent issuer is the empty string
s    = u32be(len(i)) ‖ i ‖ u32be(len(p)) ‖ p
h    = BLAKE3 keyed hash, key K (32 bytes),
       input "tessera/dicom-deid/date-shift/v2" ‖ s
n    = u64 from h[0..8], little endian
Δ    = 1 + (n mod 3652)                # days
```

That gives a shift between 1 and 3652 days (about 10 years) and never 0. The modulo bias is below 2⁻⁵⁰. **Test
vectors**, with K = bytes `00 01 02 … 1f` (same binding as §4.1): (issuer `""`, PatientID `PID-FICTUS-42`) →
`h[0..8]` = `31f956695789ce57` → Δ = **814**; (issuer `FICTUSHOSP`, same PatientID) → `4c8b4c2386e641e1` → Δ =
**2733**. P2c pins both. Values are read before the profile runs. Modified Dates **refuses** when (0010,0020) PatientID
is absent or empty on any slice, or differs between the slices of a product. If it did not, all unidentified subjects
would collapse onto one Δ, and one known date would then reveal every subject's dates. Products of one run share Δ per
subject, and a custodian key extends that across runs.

**The shift.** A DA becomes the date Δ days earlier (proleptic Gregorian). A DT has its date part shifted the
same way, keeps its time part and precision, and loses any UTC-offset suffix. Values that cannot be shifted exactly (a
DT of year or year-month precision, a malformed DA) are removed and counted. Shifting by whole days keeps time-of-day
and every interval inside the subject exactly. It also keeps SUV decay correction, which subtracts times.
TimezoneOffsetFromUTC (0008,0201) keeps its Basic action, because its `C` cell resolves to remove. Δ itself goes into
the envelope.

**Schema fields.** `study_date` is `Sensitive` (`schema.rs:569-575`) and follows the table above. Any field the
schema marks `Identifying` is subject to the §9.1 gate. The §9.3 gate covers the operator's sealed
`Manifest.timestamp`.

**Coarsening (corrected).** Round 1 rejected coarsening as "not a PS3.15 option". That was wrong. E.3.6 lets
Modified Dates "aggregate[] or transform[] dates", for example "setting all dates to the first day of the month",
and coarsening would attest as 113107. It is still **not offered in this landing**, for utility rather than
conformance reasons. It destroys the intra-subject intervals, which SUV decay correction and longitudinal
response measurement need, and a shift keeps them while matching protection. It can be added later as a
`date_method = "aggregate-month"` parameter of the same option, and would need its own record field.

## §6 — Sealed vs envelope

| Material | Sealed (`manifest_hash`) | Envelope (`aux/identity/identity.age`, recipients only) |
|---|---|---|
| De-identified header of the first slice (`extra["dicom_header"]`) | yes, or no with `header = "none"` | — |
| Curated non-identifying metadata (modality, rescale, kVp, geometry, manufacturer, model) | yes | — |
| Replaced UIDs in curated metadata (`study_instance_uid`, …) | yes (pseudonyms, §9) | originals |
| `study_date` | per §5 | original |
| Original values of **every attribute the profile acted on, per slice** | never | yes |
| UID map, date offset Δ, original source paths, original spec path | never | yes |
| The `dicom_deidentify` record (§7) | yes | — |

**Before P4 (envelope v1).** P3a already moves the source and spec paths out of the seal (§9.2, §9.5). So P3a adds
two optional, additive fields to the v1 document: `source_references` and `spec_reference`, behind
`#[serde(default, skip_serializing_if = …)]`. `IdentityDocument` sets no `deny_unknown_fields`
(`identity.rs:30-41`), so an older reader ignores them, and `v` stays 1. The paths therefore go into the envelope
from the first phase that removes them from the seal. They are dropped only when there are no recipients,
exactly like everything else (below).

**Identity document v2.** The new shape is `{ v: 2, slices: [{ modified_attributes: { "GGGG,EEEE": {vr, value} } }],
uid_map, date_shift_days, source_references, spec_reference, curated_identifying }`. `modified_attributes` holds
the original of every element whose action was not `K`, at its full sequence path. This is the content PS3.15
puts in the Modified Attributes Sequence (0400,0550) of an Encrypted Attributes Data Set. Tessera does not use
DICOM's RSA/AES encoding for it, because the sealed product is not a DICOM instance, and it keeps ADR-0047's
`age` X25519 envelope, which sits outside the seal for determinism. Storing the touched attributes only, instead
of N full headers, bounds the envelope to the identifying content. v1 envelopes still decrypt: `reidentify`
reads both versions.

**Without recipients** (`--deidentify` alone) there is no envelope. The originals, the UID map and Δ are
**dropped**, and that is the documented meaning of destructive de-identification. A custodian key does not
change this. It only makes forward mapping reproducible (§4.3).

**The default flips in P3d (owner decision D1).** From P3d, a DICOM ingest with no flag runs the Basic profile.
Keeping identity requires the explicit `--keep-identity` CLI flag or the spec key `keep_identity = true`, which
is mutually exclusive with `deidentify`, `recipients` and `[product.deidentify_profile]` (a hard error). That path
keeps today's behaviour: it warns, seals no header, and seals the curated fields in the clear. Until P3d lands,
the keep-PHI path stays the default and the book says so.

## §7 — Attestation: what the product says about itself

In the sealed header (unless `header = "none"`):

- PatientIdentityRemoved (0012,0062) = `YES`. E.2 states that the Basic Profile "does not address information
  in the pixels" unless a Clean Pixel option is applied, so `YES` is a claim about attributes only, and the
  book says so;
- DeidentificationMethod (0012,0063) is LO with VM 1-n (PS3.6 6-1). It is **multi-valued**, with every value at
  most 64 characters. Value 1 is `PS3.15 2026d Basic Application Confidentiality Profile` (54 characters), and
  each further value is the CID 7050 meaning of one applied option, plus `Device dates follow the date option`
  when §3.0 overrode Device Identity. It carries **no software version**. The
  producing tool and version are already sealed in ADR-0058's `producer`, and a version here would move
  `manifest_hash` at every release without any privacy gain;
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
  "pseudonym_fields": ["…"],
  "pixel_review": "not-required | acknowledged",
  "overridden": ["…§3.0 rule-1 rows, when Device Identity runs without Full Dates…"],
  "spec_preimage": "deid-redacted-v1",
  "counts": {
    "removed": 0,
    "replaced_uids": 0,
    "shifted": 0,
    "unknown_public_removed": 0
  }
}
```

`table_digest` covers every vendored TSV, including the §2.1 generated lists. This is the machine-checkable
claim. `inspect` prints it, and the legacy detector (§10) looks for its absence. The counts are deterministic
for fixed inputs. **Rejected:** stating the claim only in (0012,0063), because free text cannot be checked, and
with `header = "none"` there is no header to put it in.

## §8 — Pixel data and burned-in annotation: out of scope, enforced

This ADR does not inspect or alter pixels. Clean Pixel Data and Clean Recognizable Visual Features are not
offered. A de-identifying ingest **refuses** when, on any slice:

1. BurnedInAnnotation (0028,0301) = `YES`, or RecognizableVisualFeatures (0028,0302) = `YES`. This cannot be
   overridden. Pixel de-identification is not implemented, and the error says so;
2. BurnedInAnnotation is **absent**, and Modality (0008,0060) is `US`, `OT`, `XC`, `DOC` or `SC`, or SOPClassUID is
   in the Secondary Capture family `1.2.840.10008.5.1.4.1.1.7*`. These sources routinely burn in text and
   routinely omit the flag (the round-1 audit). The operator can proceed only with
   `pixel_review = "acknowledged"`, which states that they inspected the pixels. It is sealed in the record
   (§7), and the attestation is unchanged, because Basic makes no pixel claim (§7).

Tessera never sets either flag itself, because setting one would be a claim about pixels. Overlay data
(60xx,3000) and curve data (50xx,xxxx) are removed by Basic, because they are header elements, not pixel data.
The modality list in rule 2 is policy, stated in the book, and extending it is an ordinary PR.

## §9 — Seal-time gates (fail-closed)

1. **Identifying fields.** On a de-identifying ingest, the current warning (`engine.rs:224-245`) becomes a
   **refusal**. The ingest stops if a field the schema marks `Identifying` is in sealed metadata, unless its
   value was produced by the profile (a replaced UID) or is listed in `pseudonym_fields` (an operator-issued
   pseudonym such as `patient_pseudonym`). Both kinds are listed in the record. This also covers the ADR-0058
   §5 inheritance: what a derived product inherits is the pseudonym.
2. **Opaque source reference, for every member of the run.** If any member of a spec run de-identifies, every
   member without a `source_label` gets `ingested_from.reference = urn:tessera:source:<content_hash>`, the
   ADR-0048 export form. That includes non-DICOM members (a NIfTI or table ingested next to the DICOM), because
   their paths sit in the same directory trees and are just as likely to carry a name (round-3 audit). For a
   DICOM member, the paths go into its envelope's `source_references` field, which P3a adds to the v1 document
   (§6), so they are never silently dropped while a recipient exists. A non-DICOM member has no envelope, so its
   path is held only by the operator's spec. The spec is bound to the product by the redacted spec hash (§9.5),
   whose pre-image carries that source's `content_hash`, so the operator can still re-link it.
   `content_hash` (the merkle root over the raw files) stays as ADR-0048 decided. Only a holder of the original
   bytes can confirm it, and that holder can already compare pixels.
3. **Timestamp.** Unless Full Dates is selected, the operator's `timestamp` must not fall on a calendar date
   that the profile removed or shifted (any DA/DT of §5, on any slice). The error suggests a non-identifying
   timestamp, or `timestamp = "from-source"`, which takes the (post-profile) StudyDate+StudyTime and so works
   under Modified Dates and fails under Basic. The CLI help for `--timestamp` ("Acquisition timestamp
   (ISO-8601), recorded verbatim", `main.rs:987-988`) is rewritten to say this.
4. **`name` and `study`** are operator text. The book says they must not carry identity. They cannot be
   checked mechanically.
5. **Opaque spec edge (§0 finding 8).** If any member of a spec run de-identifies, every member's
   `ingested_via_spec` edge changes. The reference becomes `urn:tessera:spec:<hash>` instead of the spec path,
   and the original path goes into the envelope's `spec_reference` field (P3a, §6). The hash is BLAKE3 over a **redacted
   pre-image**, tagged `tessera spec-hash deid-redacted v1`: the canonical spec with every `input`/`inputs` path
   replaced by that source's sealed `content_hash`, and `key_file` replaced by `key_id`. Every other field of that
   pre-image is either already sealed or operator text (name, timestamp, metadata, options, recipients). So guessing it
   gains nothing beyond what the seal already shows, and the dictionary attack on paths is gone. A holder of the spec
   and the sources can still recompute the hash (`tessera ingest --spec … --print-spec-hash`), so provenance stays
   verifiable. The redaction covers **every** member's `input`/`inputs` path in such a run, DICOM or not. Runs with
   no de-identifying member keep both the ADR-0035 pre-image and path references unchanged, so existing products
   and tests do not move. The CLI's synthesized inline spec (`reference = "cli-inline-spec"`) goes through the same
   redaction. This amends ADR-0035 (owner decision D7).

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

### §11.0 — The oracle: decode per VR, never grep

A byte grep finds text and misses everything rendered as numbers (§0 finding 9). The gate is a **decoding
oracle** over the sealed manifest. A raw byte scan is kept only as a second, weaker check.

**Oracle A (gate).** Parse `manifest.json`. Walk `extra["dicom_header"]` recursively: an object keyed `GGGG,EEEE`,
each value `{vr, value}`, and for `SQ` an array of nested objects. Decode each element with the inverse of
`value_to_json` (`dicom.rs:402-419`):

| VR class | VRs | decoded as |
|---|---|---|
| text | AE AS CS DA DS DT IS LO LT PN SH ST TM UC UI UR UT | the string values, verbatim |
| 8-bit binary | OB UN | each string parsed as `u8`, concatenated to bytes |
| 16-bit | OW US SS | each string as `u16` / `u16` / `i16`, to little-endian bytes |
| 32-bit | OL UL SL OF FL | `u32` / `u32` / `i32` / `f32` / `f32`, little-endian |
| 64-bit | OD FD OV UV SV | `f64` / `f64` / `u64` / `u64` / `i64`, little-endian |
| tag | AT | `(group, element)` pairs |
| sequence | SQ | recurse |
| pixel sequence | `{"fragments": n}` | no value (pixels are the volume block) |

**Any VR the oracle cannot decode fails the test**, so an unknown rendering can never pass silently. A unit test
round-trips `value_to_json` through the decoder for one element of every VR, which pins the oracle itself.
Oracle A asserts **four** things at every depth: (a) every seeded element whose resolved action is remove is
**absent by tag**, whatever its value; (b) no decoded value, as text or bytes, contains any seeded marker; (c)
every protected UI value is a `2.25.` replacement or under the DICOM root, and, under Modified Dates, every DA/DT
is shifted by Δ; (d) **no element has VR `UN`**. Check (d) closes the round-2 gap. An implicit-VR private SQ
sealed as UN bytes would hide its items from check (a). R8 forbids any UN in the seal, so (d) failing is the
signal, and every sequence that does reach the seal is structured and fully walked.

**Oracle B (secondary).** A raw scan of every container member except `aux/identity/identity.age`, and after
`shred` of every member, for the text markers and for each binary marker in both raw and decimal-rendered form.
The test also asserts the container is STORED.

**Markers, one per seeded element, valid for its VR and unique.** Strings are `PHI<gggg><eeee>`, cut to the VR's
maximum length and character set: CS upper-case, AE at most 16. DA is a unique date in 1901 (row index as day
offset). TM is a unique time. DT combines the two. DS/IS are `9000000 + row`. AS is a unique `nnnD`. UI is
`1.2.826.0.1.3680043.99.<row>`. UR is `https://phi.invalid/studies/1.2.826.0.1.3680043.99.<row>`, a URL carrying a
marker UID. R6 attributes get a marker date in LO text. Binary VRs get the 8 bytes `PHI\0` ‖
`u32le(row)`. Numeric VRs get a value unique to the row.

### §11.1 — The cases

1. **Every row, generated.** The fixture seeds one element per E.1-1 row (VRs per §2.1 R5; each pattern row as
   at least two concrete members; the private rule as a private block of explicit- and implicit-VR elements,
   including OB, PN and UI, and an implicit-VR private SQ holding a PN and a UI), plus: every R3 replace and keep
   attribute, every R4, R6, R7, R9, R10 and R11 attribute, an HL7 reference item whose (0040,E001) copies its
   ReferencedSOPInstanceUID, a nested copy of
   each inside a sequence two levels deep, group 0002/0004 elements in the dataset body, a File Meta
   SourceApplicationEntityTitle, and a PHI-bearing input and spec path. Ingest a 3-slice series through the
   engine three ways: `--deidentify`, crypto-shred, and crypto-shred then `shred`. Oracles A and B must pass. The
   test asserts **set equality** between the seeded ids and the vendored E.1-1 row ids (657) plus every generated
   list, so a shrunken fixture fails rather than passing vacuously. This case has two forms: object-level (P2c: `apply`,
   then `header_to_json`, then Oracle A, covering the header rows; the paths are covered by §11.1.7 in P3a) and end to
   end (P3b: through `engine::run`, Oracles A and B).
2. **Option matrix.** For each supported option alone, and for the PET recipe: the rows that option turns into
   `K` keep their marker, every other marker is absent, and (0012,0064) lists exactly the applied codes.
3. **UIDs.** Same input + same K → same bytes (S15). Per-run key differs from custodian key. One original UID on
   two slices, in a nested reference and in a Safe Private UI element → one replacement. A DICOM-root value is
   kept. Every R3 keep attribute is kept. PET+CT sharing FrameOfReferenceUID in one spec → still shared. The
   replacement is valid (`2.25.` prefix, at most 64 characters, UUIDv8 bits).
4. **Dates.** Basic → no DA/DT/TM element anywhere. Modified → every DA/DT shifted by the same Δ, including the
   R4 attributes, intervals preserved, Δ ≠ 0, Δ recoverable from the envelope. Two subjects with equal PatientID
   and different IssuerOfPatientID → different Δ. An absent PatientID → refused. The timestamp gate refuses a
   removed date.
5. **Gates.** BurnedInAnnotation `YES` → refused. Absent flag on `US` → refused; with `pixel_review` → accepted
   and recorded. An unregistered `Identifying` metadata field → refused. A declared pseudonym → allowed and
   recorded.
6. **Groups and file meta.** No group 0000/0002/0004 element and no File Meta value appears in the seal.
7. **Spec edge.** On a de-identifying run, the sealed `ingested_via_spec` reference is `urn:tessera:spec:…`. Its
   hash equals the redacted pre-image hash and differs from the ADR-0035 hash of the same spec. No spec or input
   path string is in the seal. A non-de-identifying run is unchanged.
8. **Attestation shape.** Every (0012,0063) value is at most 64 characters, and none contains a version string.
9. **Envelope v2.** `reidentify` returns the originals of every slice's touched attributes, the UID map, Δ and
   the paths. v1 envelopes still decrypt.
10. **Conformance-table drift** (§2), hermetic, including the §2.1 list counts.
11. **Legacy detector.** A product built the old way is flagged by `inspect`/`verify`.
12. **Precedence (§3.0).** Device Identity alone → the 11 device dates are absent, the extra (0012,0063) value and
    `overridden` are present. Device Identity + Modified → they are shifted by Δ. Device Identity + Full → they
    are kept. In no mode is an original subject date present next to a shifted one.
13. **R7 / R8.** A RetrieveURL carrying a marker UID is absent in every mode, and CodingSchemeURL is kept. Under
    Safe Private, a listed private SQ read as UN is re-typed, its UI replaced and its PN removed. An unlisted UN
    is removed. A UN injected after `apply` makes the invariant fail the ingest.
14. **Test vectors.** The §4.1 UID vector (raw and masked bytes) and the two §5 Δ vectors reproduce exactly.
15. **R9-R11.** Under every mode and option: every PN element is absent (including the 3 unlisted ones and a PN
    inside a nested item); the 6 R9 text names, the 10 R10 identifiers and the 23 R11 comment fields are absent;
    the replaced ReferencedSOPInstanceUID never sits next to its original in any text VR (the round-3 `^PHIEXT`
    case); a BreedRegistrationNumber is absent while species and breed descriptions stay; the kept classes
    (CodeValue, DeviceID, StackID) are present.
16. **Surface options.** `header = "none"` seals no `extra["dicom_header"]` but does seal the record; `also_remove
    = ["0010,2298"]` removes ResponsiblePersonRole and is recorded; `also_remove` on an `X` tag is a hard error;
    `timestamp = "from-source"` yields the shifted StudyDate+StudyTime under Modified Dates and is refused under
    Basic; `--pseudonym-field` on a field no `--meta` supplies is a hard error.

## §12 — Phasing

**Rules:** (1) no phase ships an attestation or a doc claim stronger than what that phase enforces; (2) every
phase's acceptance is satisfiable with that phase and earlier ones only, so no criterion names a later surface;
(3) each phase is about 400 hand-written lines or fewer (the generated TSVs are not counted). The line estimates
are mine, so the implementer splits a phase further if it comes out larger.

- **P1 — Vendored tables and generated policy** (~300). The generator (E.1-1, E.3.10-1, the PS3.6 6-1/7-1/8-1
  registry; the R3/R4/R6/R7/R11 lists, the R9/R10 searches with per-candidate classes, and the §3.0 override
  set; rules R1-R11 with their citations), the TSVs with provenance headers, a static parser, and the hermetic
  drift test. *Acceptance:* §11.1.10 green with every §2 count; source sha256 equal to §2. No behaviour change.
- **P2a — Engine skeleton** (~250, `tessera-ingest::dicom::deid`). `Profile`,
  `apply(obj) -> (obj', Record, Modified)`: compound-code resolution, recursion, private removal under Basic, R1
  groups, R2 unknown public, record counts. Pure, not wired. *Acceptance:* §11.1.6 at object level; a nested
  private and a nested group-0004 element are removed.
- **P2a-ii — UN handling** (~250). R8 rules 1-4: re-typing public UN, the Safe Private table lookup, the
  Implicit VR SQ re-parse with per-item creator lookup, and the no-UN invariant. *Acceptance:* §11.1.13 (R8 part)
  at object level.
- **P2b — Engine rules** (~300). §3.0 precedence; R4/R6 date-valued removal (Basic) and retention (Full); R7; R9;
  R10; R11. *Acceptance:* §11.1.12 (Basic and Full parts), §11.1.13 (R7 part) and §11.1.15 (Basic, Full and the
  non-keyed options) at object level.
- **P2b-ii — Attestation and the oracle** (~250). The attestation attributes (multi-valued (0012,0063), the
  §3.0 extra value, (0012,0064), (0028,0303)), the `value_to_json` inverse, and Oracle A with check (d).
  *Acceptance:* the oracle round-trip test for every VR; §11.1.8 at object level.
- **P2c — Keyed transforms** (~300). UID derivation over the full coverage with the DICOM-root guard; the per-run
  K derivation; the subject key, Δ and its refusals; the Modified Dates shift; Retain UIDs. Pure, not wired.
  *Acceptance:* §11.1.1 (object-level form), §11.1.2-3, §11.1.4 (all but its timestamp-gate sentence),
  §11.1.12 (Modified part), §11.1.14 and §11.1.15 (Modified part) at object level.
- **P3a — Provenance and pixel gates on today's path** (~350). The §9.5 redacted spec edge with
  `--print-spec-hash`; the §9.2 opaque source reference for every member of a de-identifying run; the two
  additive v1 envelope fields; the §8 refusals **with their surface** (`pixel_review` product key and
  `--deid-pixel-review`). All of it applies to the existing `--deidentify` / `--recipient` paths. It adds no
  attestation and only tightens. Can land in parallel with P1-P2c. *Acceptance:* §11.1.5 (pixel part) and
  §11.1.7 end to end; `reidentify` returns the paths from a v1 envelope; a mixed DICOM+NIfTI run seals no path.
- **P3b — Wire the Basic profile** (~350, after P2c and P3a). `engine.rs` dispatch to `deid::apply` (Basic,
  per-run keyspace, selected by `deidentify = true` / `recipients`), curated metadata from `obj'`, the record and
  attestation, the §9.1 gate **with its surface** (`pseudonym_fields` product key and `--pseudonym-field`), and
  the §9.3 gate. The attestation and every gate it relies on land together. *Acceptance:* §11.1.1 (end-to-end form,
  Basic), §11.1.4's timestamp-gate sentence, §11.1.5 and §11.1.8 through `engine::run` and the CLI; S15 re-ingest
  byte-identical.
- **P3c — Public surface catch-up** (~250, after P3b). Remove `PHI_TAGS`/`phi_present`/`deidentify` (a
  `feat(ingest)!` change); rewrite the CLI help (`--deidentify`, `--timestamp`); regenerate the affected trycmd
  snapshots; rewrite the book/FEATURE-MATRIX line to state that the v1 envelope holds slice 1's original header
  plus the paths only. *Acceptance:* trycmd snapshots green; `guardrails-gates` green.
- **P3d — Default flip (D1)** (~200, after P3c). No flag means Basic; `--keep-identity` / `keep_identity = true`
  (exclusive with every de-identification key) keeps today's keep-PHI path and its warning; the docs follow.
  *Acceptance:* a flagless ingest is de-identified (Oracle A); `--keep-identity` reproduces the pre-flip bytes; a
  combination with a de-identification key is a hard error.
- **P4 — Envelope v2** (~300, after P3b). `IdentityDocument` v2 (per-slice modified attributes, UID map, Δ when
  present, source and spec paths), and `reidentify` reading v1 and v2. It updates the slice-1 doc line to the
  full claim. *Acceptance:* §11.1.9 **under Basic**: per-slice originals, UID map and paths recovered. The Δ part
  of §11.1.9 moves to P4b, the first phase with a Modified Dates surface.
- **P4b — Option selection** (~300, after P4). The `[product.deidentify_profile]` table with `options`,
  `--deid-option`, and the exclusivity and unknown-option errors. *Acceptance:* §11.1.2, §11.1.4 and §11.1.12
  end to end, plus §11.1.9's Δ part (Δ read back from the v2 envelope).
- **P4c — Keyspace and header surface** (~300, after P4b). `key_file`, `--deid-key`, `keygen --deid`, the
  custodian keyspace with the §4.3 `shred` warning and `inspect --json` keyspace output; `header = "none"`;
  `also_remove`; `timestamp = "from-source"`. *Acceptance:* §11.1.3 (custodian part) and §11.1.16 end to end.
- **P5 — Legacy detection and docs** (~300, after P3d and P4c). The `inspect`/`verify` warning; the book
  (`ingest.md`, `provenance-signing.md`, a de-identification page with the option table, §2.1 R1-R11, §3.0, the
  PET recipe, the §8 modality list and the custodian-keyspace erasure caveat); the FEATURE-MATRIX rows; the
  security note; the ADR-0047 amendment banner (§1 step 4, §4) and an ADR-0035 note (§9.5); the scheduled
  edition-drift workflow. *Acceptance:* §11.1.11 against a synthetic old-style manifest built in the test
  ((0012,0062) = `YES`, no record), because P3c has removed the old code path; `guardrails-gates` green.

**Order:** P1 → P2a → P2a-ii → P2b → P2b-ii → P2c → P3b → P3c → P3d; P3b → P4 → P4b → P4c; {P3d, P4c} → P5.
P3a is independent and must land before P3b. Each acceptance list above cites only cases whose surface exists by
that phase: object-level cases use `deid::Profile` directly (P2*), CLI and spec cases start at P3a/P3b, options
at P4b, keyspace and header at P4c. **Follow-up issues** (filed with the plan, not built here): Clean
Descriptors (which would address the free-text description residual); Clean Pixel Data / Recognizable Visual
Features; DICOM egress resolving the IOD-conformance qualifiers; the schema `dicom_tag` binding (blocked on
consumer-schema registration); date aggregation as a Modified Dates method; per-subject custodian keys; URL
rewriting instead of R7 removal, if a consumer needs retrieve locations; `header = "none"` as a default (if D2
goes that way).

## Owner decisions needed

Each decision lists my recommendation and the reviewers' (rounds 1-3).

- **D1. Is de-identification the default for DICOM ingest?** *Mine:* yes, in phase P3d (Basic; round 3 agrees). Keep-PHI
  becomes the explicit `--keep-identity`. *Reviewers:* the same, with the flip placed in a phase, which P3d does. Cost:
  a breaking CLI change, and every internal script that relied on PHI in the clear must add the flag. Today the unsafe
  path is the default, and a forgotten flag ships identity.
- **D2. Default sealed header: de-identified or `none`.** *Mine:* de-identified, sealed only from P3b, after
  P2a-P2c have closed the UI, URI (R7), date (R4/R6), UN (R8), name (R9), identifier-copy (R10) and comment (R11)
  gaps. *Reviewers:* de-identified only once the UI and date gaps (round 1), the URI and alternative-calendar gaps
  (round 2), and B1 + B2 (round 3: R10 and R9) are closed. P3b depends on P2b, so the header is first sealed after
  all of them. Cost: Tessera's privacy is exactly Annex E plus §2.1, no stronger, and free text in
  attributes Annex E retains stays a residual. `none` is strictly safer but discards most DICOM-derived context.
- **D3. The anatomist extension.** *Mine:* `also_remove` in the spec now, and a schema `dicom_tag` binding once
  schemas are registrable. *Reviewers:* the same. Waiting for the schema route alone blocks anatomist's first
  ingest on an unbuilt feature.
- **D4. Disclosure.** *Mine:* a GitHub security advisory for `v0.1.0-alpha.1` plus a book security note.
  *Reviewers:* the same, because the published `YES` attestations are false. The cost is public acknowledgement.
- **D5. Pixels.** *Mine (adopting the reviewer's):* refuse on a `YES` flag with no override. Also refuse an absent
  BurnedInAnnotation for US/OT/XC/DOC/SC and the Secondary Capture classes unless `pixel_review = "acknowledged"`.
  Cost: an extra acknowledgement step for those modalities, and Tessera has no means to verify the
  acknowledgement.
- **D6 (new). Custodian key with crypto-shred** (§4.3). *Mine:* allow it, record the keyspace, warn on `shred`,
  and amend ADR-0047 §4. The alternative is to refuse the combination, which gives up cross-run linkage for
  crypto-shred users. *Reviewers:* allow, warn on `shred`, and record the keyspace in `inspect --json` (round 2), which
  P4c does. Cost of mine: erasure in a custodian keyspace
  requires destroying a key that serves every subject.
- **D7 (new). Redacted spec-hash pre-image for de-identifying runs** (§9.5, amends ADR-0035). *Mine:* adopt it.
  It extends to **every member's** `ingested_from` reference and spec path in a de-identifying run, non-DICOM
  members included (§9.2, round 3). The alternative is to drop the `ingested_via_spec` edge on de-identifying runs,
  which loses spec provenance. *Reviewers:* agree (round 2), with the non-DICOM extension (round 3). Cost of mine:
  two spec-hash pre-images exist, chosen by whether the run de-identifies, and `--print-spec-hash` is needed to
  verify one.

## Consequences

- One coherent, edition-pinned, testable claim replaces a 12-tag list. Every de-identified product says, inside
  its seal, which standard edition and options produced it.
- No new crate dependency. About 20 KB of generated, provenance-stamped data, and one stdlib Python generator
  whose eleven hand-written rules (§2.1 R1-R11) are each cited, two of them hand-classified over recorded searches.
- The de-identified seal no longer carries the header classes of §0 finding 1. In a de-identifying run, no
  member's `ingested_from` or `ingested_via_spec` edge carries a path, non-DICOM members included (§9.2, §9.5).
  Runs without a de-identifying member are unchanged.
- Determinism is unchanged: every derived value is a keyed function of the inputs.
- `manifest_hash` changes for de-identified DICOM ingests. There is no corpus regen.
- Residual risk, stated: free text in attributes Annex E retains (including URLs, dates or names typed into it,
  and the free-text descriptions R10 classified as Clean Descriptors territory); device and equipment identifiers
  that neither E.1-1 nor R10 removes; pixel
  content, including an acknowledged review (§8); linkage by anyone holding the original files (pixels and
  `content_hash`); the custodian key (§4.3); the operator's `name`/`study`; and the ADR-0047 §4 limits (no revocation,
  no audit) for the envelope.

## Revision log

**r2 (2026-10-18), after the round-1 audit of `08cc176`:**

| Audit item | Resolution |
|---|---|
| C1 39 unlisted UI tags survive | §2.1 R3 (generated keep 16 / replace 23, DICOM-root guard); §4.1; §11.1.3 |
| C2 unlisted DA/DT (and TM) kept or unshifted | §2.1 R4 (15 generated attributes), VR-keyed invariant; §5; §11.1.4 |
| C3 spec edge seals path + unkeyed hash | §0 finding 8; §9.5 redacted pre-image + opaque reference; D7; §11.1.7 |
| C4 byte grep blind to binary VRs; rows lack a VR | §0 finding 9; §11.0 per-VR oracle; §2.1 R5; §11.1.1 set equality |
| P Safe Private keeps 7 UI rows | §2.1 R3, §3, §4.1: replaced by the same map |
| P custodian keyspace vs shred | §4.3; D6 |
| P Δ keyed on PatientID only | §5: (Issuer, PatientID) subject key; refuse when absent or inconsistent |
| P E.1.1 group 0004 / file meta not pinned | §2.1 R1; §11.1.6 |
| P (0012,0063) > 64 characters, version churn | §7: multi-valued LO, no version |
| P absent burned-in flag proceeds | §8 rule 2; D5 |
| P coarsening rejection wrong | §5 corrected: conformant, deferred for utility |
| Phasing: P3 attests before P4 gates; P2 too big | §12: the rule, P2a/P2b, P3a/P3b/P3c, gates with the attestation |

**r3 (2026-10-18), after the round-2 audit of `8ebe318`** (the auditor's regeneration of the R3/R4 sets matched):

| Audit item | Resolution |
|---|---|
| C1 UIDs inside UR fields | §2.1 R7: 15 UR generated (3 in E.1-1; 12 unlisted: keep 2, remove 10); §11.1.13 |
| C2 alternative-calendar LO dates | §2.1 R6 (generated rule, exactly 2 attributes); §3.0; §5; §11.1.1 |
| C4 gap: implicit-VR private SQ as UN | §2.1 R8 (re-type or remove, no UN sealed); §11.0 check (d); §11.1.13 |
| N1 option precedence | §3.0: the date mode alone governs date rows; 11 overrides attested and recorded; §11.1.12 |
| N2 date counts | §3, §5 regenerated: 169 date-VR rows; Full/Modified 169 = 166 + 2 OB + 1 SH; 3 unmarked |
| N3 (006A,0003) | §2.1 R3, §4.1: 54 `U` rows replaced, the `D` row removed |
| N4 paths before envelope v2 | §6: two additive v1 fields in P3a; §9.2, §9.5 |
| N5 D1 not in any phase | §6, §12 P3d, D1 |
| N6 Δ and UN-UID encodings | §5 byte-exact Δ with 2 vectors; §4.1 byte-exact UID with a vector; R8 for UN |
| Phasing | §12: P2a/P2b/P2c, P3a-P3d, P4 before P4b; every criterion depends only on earlier phases |

**r4 (2026-10-18), after the round-3 audit of `a32d3f5`** (the auditor reproduced every count and both vectors):

| Audit item | Resolution |
|---|---|
| B1 UID copied into an ST | §2.1 R10: (0040,A390)+(0040,E001) removed; PS3.3 searched (7+91 hits), 10 removed |
| B2 unlisted names survive | §2.1 R9: every PN removed (3 newly covered); 6 text-VR names incl. (0074,100C) |
| (found in the search) | §2.1 R10 BreedRegistrationNumber; R11 23 unlisted comment fields; §11.1.15 |
| §3.0 "a TM is kept" | §3.0 rule 1: only TMs that Modified marks `C` or R4 lists |
| R8 nested UN | R8 rule 3: item elements re-enter rules 1-3, with the creator looked up inside the item |
| §4.1 raw vs masked bytes | §4.1: both given |
| mixed runs seal paths | §9.2 / §9.5 cover every member; D7; Consequences |
| §0 finding 1 overstated | §0 finding 1 reworded |
| phasing forward references | §12 rules 1-3; P2a-ii, P2b-ii, P4c split out; surfaces in P3a/P3b; P4's Δ part to P4b |
| CLI cannot declare a pseudonym | §3.1 `--pseudonym-field`, product-level keys; §11.1.16 |

One refinement to the round-1 audit, not a dispute: of the six E.1-1 rows without a VR in PS3.6 Table 6-1, two
((0002,0003), (0004,1511)) resolve from Tables 7-1/8-1. Only the two command-group rows and the two patterns
need the R5 handling.

## Appendix — the probe that generated the §2.1 lists

This uses the Python standard library only, and runs over `part06.xml` / `part15.xml` (2026d, digests in §2).
It prints the R3 keep and replace sets, the R4 date set, the R6 text-date set, the R7 UR split, the R9 unlisted
PN set and the R11 comment set. A second probe below runs the PS3.3 searches behind R9 part 2 and R10. P1 turns
both into the vendored generator.

```python
import re, sys, xml.etree.ElementTree as ET
NS = {"d": "http://docbook.org/ns/docbook",
      "xml": "http://www.w3.org/XML/1998/namespace"}
KEEP = re.compile(r"SOPClass|TransferSyntaxUID$|^CodingSchemeUID$"
                  r"|^ContextUID$|^MappingResourceUID$")

def rows(path, table_id):
    root = ET.parse(path).getroot()
    t = root.find(f".//d:table[@xml:id='{table_id}']", NS)
    txt = lambda e: " ".join("".join(e.itertext()).split())
    return [[txt(td) for td in tr.findall("d:td", NS)]
            for tr in t.findall(".//d:tbody/d:tr", NS)]

p06, p15 = sys.argv[1], sys.argv[2]
listed = {r[1].upper() for r in rows(p15, "table_E.1-1")}
reg = [r for r in rows(p06, "table_6-1") if r[0].upper() not in listed]
kw = lambda r: r[2].replace("\u200b", "")
ui = [r for r in reg if r[3] == "UI"]
print("keep", [r[0] for r in ui if KEEP.search(kw(r))])
print("replace", [r[0] for r in ui if not KEEP.search(kw(r))])
print("dates", [r[0] for r in reg if r[3] in ("DA", "DT", "TM")])
TEXT = ("LO", "SH", "ST", "LT", "UT", "UC")
print("r6", [r[0] for r in reg if r[3] in TEXT and "Date" in kw(r)])
UR_KEEP = re.compile(r"^CodingSchemeURL$|^URNCodeValue$")
ur = [r for r in reg if r[3] == "UR"]
print("r7_keep", [r[0] for r in ur if UR_KEEP.search(kw(r))])
print("r7_remove", [r[0] for r in ur if not UR_KEEP.search(kw(r))])
print("r9_pn", [r[0] for r in reg if r[3] == "PN"])
print("r11", [r[0] for r in reg if r[3] in TEXT
              and re.search(r"Comment|Notes?$|Remark", kw(r))])
```

The PS3.3 search (R9 part 2, R10). It also uses the standard library only, and runs over `part03.xml` (2026d,
sha256 `3ae695b956ba57c490933a49220b515aeaedb5ece86f05c4bf7e94ee0fbcc6ef`, fetched 2026-10-18) plus the PS3.6
registry and E.1-1 rows parsed above:

```python
def ps3_hits(p03, vr, listed, pattern):
    """Text-VR attributes not in E.1-1 whose PS3.3 description matches."""
    tag = re.compile(r"^\(([0-9A-Fa-f]{4}),([0-9A-Fa-f]{4})\)$")
    desc = {}
    for tr in ET.parse(p03).getroot().iter(f"{{{NS['d']}}}tr"):
        cells = [" ".join("".join(td.itertext()).split())
                 for td in tr.findall("d:td", NS)]
        for i, c in enumerate(cells):
            m = tag.match(c)
            if m:
                t = f"({m[1].upper()},{m[2].upper()})"
                desc.setdefault(t, set()).add(" ".join(cells[i + 1:]))
                break
    text = {"ST", "LT", "UT", "LO", "SH", "UC"}
    rx = re.compile(pattern, re.I)
    return sorted(t for t, ds in desc.items()
                  if vr.get(t) in text and t not in listed
                  and rx.search(" | ".join(ds)))
```

It is called with patterns B, C and D of R10, which give 23, 7 and 91 hits.
