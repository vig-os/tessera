# Contributing

Tessera's development setup, gates and conventions are described in [`tessera/CLAUDE.md`](tessera/CLAUDE.md)
("Dev environment") and in the [book](docs/book/src/SUMMARY.md). This file covers how to test a change.

## The gate

The merge gate is `nix flake check -L --keep-going` at the repository root, which CI runs on every pull request.
A single check runs locally with `nix build -L .#checks.x86_64-linux.<attr>` (`workspace-test`,
`workspace-clippy`, `sql-tests`, ...).

## The changelog

A user-facing change adds its entry to the root `CHANGELOG.md`, under the right subhead of `## Unreleased`
(`Added`, `Changed`, `Removed`, `Fixed`, `Security`; `Format break` for an on-disk format break). Never edit a
released `## [<version>]` section: it is that release's notes. The `changelog-sections` check
(`scripts/changelog.py check`) holds the file's structure. The release-time step that turns `Unreleased` into the
version's section is in [`docs/RELEASING.md`](docs/RELEASING.md), with how the GitHub Release notes are taken from
the file.

## Testing one crate on ksb (maintainers and agents)

With access to the ksb batch cluster, the way to build and test one crate without compiling on your own machine
is `scripts/ksb-test.sh` (#957):

```sh
scripts/ksb-test.sh HEAD tessera-core                     # the crate's whole nextest suite
scripts/ksb-test.sh HEAD tessera-cli --bins               # cargo target/feature flags pass through
scripts/ksb-test.sh my-branch tessera-io 'test(roundtrip)'  # a nextest filterset (-E)
scripts/ksb-test.sh --help                                # every option, the exit codes, the cache rules
```

It takes a commit in your clone, pushed or not, whose flake exports `legacyPackages.ksbTest` (#960 or later).
Older commits exit 3 (`attribute 'ksbTest' missing`). It exits with the test run's own exit code (0 pass, 100 a test
failed, 4 nothing matched the filterset, 124 the `--timeout` expired), or 3 when the job produced no test result
(an evaluation or compile error, whose log it prints). It prints nextest's summary line and, when a test fails,
each failing test's output. `junit.xml`, `nextest.log`, `summary.txt` and `failures.txt` stay in the job's
output (`ksb out <job> --path junit.xml --dest FILE`).

ksb labels any exit 124 as `reason: deadline`, including the test `--timeout`. Inspect `summary.txt` and
`nextest.log` to identify a test timeout. An infrastructure deadline during preparation produces no test result
and the wrapper exits 3, printing ksb's reason and log.

**How.** The script writes a small job flake that pins the commit and the Cargo.lock git dependencies and calls
`legacyPackages.ksbTest` of this repository's `flake.nix`. That builds the crate's test binaries in one crane
derivation and runs them under nextest in a second, which records the outcome instead of failing; the job's app
copies the record to the job output and exits with the recorded code. Nothing compiles in the job pod.

**Measured** on ksb, 2026-11-22/23, wall time from submit to printed result. ksb's own overhead (copying the
inputs, evaluating, starting the pod) is about 2-4 minutes of every new job.

| run | what is built | `tessera-core` | `tessera-io` | `tessera-cli --bins` |
| --- | --- | --- | --- | --- |
| cold | the dependency build, then the crate | 308 s | | |
| warm, new commit | the crate only (dependency build cached) | 391 s | 388 s | 306 s |
| `--rerun` | nothing (the tests run again) | 234 s | 325 s | 262 s |
| the same request again | nothing (the stored result) | 1 s | 1 s | 1 s |

(Cold: 2a4e81c5. Warm: 7a891383, dev at #942 plus this change; the three ran at once, and `tessera-core`'s
schema-reference drift test runs `cargo run`, which compiles its example in the dev profile. `--rerun`: 52a5e2a7.)

**Caching.** The same request on the same commit returns the stored result without running anything, so a
deterministic failure stays failed; `--rerun` runs the tests again (for a suspected flake) without rebuilding.
The test-binary build is shared by every filter on a commit, and the dependency build by every commit with the
same `Cargo.lock`.

**What differs from `workspace-test`.** One crate (`-p`); 4 test threads (`--threads`) instead of the build's
core count; `fail-fast = false`, so one run reports every failure; a JUnit report; a test-run timeout. The same
toolchain, source filter, native dependencies, release profile and RAM-backed `TMPDIR`.

**Known gaps.**

- Tests that need the private DUPLET fixtures cannot run: they are not a ksb dataset (`dataset_refused`).
- Nothing that counts instructions or reads hardware counters works: the job seccomp profile has no
  `perf_event_open`.
- The tests run on the ksb builder, not in the job pod, so the job's `--cpu`/`--mem` do not bound them. The thread
  count is bounded; memory is not, so a test's RSS is not limited the way a 16 GiB laptop would limit it (#961).
- The job replaces the dev-shell inputs (`vigos`, `guardrails`) with an empty stub, because ksb copies every
  locked input to the compute VM first and those ~300 MB never reach the Rust build. A test that needed them would
  fail evaluation, not run against something else.
