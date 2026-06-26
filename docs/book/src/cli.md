# Command-line tool

The `tessera` CLI packs, unpacks, verifies, and inspects `.tsra` products.

> Every command block below is `{{#include}}`d from the project's `trycmd` test suite
> (`crates/tessera-cli/tests/cmd/*.trycmd`) — the **same files** CI runs the real binary against. So
> these transcripts are verified on every build and cannot drift from the tool's actual behaviour.

## Overview

{{#include ../../../tessera/crates/tessera-cli/tests/cmd/help.trycmd}}

## Version & sub-command help

{{#include ../../../tessera/crates/tessera-cli/tests/cmd/commands.trycmd}}

## Inspecting a product

`inspect` prints a human summary of a `.tsra`'s manifest — its identity, product kind, and the per-block
digests that roll into the content hash:

{{#include ../../../tessera/crates/tessera-cli/tests/cmd/inspect.trycmd}}

(The hash lines render as `[..]` here because the transcript uses `trycmd` wildcards so it survives a
corpus regeneration; the structural lines are matched exactly.)
