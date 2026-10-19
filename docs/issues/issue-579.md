---
type: issue
state: open
created: 2026-10-16T23:16:46Z
updated: 2026-10-18T12:50:41Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/579
comments: 2
labels: bug
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-19T07:17:59.263Z
---

# [Issue 579]: [bug(dicom de-id): PHI_TAGS is a 12-tag subset; veterinary owner tags and other PS3.15 tags survive into the sealed manifest even with --recipient](https://github.com/vig-os/tessera/issues/579)

Found by MorePET/anatomist's pre-mortem of its tessera adoption ADR (PR #103), with synthetic pydicom data only (invented 'fictus' names).

**Repro (tessera dev fa581f5):**
1. Build a synthetic DICOM series that sets `ResponsiblePerson (0010,2297)`, `ResponsiblePersonRole (0010,2298)`, `ResponsibleOrganization (0010,2299)`, `InstitutionAddress (0008,0081)`, `StationName (0008,1010)`, `DeviceSerialNumber (0018,1000)`, `StudyID (0020,0010)` and `ImageComments (0020,4000)` (with an owner name and phone number).
2. `tessera ingest --spec` with `format = "dicom-series"` and one `recipients` entry. Exit 0; `aux/identity/identity.age` is written, and `reidentify` works.
3. `tessera shred` → exit 0, `verify` → exit 0.
4. Grep the sealed manifest.

**Expected:** no personal data in the seal after de-identification. That is the premise of ADR-0047 crypto-shredding: destroying the key erases the identity.
**Actual:** the sealed `extra.dicom_header` still holds the owner name (0010,2297), the institution address, an image comment with name and phone, the station name, the device serial and the study ID. The sealed metadata also keeps the original Study and Series Instance UIDs, which tessera's schema marks `identifying`, plus `study_date`. None of this is removed by `shred`, because it sits inside the seal.

**Cause:** `tessera-ingest/src/dicom.rs:657`: `PHI_TAGS` is a 12-tag "pragmatic subset", not DICOM PS3.15 Annex E, and includes none of the veterinary owner tags.

**Proposed fix:**
- implement the PS3.15 Basic Application Level Confidentiality Profile, including the veterinary tags (0010,2297–2299) and private tags (remove by default);
- replace or drop the UIDs (a mapping goes into the identity envelope);
- coarsen or shift dates per option;
- seal no raw `extra.dicom_header`; put the full original header only inside the encrypted envelope;
- add a regression test with a seeded veterinary header asserting that no PHI string appears in the seal.

**Severity:** high for any crypto-shred user. The shred can't erase identity that's in the seal.
---

# [Comment #1]() by [gerchowl]()

_Posted on October 16, 2026 at 11:31 PM_

Design note from the consumer side: tessera's own schema already marks the Study and Series UIDs `identifying`, yet DICOM ingest seals them, because the strip list is the hard-coded `PHI_TAGS` rather than the schema. Suggest: the de-identification set = a full PS3.15 baseline ∪ every field the product schema marks `identifying`, so a consumer schema (ADR-0050 pattern, e.g. anatomist adding the veterinary owner tags 0010,2297–2299) extends it declaratively, with no code change. Anatomist will supply such a schema with its DICOM ingest (MorePET/anatomist#87).

---

# [Comment #2]() by [gerchowl]()

_Posted on October 18, 2026 at 12:50 PM_

Phase → #N:

- P1 → #611
- P2a → #612
- P2a-ii → #613
- P2b → #614
- P2b-ii → #615
- P2c → #616
- P3a → #617
- P3b → #618
- P3c → #619
- P3d → #620
- P4 → #621
- P4b → #622
- P4c → #623
- P5 → #624


