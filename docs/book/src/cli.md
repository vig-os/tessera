# Command reference

The full `tessera` command surface, grouped by task. Each command has task-oriented coverage in the
chapters above; this is the at-a-glance index and the `--help` detail.

## The command index

Every verb, grouped the way `tessera --help` groups them. This block is generated from the binary's own
help output by the `derived-docs` gate, so it is here in the raw file — no build step needed to read it —
and a new command cannot land without this page gaining it in the same commit:

<!-- guardrails:derived cmd="cd tessera && cargo run -q -p tessera-cli --bin tessera -- --help" -->
Tessera FAIR data-product CLI

Usage: tessera <COMMAND>

Inspect & navigate:
  inspect     Manifest summary (id, product, blocks, hashes)
  verify      Verify integrity (magic, seal, every block digest)
  schema      Validate against the embedded product schema (--json dumps it)
  tree        Render the .tsra as a navigable hierarchy
  ls          List one node's children (meta / a block / sources)
  read        Read table data as CSV/TSV/NDJSON (cross-block)
  stats       Numeric overview of an array block (shape, dtype, value range)
  slice       Pull a plane/line/point of an array block (--index z,:,: · --format csv|json|npy|png)
  project     Collapse an array along an axis → 2-D image (--mode max|mean|sum)
  pyramid     Build a multiscale pyramid of an array block → a new .tsra
  export      Emit a FAIR discovery record (JSON to stdout)

Query (needs --features sql):
  sql         Run SQL (DataFusion) over a table block

Ingest & pack:
  ingest      Ingest a vendor file (or a declarative --spec) into a sealed .tsra
  pack        Pack an exploded dir (manifest.json + blocks/) into a sealed .tsra
  unpack      Explode a .tsra into a directory
  extract     Extract one block's raw bytes (digest-verified)

Versioning (content-addressed repo):
  init        Initialize a content-addressed repository for CoW versioning
  import      Import a sealed .tsra as the first version of its lineage
  commit      Commit a new version (reuses unchanged blocks by digest)
  log         Show a lineage's version history
  diff        Diff two versions (blocks + metadata)
  seal        Export a version to a standalone .tsra with its history
  publish     Export a history-free standalone .tsra for publication
  forget      Forget a lineage (objects reclaimed by gc)
  gc          Reclaim objects unreachable from any ref

Collections (catalog of products):
  collection  Inspect / ls / verify a collection.json + its members

Distribution (needs --features cloud):
  push        Push a sealed .tsra to an OCI registry
  pull        Pull a .tsra OCI artifact from a registry

Signing & trust:
  keygen      Generate an ed25519 keypair
  trust       Manage the trust store of public keys verify-sig accepts
  sign        Sign a sealed .tsra (embeds aux/signatures/<key_id>.sig.json; --sidecar for detached)
  verify-sig  Verify a sealed .tsra against its signature (embedded first, sidecar fallback)

Confidentiality (crypto-shred):
  reidentify  Recover a crypto-shred product's identity with a recipient private key (ADR-0047)
  shred       Permanently remove a product's identity envelope (aux/identity)

Diagnostics:
  info        What this build is (version, backends, build modes) — --json for machines
  bench       Bench the write engine on this host (throughput + peak RSS)

Run `tsra help <command>` for details, flags, and what to pass.

Options:
  -h, --help     Print help
  -V, --version  Print version
<!-- guardrails:derived:end -->

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
