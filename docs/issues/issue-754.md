---
type: issue
state: open
created: 2026-11-03T13:13:27Z
updated: 2026-11-03T13:13:27Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/754
comments: 0
labels: refactor, priority:medium, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-04T08:35:54.488Z
---

# [Issue 754]: [refactor(io): one source opener for local and URL products (TsraSource lives in the CLI; most verbs and Python are file-only)](https://github.com/vig-os/tessera/issues/754)

Found by an independent SOLID/DRY review of dev 74fc409 (astra + Fable, cross-checked by a third verifier). Confirmed by the verifier.

## Defect (DIP): the product-source abstraction lives in the CLI binary
- `tessera-cli/src/main.rs` ~49-77 defines `trait TsraSource` and `open_local_or_url`. They are used only at ~1370/1500/1742 (`inspect`, `verify`, `extract`).
- Every other verb calls `Reader::open(path)` directly: `main.rs` ~1519/1766/1883; `nav.rs` ~588/1068/1329/1482/1518/1869; `sql.rs` ~164; `version.rs` ~29/153; `provenance.rs` ~439; `collection.rs` ~127/221/271. So does Python (`tessera-py/src/lib.rs` ~54).
- io exposes only `cloud::open_url -> Reader<ObjectStoreReader>` and `Reader::open(Path)`.

## Cost
Only `inspect`, `verify` and `extract` accept `s3://`. `tree`, `ls`, `read`, `stats`, `sql`, `log`, `provenance` and Python don't, and every new verb re-decides.

## Fix shape
io owns one `open_any(&str) -> Reader<Box<dyn ReadSeek…>>` (or an object-safe `Source` trait), feature-gated on `cloud`. Delete `TsraSource`. Every verb and Python use it.

No ADR is needed (ADR-0002 §4 already un-deferred cloud reads). Related: #672, integrity for object-store sources.

