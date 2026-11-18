---
type: issue
state: open
created: 2026-11-17T16:16:42Z
updated: 2026-11-18T00:22:30Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/895
comments: 2
labels: feature, priority:medium, area:core, adr
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-18T09:43:23.400Z
---

# [Issue 895]: [feat(schema): product schemas declare number encodings and value ranges instead of hard-coded ingest limits](https://github.com/vig-os/tessera/issues/895)

## Problem
#886 / PR #894 enforces the I-JSON bound (sealed JSON numbers within ±(2^53−1)) as one format-level check in `canonical`. The product schemas don't state it, though, and per-source limits are hard-coded in ingest code:
- `FieldSpec.dtype` accepts any `DType` name, so a schema can declare a metadata field as `u64`/`i64`. A sealed number can't hold that type's full range. Today the schema accepts that, and the conflict only shows up as a by-value refusal at write time.
- Ingest sources (DICOM DS/FD curated fields in #894; SV/UV → string) decide number handling in Rust, not in the schema that describes the field.
- Domain ranges (KVP, slice thickness, rescale slope, …) aren't declared anywhere. A physically impossible value is caught only if it also crosses 2^53.

## Proposal (needs an owner decision; post-alpha.6)
1. **The bound stays a format invariant.** It is not configurable per schema: a schema that relaxed it would reopen the #886 collision, and a seal must not depend on product data. SPEC §1 states it once.
2. **Schemas must be consistent with it.** Schema validation refuses a metadata `FieldSpec` whose `dtype` can exceed the bound (`u64`, `i64`) unless it declares a string encoding (e.g. `"dtype": "u64", "encoding": "string"`). The "by type, never by value" rule is then declared in the schema, not hard-coded per ingest source, and the DICOM SV/UV handling reads it from the schema.
3. **Optional domain ranges.** `FieldSpec` gains optional `minimum`/`maximum` (inclusive, within the bound), validated at ingest, with errors that name the field and its source (e.g. the DICOM tag). This replaces the hard-coded DICOM range checks with schema data, in line with the schema-driven / domain-agnostic engine. New keys follow ADR-0060's newer-field rules (`skip_serializing_if`, so no existing seal moves).

## Acceptance
- An ADR or an ADR-0020/ADR-0060 amendment records 1–3.
- Built-in schemas declare their 64-bit and range rules; the hard-coded ingest checks are removed in favour of schema lookups.
- Tests: a schema with a bare `u64` metadata field is refused; an out-of-range value is refused naming the field and its source; in-range values and existing seals are unchanged.

---

# [Comment #1]() by [gerchowl]()

_Posted on November 17, 2026 at 07:56 PM_

Held pending #897 (seal encoding spike). If deterministic CBOR is chosen, 64-bit fields are native and this reduces to the value-range part (min/max).

---

# [Comment #2]() by [gerchowl]()

_Posted on November 18, 2026 at 12:22 AM_

Un-held: the owner chose ADR-0066 option A′ (#897, 2026-11-18), and this issue is part of it: schema-declared 64-bit-as-string encodings plus value ranges.

