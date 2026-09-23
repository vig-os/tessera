# Command reference

The full `tessera` command surface, grouped by task. Each command has task-oriented coverage in the
chapters above; this is the at-a-glance index and the `--help` detail.

> Every block below is `{{#include}}`d from the project's `trycmd` test suite
> (`crates/tessera-cli/tests/cmd/*.trycmd`) — the same files CI runs the real binary against, so these
> transcripts are verified on every build and cannot drift from the tool's behaviour.

## Overview

{{#include ../../../tessera/crates/tessera-cli/tests/cmd/help.trycmd}}

## Version & sub-command help

{{#include ../../../tessera/crates/tessera-cli/tests/cmd/commands.trycmd}}
