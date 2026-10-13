# HANDOFF — ADR-0059 / #522 (chunk-index schema v2), PR #539

Written under an imminent power cut. Everything is **committed and pushed**; the working tree is clean.

- **Branch:** `feat/347-chunk-index-schema-v2`
- **PR:** #539 (**DRAFT — do not un-draft or merge until C4–C7 are all in it**)
- **Pushed tip:** `87ae563`
- **Worktree:** `/home/larsgerchow/.flock/worktrees/tessera-46753ae8/fix-416-adr0058-streaming-and-inspect`
- **Orchestrator:** `worktree-rapid-stone-0736-ff` (name changes when it restarts — re-check with `ListAgents`)

## Where we are

ADR-0059 is **ratified** and merged to dev (PR #531). Implementing its §11 as a seven-commit
sequence, all in #539. **C1–C4 are done and pushed. C5 is next and has not been started.**

| | | state |
|---|---|---|
| C1 | `9e7c4b2` fail closed when chunk statistics cannot be exact (M2 / #523) | done |
| C2 | `fc2ea41` schema v2 — S2 counters, histogram type, `indexed_digest`, M3 native-LE digest | done |
| C3 | `dc5137f` build the block histogram + the determinism gate | done |
| — | `4cc96b0` review round 1: 3 blockers + 5 nits | done |
| — | `a6440e9` review round 2: 5 ADR/doc nits | done |
| — | `f50f976` strictly equal-width bins (owner decision) | done |
| — | `41b328c` round 3: 4096 normative, spec `exact` dropped, 4 nits | done |
| **C4** | `b68db27` **THE FORMAT EVENT** — default-on + corpus regenerated | done, accepted |
| — | `87ae563` index every array-producing path + coverage guard | **done, NOT re-verified — see below** |
| **C5** | reader enforcement | **NEXT — not started** |
| C6 | optional per-block sha256 co-digest (#526), default off, + `sha2` into Gate B's crate list | not started |
| C7 | flip ADR-0059 to Accepted, record the ratification | not started |

## FIRST THING TO DO ON RESUME

`87ae563` was committed with `--no-verify` and **the full workspace was not re-run after it**.
The guard test itself was 4/4 green, but the four path flips it accompanies (`raw.rs`, `nifti.rs`,
`canonical.rs`, `nav.rs` pyramid) came after the last full-workspace run. So:

```sh
cd tessera
export CARGO_TARGET_DIR=/mnt/HDD/larsgerchow/tessera-target-tsra-416
export SCCACHE_DIR=/mnt/HDD/larsgerchow/sccache
cargo test --workspace          # expect fallout in ingest/cli tests that assert block COUNTS
cargo clippy --workspace --all-targets
prek run --files $(git diff --name-only HEAD~1 | tr '\n' ' ')
```

**Expected fallout:** any test asserting a product has N blocks now sees N+1 for an integer array
(this is exactly what happened in `dicom.rs`, where two tests needed updating). Also check whether
`tessera pyramid`'s trycmd/doc output changed — the pyramid now emits a `.cidx` per level.
Then re-run `nix build -L .#checks.x86_64-linux.seal-profile-determinism` and check #539's CI.

Everything must be **signed** (`-S`; never `-c commit.gpgsign=false`) and must not use
`--no-verify` once there is time to run the gates.

## C5 — the next commit (requirements are fixed, from ADR-0059 §11 step 3)

Reader-side enforcement, landing **in the same PR as C4** so dev never sees a digest that is
written but unenforced:

1. Verify `recipe` — an `@1` index must not be read as `@2`.
2. Verify `indexed_digest` against the data block's own `BlockRef.digest`; refuse on mismatch.
3. Validate the payload rather than trusting it (`ChunkIndex::from_bytes` validates **nothing**):
   reject, treating the index as **absent**, when `lo > hi`, `counts` is empty,
   `counts.len() != bins`, `width == 0`, or `width`/`bins` do not match the §5 layout recomputed
   from `lo`/`hi` with the **normative 4096** budget. `exact` is derived from `width == 1` — there
   is no `exact` on the wire.
4. The §5 rule: hist `spec` vs payload disagreement ⇒ corrupt ⇒ fall back as if absent.
5. Serve quantiles/median/mode, skew/kurtosis, Tukey and k·σ outlier counts **from the histogram**,
   through `stats --json`, each with its own `exact`/`method`.
6. Update the `multiblock.rs` consumer for v2.

## Facts worth not rediscovering

- **Corpus moved in C4, exactly 3 of 8 fixtures** (accepted): `recon_int16`, `multiblock_study`,
  `segmentation_uint8`. `id` unchanged for all three. The old→new hashes are in `b68db27`'s body.
  `segmentation_uint8` is the good evidence case: uint8 + bool got sidecars, float16 did **not**.
- **One golden moved outside the corpus** (accepted): the DICOM ingest golden,
  `b11c199c… → 77bc2790…`, old value kept in a comment. Must be named in #539's body next to the
  corpus moves — **this is an outstanding request from the orchestrator and is NOT done yet.**
- **#539's body still describes C1–C3 only.** It needs updating for C4 onward before un-drafting.
- **A new test file is invisible to every nix check until `git add`ed** — flakes copy only tracked
  files. Cost an hour once; `cargo test` finds it locally while the gate says
  `no test target named …`.
- **Reading a CI log:** check byte count and that first/last timestamps span the reported duration
  before grepping. `gh api .../jobs/<id>/logs` can return a 99-byte error body that greps clean;
  prefer `gh run view --job <id> --log` after the run completes.
- The determinism gate is wired into `seal-profile-determinism` (dev **and** release), and its
  aarch64 leg was confirmed from the log — not inferred.
- Deliberate exclusions from default-on, already documented: `array_block`/`array_job` primitives,
  `tessera bench`, `commit --add-block` (copies, does not build).
- **#554** (i64 > 2^53 as a JSON number) is filed separately — **do not fold it in.**
- Disk: root is ~93% full. Build only into `/mnt/HDD/larsgerchow/tessera-target-tsra-416`.
