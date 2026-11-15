---
type: issue
state: open
created: 2026-11-15T01:51:15Z
updated: 2026-11-15T01:51:15Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/857
comments: 0
labels: bug, security, priority:high, area:io, area:integrity
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-15T08:48:29.661Z
---

# [Issue 857]: [fix(io): zip GHSA-m665-v7pm-7grj: a .tsra that verifies can read differently under a streaming zip reader](https://github.com/vig-os/tessera/issues/857)

## Advisory

[GHSA-m665-v7pm-7grj](https://github.com/advisories/GHSA-m665-v7pm-7grj) (CVE-2026-115905), critical. `zip`
`<= 9.0.4` has a parser differential. `ZipArchive` trusts the central directory, while `ZipStreamReader`
trusts the local file headers, so one archive can show two different sets of members. 9.0.5 fixes it, but
only as an **opt-in** API (`ZipReadOptions::enforce_header_consistency`,
`ZipArchive::validate_header_consistency`). Default reads behave exactly as before.

It was found because Dependency Review on the dev→main promotion PR #855 flags zip 9.0.4 as the only
vulnerable package in v0.1.0-alpha.5. Renovate's #689 bumps only `Cargo.lock` and fails CI.

## Impact on tessera

- `.tsra` is a STORED zip64 container, and tessera reads it only through the central directory. Every
  `by_name` takes the compression method from the CD, and block digests are checked on the CD view. So
  tessera's own view is consistent, and it cannot be tricked into reading unverified bytes.
- **It is still affected as an integrity format.** `tessera verify` attests the central-directory view
  only. A crafted, genuinely sealed `.tsra` passes `verify` on zip 9.0.4 while a streaming reader (zip's own
  `ZipStreamReader`, libarchive in streaming mode, Java's `ZipInputStream`) sees something else:
  1. **name cloak:** a block whose local header says `manifest.json`, so a streaming reader sees two
     manifests;
  2. **codec cloak:** a STORED block whose sealed bytes are also a valid deflate stream, with a local
     header that says DEFLATE and carries the CRC of the inflated bytes, so a streaming reader extracts
     other content with a valid checksum;
  3. **duplicate name:** two CD records for one block, where zip resolves to the last record (the
     genuine one) and a first-match reader gets the forged one;
  4. **hidden member:** a complete local entry in bytes no CD record covers. zip 9.0.5's own
     consistency check **accepts** this one, because no header disagrees and nothing overlaps.
- The `.npz` ingest reader is not changed. Its reference reader, numpy through `zipfile`, also reads the
  central directory.

## Fix

- zip `>= 9.0.5` (workspace floor and lockfile).
- Every `.tsra` member read refuses a member whose local header disagrees with its CD record, and refuses
  any read from an archive that names a member twice. This costs no extra I/O, because zip reads that
  header anyway to find the data.
- `verify` (serial and parallel) also checks that the entries tile the archive from byte 0 to the central
  directory, with no gap and no overlap. This is tessera's own rule, and it is what catches case 4.
- SPEC §6 states the one-reading rule.
- Snapshot follow-ups that made #689 red: `info.trycmd` (`npz-member 9.0.5`), the `ingest_npz_member`
  decoder digest in `corpus/ingest-corpus.json`, and the `zip.txt` feature snapshot.

