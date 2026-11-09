---
type: issue
state: open
created: 2026-11-09T05:30:11Z
updated: 2026-11-09T05:30:11Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/785
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-09T09:04:18.720Z
---

# [Issue 785]: [docs(ingest): RUSTDOCFLAGS=-D warnings cargo doc -p tessera-ingest fails on an unresolved [`parse_header`] link](https://github.com/vig-os/tessera/issues/785)

## Observed (dev at 26159de)

```console
$ cd tessera && RUSTDOCFLAGS="-D warnings" cargo doc --no-deps -p tessera-ingest
error: unresolved link to `parse_header`
  = note: the link appears in this line:
          a restricted-literal header parse ([`parse_header`]) and a buffer handed to
  = note: no item named `parse_header` in scope
error: could not document `tessera-ingest`
```

The link is at `tessera/crates/tessera-ingest/src/npy.rs:7`. `pub fn parse_header` exists at `npy.rs:145`, so the link
should resolve; why rustdoc can't find it in scope still needs investigating.

## Why it matters

No gate runs rustdoc with `-D warnings`, so broken intra-doc links in the ingest crate go unnoticed. Found while
working on #762; it is not caused by that change.

## Ask

Fix the link, and consider a rustdoc `-D warnings` gate for the workspace so this class of breakage is caught.
