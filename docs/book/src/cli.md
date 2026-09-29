# Command reference

The full `tessera` command surface, grouped by task. Each command has task-oriented coverage in the
chapters above; this is the at-a-glance index and the `--help` detail.

## Where the transcripts live

The blocks below are `{{#include}}`d from the project's `trycmd` suite
(`tessera/crates/tessera-cli/tests/cmd/*.trycmd`) — the same files CI runs the real binary against, so
every transcript here is verified on every build and cannot drift from the tool's behaviour.

Reading this page **outside a rendered book** (on GitHub, say) you will see the include directives rather
than their contents. That is deliberate: there is one copy of each transcript, and it is the one the tests
execute. The files are plain text and perfectly readable in place —
[`help.trycmd`](https://github.com/vig-os/tessera/blob/dev/tessera/crates/tessera-cli/tests/cmd/help.trycmd)
is the grouped command index, and the rest of that directory is a walkthrough per topic. `tessera --help`
prints the same index locally.

## Worked transcripts

The blocks below are `{{#include}}`d from the project's `trycmd` suite
(`tessera/crates/tessera-cli/tests/cmd/*.trycmd`) — the same files CI runs the real binary against, so
every transcript is verified on every build and cannot drift from the tool's behaviour.

Reading this outside a rendered book (on GitHub, say) you will see the include directives rather than
their contents — the transcripts themselves live in those `.trycmd` files and are perfectly readable
there, which is the point of keeping one copy rather than two.

### Overview

{{#include ../../../tessera/crates/tessera-cli/tests/cmd/help.trycmd}}

### Version & sub-command help

{{#include ../../../tessera/crates/tessera-cli/tests/cmd/commands.trycmd}}

### Provenance: the recipe and the chain

{{#include ../../../tessera/crates/tessera-cli/tests/cmd/provenance-walk.trycmd}}
