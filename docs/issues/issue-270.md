---
type: issue
state: closed
created: 2026-07-01T14:24:56Z
updated: 2026-09-28T17:54:38Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/270
comments: 1
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:59.617Z
---

# [Issue 270]: [polish(cli): usability batch from the 5-persona review — export shape, --timestamp/--meta validation, file-not-found path, ls extra, --force, schema wording](https://github.com/vig-os/tessera/issues/270)

Consolidated smaller UX findings from the fresh-context usability review (all 5 personas). Each is a quick, self-contained fix.

- **`export <FILE>` fails** (`unrecognized subcommand '<file>'`) — it's a group (`ro-crate`/`datacite`) but the top-level tagline reads like a leaf verb. **All 5 reviewers hit this.** → default bare `export <FILE>` to `ro-crate`, or fix the tagline + list exporters on the arg error (like `ls` does).
- **`--timestamp "yesterday"` accepted** → seals cleanly, then `export ro-crate` emits `"datePublished":"yesterday"` (invalid) and DataCite `"publicationYear":0`. → parse RFC-3339 at the door, reject clearly.
- **`--meta modality=BOGUS` overrides a `required·coded` field and still passes `schema-valid`.** → enforce `coded` at schema-check; require `--meta-override` to clobber a schema-derived value.
- **File-not-found errors don't quote the path** (`error: io: No such file or directory (os error 2)`). → include the filename in every I/O error (contrast the excellent node-not-found error).
- **`ls FILE extra` is silent-empty** (exit 0, no output) when there are no extras. → print `no extra fields` / `(0 fields)` to match `meta/ (N fields)`.
- **`ls FILE extra/dicom_header` dumps ~600KB JSON** instead of listing the 138 child keys. → list keys (with VR + value preview); add an explicit `--json`/`read-json` to dump; enables path-addressed reads (`ls extra/dicom_header/0028,1053`).
- **Silent clobber of an existing sealed `.tsra` on re-ingest.** → refuse without `--force`; print the existing `manifest_hash` in the refusal (sealed artifacts should be immutable-by-default).
- **`schema`/`ls schema` print "file predates embedded schemas"** on files this build produces (dicom-series/ge-hdf5 don't embed yet). → say `schema: recon v1.0 (built-in registry)`; and embed the schema on ALL ingest paths, not just blob.
- **`producer` stamped by blob ingest but not dicom-series/ge-hdf5** — inconsistent. → stamp on every seal path.
- **Packaging:** ship the default binary with `sql` (or hide `sql` from help when not built + point the fallback at an install URL, not `cargo build`). Add a pre-alpha `0.0.0` banner; document that `tsra` and `tessera` are the same binary.
---

# [Comment #1]() by [gerchowl]()

_Posted on September 28, 2026 at 05:54 PM_

Closing as **done** — verified on `origin/dev` in the 2026-09-28 backlog triage.

Evidence: commit b10042f 'fix(cli): usability quick-wins from the 5-persona review (read<blob>, export shape, ls extra, schema wording) (#273)'.

https://claude.ai/code/session_01XdERKMVDAwfMJSKdTytNnK

