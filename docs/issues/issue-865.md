---
type: issue
state: open
created: 2026-11-15T11:08:14Z
updated: 2026-11-18T14:52:08Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/865
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-19T09:52:13.571Z
---

# [Issue 865]: [test(ci): measure the fsync and tmp footprint of the non-cargo checks and decide whether they need the RAM TMPDIR](https://github.com/vig-os/tessera/issues/865)

Follow-up to #793.

#793 puts `TMPDIR` on a RAM fs (`withRamTmpdir` in `flake.nix`) for the checks that run tessera's tests through cargo (`workspace-test`, `workspace-doctest`, `sql-tests`, `s3-range-read`, `seal-profile-determinism`, `ingest-gate-a*`). On a [batch-host] pod the build's TMPDIR sits on the shared `/nix` volume, and every `WriteSession` commit waits on three fsyncs there (#695). The flake's evaluation guard only looks for cargo test / nextest / cargo run.

Five checks run tessera's write path some other way and keep the on-disk TMPDIR:

- `tessera-py-import` and `tessera-wheel-import` (the Python `Builder`)
- `ingest-producer-equality` (`tessera ingest`, which goes through `WriteSession` via `tessera-ingest/src/engine.rs`)
- `oci-roundtrip` and `registry-roundtrip` (the CLI)

Their fsync counts and tmp peaks have not been measured.

## Ask
- Measure each one's fsync count (strace `-c -e trace=fsync,fdatasync`) and peak TMPDIR use. The `#footprint` app in `tessera/bench/[batch-host]/793-io-hang/` already does the latter.
- If any of them issues fsyncs in the hundreds, give it the RAM TMPDIR. They are `runCommand`/non-crane derivations, so the helper needs a form that doesn't rely on `preBuild`. Then extend the guard so the coverage can't silently regress.
- Otherwise, record the numbers in the `flake.nix` comment and close this.
