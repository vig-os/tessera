---
type: issue
state: closed
created: 2026-08-20T09:00:10Z
updated: 2026-09-28T17:55:21Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/400
comments: 3
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:40.063Z
---

# [Issue 400]: [determinism: feature-unification can silently move sealed bytes — add the seal-path feature-snapshot gate (ADR-0057 Gate B)](https://github.com/vig-os/tessera/issues/400)

## Motivation

While probing #398 (ADR-0057) I checked a claim ADR-0056 §12 asserts but never proves:

> *"A feature may decide whether a format is **readable**; it may never change how one **encodes**."*

It holds today — but **only by luck, and some of the luck is already spent.**

A structural diff of resolved features (`cargo tree -p tessera-cli -e features`, default vs
`--features sql`) shows the optional, ingest-unrelated `sql` feature turning on features **inside the
shared arrow tree**:

```text
arrow-array  feature "chrono-tz"
arrow-schema feature "canonical_extension_types"
arrow-ipc    feature "zstd" / "lz4"
arrow-cast   feature "prettyprint"
```

`arrow-array/chrono-tz` is the one that matters. ADR-0056 §5 lists as hazard **H1**, rated
*high × fatal*:

> *"Timezone/timestamp normalisation — **tzdb is a filesystem dependency** (`chrono-tz` compiled-in vs
> host `/usr/share/zoneinfo`); two hosts, two `i64`s."*

So an unrelated optional feature **already** changes the timezone machinery compiled into the arrow
crate that ADR-0056's table lane will canonicalise through. It is inert only because nothing ingests
Arrow yet — it stops being inert the day #386 lands.

