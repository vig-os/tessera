---
type: issue
state: open
created: 2026-11-16T08:31:12Z
updated: 2026-11-16T08:31:12Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/888
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-16T08:59:31.945Z
---

# [Issue 888]: [core/io: block specs (ArraySpec/TableSpec/blob) still accept the array form and have no newer-field walk](https://github.com/vig-os/tessera/issues/888)

## Problem

#873 (PR #883) makes the manifest, collections and the embedded product schema accept only their declared
spelling. A record must be a JSON object, not a positional array, and an enumerated name must be a string,
not `{"name": null}`. All three now deserialise through one check, `tessera_core::canonical::from_declared_form`.

`blocks[i].spec` is opaque to the spine (SPEC §5), so that check does not reach it. Its typed views are still
read with plain `serde_json::from_value`, which also accepts a positional array and a one-key-object name:

- `ArraySpec` / `TableSpec`: tessera-io's reader, tessera-py (`src/lib.rs:272,282`), tessera-cli (`nav.rs:899`)
- the blob spec: `tessera-io/src/blob.rs:141`

So a sealed block spec such as `{"dtype": …, "columns": [["t","u8", …]]}` decodes as if each column were the
object form. Any key inside an array-form column is invisible.

The block specs also have no newer-field walk (ADR-0060 §2.2) at all, so an unknown key in a spec is dropped
silently today, with or without the array form.

## Proposed

Decide the forward rule for block specs (ADR-0060 §2.2 scope, SPEC §5a/§5b/§5e), then:
- read every spec through `from_declared_form` (make it `pub` or add a `tessera_core` helper);
- add the declared-field walk if specs are format rather than data.

The seal is unaffected, since the seal is over the object as parsed. Only interpretation changes.

Found by the review of #883. Out of that PR's scope, because the brief excluded `spec` as opaque.

