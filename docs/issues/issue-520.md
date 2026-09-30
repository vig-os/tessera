---
type: issue
state: open
created: 2026-09-29T13:18:41Z
updated: 2026-09-29T17:58:40Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/520
comments: 1
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-30T07:57:05.865Z
---

# [Issue 520]: [ci: the ecosystems bench tests gate nothing — /bench/ is not in the crane src filter (same bug as ef240cb)](https://github.com/vig-os/tessera/issues/520)

## The gap

`tessera/bench/ecosystems/` is referenced by **nothing**:

- no `flake.nix` check
- no `.github/workflows/*.yml`
- no `.pre-commit-config.yaml` hook
- no `justfile` recipe

And it could not be one even if it were referenced, because the crane `src` filter (`flake.nix:50-63`) admits `/corpus/`, `/docs/examples/`, `/docs/dictionaries/`, `/tests/cmd/`, `/tests/feature-snapshots/` and `/tests/derived-docs/` — **but not `/bench/`**. Those files never reach the build sandbox.

So `test_common.py` and `test_adapters.py` (#485) — 57 assertions guarding the harness's fairness properties — **pass locally and gate nothing**. They run only when someone remembers to invoke pytest by hand.

## This is a repeat of a bug already fixed in the same file

`ef240cb` fixed the identical failure for trycmd: the docs-as-tests cases *ran zero cases in CI* because `.trycmd`/`.in/` were not in the crane src filter, and the fix was to add `/tests/cmd/` to exactly this list. Same file, same mechanism, a different path.

That it recurred is the argument for a structural guard rather than another one-line path addition: nothing currently notices when a test directory is written that the sandbox cannot see, and the symptom is silence.

## Why it matters here specifically

The #485 audit found **five** fairness defects in this harness — HDF5/NeXus missing `shuffle`, Parquet missing `BYTE_STREAM_SPLIT`, Zarr missing shuffle, the ROOT `TTree`/`RNTuple` mislabel, and NIfTI reads returning a lazy `memmap` instead of data. Every one of them produced *plausible-looking numbers*. The tests written to stop them recurring are precisely what is not running.

## Options

1. **Cheap, now:** add a flake check running `test_common.py` only — pure Python + numpy, no ecosystem libraries, no `tessera-py`. Guards the median/spread logic, the cold-cache eviction machinery and both fixtures. Needs `/bench/` in the src filter.
2. **Complete, later:** also run `test_adapters.py`, which needs h5py, zarr, pyarrow, nibabel, pydicom, uproot **and** a built `tessera-py`. That is a heavy new derivation, and #495 is currently constrained on exactly that — a new `mkCargoDerivation` on #461 moved aarch64 headroom from 2.4 GB to 852 MB. It should wait for the CI memory work (#514/#517/#519) to land.
3. **Structural:** a guard that fails when a directory containing tests is not admitted by the src filter, so the next recurrence is loud. Prevents the `ef240cb` pattern from repeating a third time.

Recommend **1 now**, **2 once the memory work lands**, and **3** as the durable fix.

Refs: #485, #495

---

# [Comment #1]() by [gerchowl]()

_Posted on September 29, 2026 at 05:58 PM_

Recorded for the history: this is the **second occurrence** of this bug class. The first was `ef240cb` (trycmd docs-as-tests ran zero cases in CI because `/tests/cmd/` wasn't in the crane src filter). Because it recurred, the fix in the linked PR adds a structural `test-coverage` check that fails on any test file visible to no check, not just a path addition.

