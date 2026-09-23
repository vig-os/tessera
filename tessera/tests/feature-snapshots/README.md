# Feature snapshots — Gate B (ADR-0057 §5)

These files are **not** test fixtures. They are the committed baseline of the resolved Cargo feature
graph for every crate on the **seal path** — the crates whose behaviour can move sealed bytes.

Each `<crate>.txt` is the output of:

```bash
cargo tree --locked --offline --all-features --charset ascii -e features -i <crate> | sort
```

normalised (absolute paths stripped, workspace-member versions pinned to `vWORKSPACE`) so the file
depends on the dependency graph and nothing else. See `scripts/feature-snapshots.sh` — it is the
single source of truth for the crate list and the invocation.

## What this gate is for

The guarantee ADR-0057 §7 states, and that Gate B protects:

> For any input `F` and any tessera version `V`, `tessera ingest F` produces the same `content_hash`
> under every distributed build of `V`. Feature selection may change which formats are **readable**;
> it must never change the **bytes** produced for a readable one.

Cargo feature unification is **monotonic** — enabling a feature anywhere in the graph can only add
features, never remove them. So an optional feature on an apparently unrelated crate can silently
change what the encoder does:

- the optional `sql` feature already turns on `arrow-array/chrono-tz` — ADR-0056 hazard **H1**
  (tzdb as a compiled-in dependency vs. the host's `/usr/share/zoneinfo`), rated *high × fatal*,
  arriving through a **feature** rather than through a host. It is captured in `arrow-array.txt` as
  today's baseline, deliberately, so the drift is recorded rather than discovered later;
- the workspace pins `vortex-btrblocks = { features = ["pco"] }` specifically to **exclude** ALP,
  which was not byte-deterministic (#380/#384). Any future dependency that transitively enables
  `vortex-btrblocks/alp` re-registers ALP in the compressor dictionary and re-encodes **every float
  column in every table** — ingested or not.

Gate A (the behavioural gate, Phase 1) tells you the day the goldens moved. Gate B tells you the day
the *risk* was introduced, on the PR that introduced it.

## When the gate fails

A changed snapshot is **not automatically wrong**. It is a *deliberate corpus event*, and the PR
must account for it. Regenerate from the repo root inside `nix develop`:

```bash
scripts/feature-snapshots.sh tessera/tests/feature-snapshots
```

Then either:

- show that no golden moved (the conformance corpus is unchanged) and state in the PR **why** the
  feature graph moved and why it is safe; or
- carry the corpus regeneration in the same PR.

Reviewing the diff is the job. Treat it exactly like a change to `corpus/corpus.json`.

## Known gap

The crate list in `scripts/feature-snapshots.sh` is hand-maintained. Adding a codec crate to the
seal path without adding it there leaves a blind spot; ADR-0057 records deriving the list from the
dependency graph as an open gap.
