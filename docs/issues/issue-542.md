---
type: issue
state: open
created: 2026-10-04T22:35:29Z
updated: 2026-10-04T22:35:29Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/542
comments: 0
labels: bug
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-05T08:17:43.080Z
---

# [Issue 542]: [ingest: a piped input seals a content-independent ingested_from digest](https://github.com/vig-os/tessera/issues/542)

## What

On a **non-seekable input** (a pipe, fifo, or `<(…)` process substitution), the sealed
`ingested_from` edge carries a `content_hash` that is **independent of the ingested bytes** — the
same constant for every such ingest. The integrity link back to the source-of-record is therefore a
false claim rather than a missing one.

## Evidence

Two CSVs with *different* content, each ingested through a process substitution:

```
$ tessera ingest table <(zcat a.gz) a.tsra --name p --timestamp … --from csv --column id:i4 --column x:f8
$ tessera ingest table <(zcat b.gz) b.tsra --name p --timestamp … --from csv --column id:i4 --column x:f8

a: content blake3:a03198f6b21c47993  source_digest blake3:16287da8bcdcfa3a888ff3e9ab50a7fe747368a3feefe61ff0bb840218a1ed47
b: content blake3:92b304d6ab22ca097  source_digest blake3:16287da8bcdcfa3a888ff3e9ab50a7fe747368a3feefe61ff0bb840218a1ed47
```

`content_hash` differs, so the two really are different products. `source_digest` is **identical**.

A regular file gives a digest that does track the content:

```
plain: edge ingested_from …/a.csv  blake3:e5bca4adf93322adb35efc126a7d97ec9b38595793c26415d87a93bca09a72e4
sub:   edge ingested_from /dev/fd/63 blake3:16287da8bcdcfa…
```

## Cause

The batch table path reads the input **twice**: once to decode, and again in
`provenance::source_digest`, which does its own `File::open` + `digest_reader`. On a pipe the second
open returns the same already-drained descriptor, so the digest is taken over **zero bytes** — hence
one constant, for any input.

`ingest table` exits 0 and prints a success line, so nothing signals it.

## Why it matters

This is a FAIR integrity claim, and a wrong digest is worse than an absent one: `Source::content_hash`
is optional, so recording nothing would have been honest. It is also invisible to the usual checks —
`content_hash` is correct (the decode saw the real bytes), the product verifies, and only the
provenance edge is wrong.

## Scope

Pre-existing, independent of #458, and on the **batch** path. #458 touches it only in that its
`decide_stream_table` routes non-seekable inputs to batch (streaming needs two passes, so it cannot
take them) — the routing is right, this destination is not.

## Suggested fix

Hash while decoding and pass the result to `provenance::ingested_from_digest`, which already exists
for exactly this reason (the DICOM series backend uses it so the bytes are read once). Failing that,
refuse to seal a digest we could not actually compute, rather than sealing the empty one.

