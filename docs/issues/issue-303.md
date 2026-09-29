---
type: issue
state: closed
created: 2026-07-02T14:26:29Z
updated: 2026-09-29T01:03:09Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/303
comments: 1
labels: bug
assignees: none
milestone: 0.1.0-alpha.2
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:52.055Z
---

# [Issue 303]: [bug(cli): read can't reach extra/ blocks (e.g. extra/dicom_header) — only ls/tree can](https://github.com/vig-os/tessera/issues/303)

## Context — first-user DUPLET FAIR shakedown

The full preserved DICOM header lands under `extra/dicom_header` (object, 167 keys — great, no
pruning). It's reachable via `ls`/`tree`:

```
$ tessera ls  study.tsra extra/dicom_header   # works — dumps the 167-tag header
$ tessera read study.tsra extra/dicom_header   # FAILS
error: container: logical_table: no blocks for prefix 'extra/dicom_header'
```

`read` is table-oriented and can't reach `extra/*` object blocks, so the two navigation verbs are
inconsistent about what's addressable. Minor, but confusing for a first user exploring what's inside.

## Ask

Either make `read` fall through to object/`extra` blocks (emit JSON), or emit a clear "use `ls` for
object blocks" hint instead of the internal `logical_table` error.

Found during: DUPLET first-user FAIR ingest (DP01).

---

# [Comment #1]() by [gerchowl]()

_Posted on July 2, 2026 at 04:00 PM_

Fixed on `feature/286-tsra-explorer` — commit da2b14b. Root cause: `read` is table-only and matches on named *blocks*, but `extra/*` are entries in `manifest.extra` (not blocks), so the target fell through to the logical-table decoder and surfaced the internal `logical_table: no blocks for prefix` error. Went with the clear-hint option (matches `read`'s existing non-table-block hint pattern): a guard now detects an `extra/<key>` target and returns *"'extra/dicom_header' is an extension (`extra/`) field, not a table — `read` is for tables. Use `tsra ls <file> extra/dicom_header` to dump it."* clippy/rustfmt/typos green; behaviour-preserving for all existing trycmd walkthroughs. (No trycmd case added — the committed corpus fixtures carry no `extra/` field; worth adding when a fixture with a preserved header lands.)