The same class of hazard exists on the Vortex side and is arguably worse: the workspace deliberately
pins `vortex-btrblocks = { features = ["pco"] }` to **exclude ALP** because ALP was not
byte-deterministic (#380 → #384). **Cargo feature unification is monotonic — it can only add.** Any
future dependency that transitively enables `vortex-btrblocks/alp` re-registers the ALP scheme in the
compressor dictionary and silently re-encodes **every float column in every table**, ingested or not.
Nothing in CI would catch that until the goldens moved, and a moved golden gets rationalised as
"expected, we added a feature".

This is independent of #386: it protects the **existing** corpus whether or not generic ingest ever
lands.

## Decision / proposed approach

Add the **structural** gate from ADR-0057 §5 ("Gate B"). It catches the *risk* on the PR that
introduces it, rather than the *symptom* on the release that ships it.

```bash
# snapshot the resolved features of every crate on the seal path
for c in blake3 zarrs pcodec zstd vortex-btrblocks vortex-file vortex-array vortex-buffer \
         arrow-array arrow-buffer arrow-schema; do
  cargo tree -e features -i "$c" --workspace --all-features | sort > tests/feature-snapshots/"$c".txt
done
git diff --exit-code tests/feature-snapshots/
```

A changed snapshot is **not automatically wrong** — it is a *deliberate corpus event*. The PR must
either show no golden moved (behavioural gate still green, snapshot updated with a stated reason) or
carry the corpus regeneration alongside. Same discipline ADR-0056 §5 prescribes for the `arrow` /
`parquet` `=` pins, applied one level deeper.

Pairs with the behavioural gate (ADR-0057 §5 "Gate A"): run `gen_corpus` under several feature
configurations and require byte-equality with `corpus/corpus.json`.

## What already exists

- `tessera/crates/tessera-io/src/conformance.rs` + `tessera/corpus/corpus.json` + the
  `tests/conformance.rs` assertion — the behavioural half is already built, it is just not run under
  multiple feature configurations.
- `cargo run -p tessera-io --example gen_corpus` regenerates the goldens.
- `flake.nix:249` runs clippy `--all-targets --all-features`; `flake.nix:252` runs nextest. Both are
  natural homes for a new check.
- Verified during the #398 probe: goldens are **byte-identical** across four configurations today
  (`-p tessera-io`; `+cloud`; a binary linking dicom+hdf5+nifti+raw; the same `+sql`), and all match
  the committed corpus. **This issue is about keeping that true, not about fixing a current break.**

## Scope

**P0**
- [ ] `tests/feature-snapshots/` committed against today's graph — capture the `chrono-tz` drift as the
      recorded baseline rather than discovering it later
- [ ] a flake check that regenerates and `git diff --exit-code`s the snapshots
- [ ] `cargo tree` is metadata-only (no compilation), so this must stay cheap — assert that in the check

**P1**
- [ ] Gate A: `gen_corpus` across `{-p tessera-io, +cloud, -p tessera-cli --features full, --all-features}`,
      all byte-equal to `corpus/corpus.json`
- [ ] CONTRIBUTING note: a snapshot change requires either an unchanged-golden demonstration or a
      corpus-regen commit in the same PR

**P2**
- [ ] derive the crate list from the dependency graph instead of hand-maintaining it (see pitfalls)

## Pitfalls

- **The crate list is hand-maintained**, so adding a codec to the seal path without adding it to the
  snapshot list leaves a silent blind spot. That is the gate's own weakest point and is why P2 exists.
- `cargo tree` output ordering must be stabilised (`| sort`) or the gate flaps.
- `--all-features` currently drags `static-hdf5` (builds HDF5 from source). `cargo tree` does not
  compile, so this should be fine — but verify the check does not accidentally trigger a build.
- A snapshot diff will fire on routine `cargo update`. That is correct behaviour, but it must be
  *cheap to review*, or people will rubber-stamp it — which would defeat the whole gate.
- Do not let the gate become "regenerate until green". The reviewable artifact is the *reason*.

## Acceptance criteria

- [ ] A PR that adds a dependency enabling `vortex-btrblocks/alp` fails CI **before** any golden moves
- [ ] The `sql` → `arrow-array/chrono-tz` drift is visible in a committed snapshot file
- [ ] Goldens proven byte-equal across the named feature configurations, in CI, on both arches
- [ ] The gate adds negligible wall-clock to `nix flake check`

## References

- ADR-0057 §5 (`docs/adr/0057-ingest-topology-thin-waist-and-release-matrix.md`) — Gates A and B, and
  the probe data behind them
- ADR-0056 §5 (hazard table, H1) and §12 (the unproven constraint)
- #380 / #384 — the ALP non-determinism this is designed to prevent recurring
- #398 (spike), #386 (generic ingest impl tracker)

Refs: #398
---

# [Comment #1]() by [gerchowl]()

_Posted on August 20, 2026 at 06:57 PM_

**P0 landed in #404** (ADR-0057 Phase 0), commit `test(ci): Gate B — the feature-snapshot determinism gate`.

- [x] `tests/feature-snapshots/` committed against today's graph — 11 crates
      (blake3, zarrs, **pco**, zstd, vortex-btrblocks, vortex-file, vortex-array, vortex-buffer,
      arrow-array, arrow-buffer, arrow-schema). The `chrono-tz` drift is captured as the recorded
      baseline: `arrow-array.txt` carries `arrow-array feature "chrono-tz"` (4 lines mentioning it).
- [x] a flake check that regenerates and diffs the snapshots — `nix build .#checks.<sys>.feature-snapshots`.
      Inside the nix sandbox there is no `.git`, so the hermetic equivalent of `git diff --exit-code`
      is: regenerate into a temp dir and `diff -ru` against the committed baseline. Same semantics,
      same failure.
- [x] assert it stays cheap — the derivation is `craneLib.mkCargoDerivation` with
      `cargoArtifacts = null` and `doInstallCargoArtifacts = false`: it has no target directory to
      inherit and issues no `cargo build`, so it *cannot* compile the workspace. Full check runs in
      seconds.

Naming note: §5 and this issue spell the entry `pcodec`, which is the project name — the crate in
`Cargo.lock` is `pco`, so that is what `cargo tree -i` is given. Same codec, the one the workspace
reaches through `vortex-btrblocks/pco`.

On the pitfalls this issue raised:

- *"`cargo tree` output ordering must be stabilised or the gate flaps"* — `| sort` under `LC_ALL=C`,
  plus `--charset ascii`, `--locked` and `--offline`. Beyond that, **`--target all`**: `cargo tree`
  otherwise resolves `cfg()` against the host triple, and CI runs on x86_64-linux *and*
  aarch64-linux, so a target-gated dependency present on one and absent on the other would fail the
  gate on one arch for no determinism reason. All 11 snapshots verified byte-identical under host /
  `--target aarch64-unknown-linux-gnu` / `--target all`, so this changes nothing today and keeps it
  that way by construction. Two further normalisations: absolute path-dependency paths are stripped
  (they differ between a checkout, a worktree and the sandbox's `/build/source`), and
  workspace-member versions are pinned to `vWORKSPACE` so a release-plz bump is not a Gate B event.
- *"`--all-features` currently drags `static-hdf5` … verify the check does not accidentally trigger a
  build"* — verified: the build log is `wrote 11 feature snapshots` and nothing else. Note this is
  now the *only* PR-path use of `--all-features`, since #402 (same PR) took it out of clippy — which
  is deliberate: Gate B keeps watching the corner of the feature space clippy no longer builds.
- *"do not let the gate become 'regenerate until green' — the reviewable artifact is the reason"* —
  `tessera/tests/feature-snapshots/README.md` says exactly that, and the check's failure message
  spells out the two acceptable resolutions (show no golden moved and state why, or carry the corpus
  regeneration in the same PR).

Acceptance criteria status:

- [x] The `sql` → `arrow-array/chrono-tz` drift is visible in a committed snapshot file.
- [x] The gate adds negligible wall-clock to `nix flake check` (no compilation).
- [x] A dependency enabling `vortex-btrblocks/alp` fails CI before any golden moves — **negatively
      tested**: appending a fake `vortex-btrblocks feature "alp"` line to the baseline fails the check
      with the diff and the regeneration instructions.
- [ ] Goldens proven byte-equal across the named feature configurations — that is **Gate A (P1)**,
      Phase 1 of the ADR's landing plan. Not in #404.

P1 (Gate A + the CONTRIBUTING note) and P2 (deriving the crate list from the dependency graph)
remain open. The hand-maintained crate list is still the gate's weakest point, as this issue says.

---

# [Comment #2]() by [gerchowl]()

_Posted on August 20, 2026 at 07:04 PM_

Phase-1 follow-up (from #404 review, non-blocking): commit a scripted negative self-test — `scripts/feature-snapshots.sh --check-drift` that appends a fake feature line and asserts the gate fails — so the negative test is in-tree, not just asserted in the PR body. The current `diff -ru` check is non-vacuous; this is an audit-trail hardening.

---

# [Comment #3]() by [gerchowl]()

_Posted on September 28, 2026 at 05:55 PM_

Closing as **done** — verified on `origin/dev` in the 2026-09-28 backlog triage.

Evidence: commit 2b7821c 'ADR-0057 Phase 0: `tessera info` + Gate B + drop static-hdf5 from the PR clippy matrix (#404)' — Gate B feature-snapshots landed under tessera/tests/feature-snapshots/.

https://claude.ai/code/session_01XdERKMVDAwfMJSKdTytNnK

