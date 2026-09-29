---
type: issue
state: closed
created: 2026-09-29T04:31:53Z
updated: 2026-09-29T05:57:43Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/489
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:25.082Z
---

# [Issue 489]: [versioning.trycmd documents failures: diff/publish/verify assert a stale version hash's error output](https://github.com/vig-os/tessera/issues/489)

## What

The last three cases of `tessera/crates/tessera-cli/tests/cmd/versioning.trycmd` assert **failure
output** as if it were the walkthrough's expected result:

```
$ tessera diff repo blake3:c40501d7ef97e3aceda7a44273152c48ca23a905aa1c331a92a0eb6959fd2004
? 1
error: io: No such file or directory (os error 2)
```

…and the same for `publish` and `verify published.tsra`. The test **passes**, because the snapshot
records the error.

## Why it is wrong

`c40501d7…` is a stale version hash. Earlier in the same file, `commit` prints the version it actually
produced:

```
$ tessera commit repo blake3:12643649… --set tracer=FDG
committed blake3:12643649[..]
  version    blake3:f5fc37b4673c991d3da16f08e83238cc6f591ffe5cb67a6017031fd69445f489
  supersedes blake3:7c9f7bd9[..]
```

So `diff` is asked for a version that does not exist, fails, and `publish`/`verify` then fail for the
file `publish` never wrote. The ids in this walkthrough are content hashes and changed at some point;
the file was re-blessed with `TRYCMD=overwrite`, which dutifully recorded the new (error) output rather
than flagging that three documented steps had stopped working.

## Why it matters

This file is **docs-as-tests**: it is `{{#include}}`d into the book as the versioning chapter's
transcript, so the rendered docs currently show a reader that `diff`, `publish` and `verify` fail. It
also means the CoW versioning path's three most interesting verbs have no working walkthrough coverage —
the gate is green while asserting they are broken.

## Fix

Repoint the three cases at the version `commit` actually prints (`f5fc37b4…`), confirm each step's real
output, and re-pin. Worth checking the same way whether any other `.trycmd` case pins an error it did not
mean to — `grep -l '^? 1' tests/cmd/*.trycmd` and read each hit against its prose.

## Guard

`TRYCMD=overwrite` cannot tell an intended error from a regression, so a blessed snapshot is only as
good as the read afterwards. A cheap gate: fail if a `.trycmd` case expects a nonzero exit **and** its
stderr matches `No such file or directory`, which is never a deliberate teaching example.

Found while adding `provenance-walk.trycmd` for #452 (that file's cases were each verified by hand rather
than blessed).

