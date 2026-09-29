---
type: issue
state: closed
created: 2026-08-20T09:00:39Z
updated: 2026-09-28T17:55:24Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/401
comments: 3
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:39.727Z
---

# [Issue 401]: [AX: `tessera info` — list compiled-in backends, decoder versions and build-mode flags](https://github.com/vig-os/tessera/issues/401)

## Motivation

There is no way to ask a `tessera` binary what it can do. `--version` prints a bare version string,
and the versioning module (`crates/tessera-cli/src/version.rs`) is the CoW repository code, not a
`--version` surface.

Today that is merely inconvenient. ADR-0057 makes it load-bearing: once ingest backends are
feature-gated, **"which decoders are in this binary" becomes a build-time fact that the binary is the
only thing that knows.** Every "why did this fail?" then costs a support round-trip that a single
command should have answered. The clinical-operator review of #398 named this as the precondition for
a diagnosable failure — the person hitting the error is usually not the person who built the binary.

It also has a second consumer: ADR-0056 §6 seals `ingest_decoder` (decoder name + version), and
ADR-0057 §7 puts the *flavor* and the full backend list in `aux/provenance.json`. A machine-readable
form of this output is what populates that.

This is independent of #386 — it is useful the day it lands, and it is a straight AX gap of the kind
the #390 onboarding audit was scoring.

## Decision / proposed approach

A new read-only verb, `tessera info`, plus a `--json` form.

```text
$ tessera info
tessera 0.1.0-alpha.1  (format tessera_version v0)
backends:   arrow 58.3.0 · parquet 58.3.0 · npy · nifti · raw · blob · dicom 0.9.1 · hdf5 1.14.6
disabled:   csv · tiff
build:      static-hdf5=yes · cloud=yes · sql=no
```

Backed by two constants that ADR-0057 §6 already requires for the spec engine, so this is mostly
surfacing state that must exist anyway:

- `BACKENDS_ALL` — every backend name, on every build (the total set)
- `BACKENDS_ENABLED` — feature-gated pushes (the compiled-in subset)

`--json` emits the same content in a form suitable for embedding into `aux/provenance.json` and for
scripting.

## What already exists

- `crates/tessera-ingest/src/spec.rs:137` — `FormatOptions`, the closed `#[serde(tag = "format")]`
  enum whose variant names ARE the backend name set (ADR-0056 §4: `--from <name>` and TOML
  `format = "<name>"` are one string set)
- `crates/tessera-cli/src/main.rs` — clap command tree to hang the verb off
- Cargo features already present and worth reporting: `cloud`, `sql`, `static-hdf5`
- `tests/cmd/*.trycmd` — the docs-as-tests harness this verb should be covered by

## Scope

**P0**
- [ ] `tessera info` — version, format version, enabled backends, disabled backends, build-mode flags
- [ ] `BACKENDS_ALL` / `BACKENDS_ENABLED` constants, one grep locating every backend
- [ ] a `.trycmd` case so the output is docs-as-tests covered

**P1**
- [ ] `tessera info --json`
- [ ] decoder **versions** not just names (`dicom 0.9.1`, `arrow 58.3.0`) — sourced from the same place
      ADR-0056 §6's sealed `ingest_decoder` will read
- [ ] wire the JSON form into `aux/provenance.json` at ingest (ADR-0042 / ADR-0057 §7)

**P2**
- [ ] `tessera --version --verbose` as an alias, for the muscle memory

## Pitfalls

- **The `.trycmd` snapshot will contain version numbers**, so it churns on every dependency bump.
  Either filter versions out of the asserted output or use trycmd's redaction — otherwise this becomes
  a permanent source of unrelated diff noise. (Related trap already hit in this repo: trycmd cases
  silently ran **zero** tests because the fixtures were not in the crane source filter.)
- The output must stay honest when features are off; a backend listed as enabled that then errors at
  dispatch is worse than no command at all. Drive both lists from one source.
- Do not seal this into the manifest. ADR-0057 §7 is explicit: flavor did not change the bytes, so it
  belongs in `aux/`, not the seal. Sealing it would invite the false inference "same flavor ⇒ same bytes".
- Resist making it a general "capabilities" dumping ground — the value is that it is short enough to
  paste into a bug report.

## Acceptance criteria

- [ ] `tessera info` on a build with `--no-default-features` correctly reports the reduced backend set
- [ ] The error text for a not-compiled-in backend (ADR-0057 §7) references `tessera info`, and that
      reference resolves to a real command
- [ ] `.trycmd` coverage that does not churn on dependency bumps
- [ ] Output is short enough to paste into an issue verbatim

## References

- ADR-0057 §7 (`docs/adr/0057-ingest-topology-thin-waist-and-release-matrix.md`) — mandates this surface
  and the two-exit-code missing-backend errors it pairs with
- ADR-0056 §4 (one verb axis, `--from <backend>`) and §6 (sealed `ingest_decoder`)
- ADR-0042 — sealed vs `aux/` boundary
- #390 — the onboarding/AX audit that scored operator-verb gaps; #398 (spike); #386 (impl tracker)

Refs: #398
---

# [Comment #1]() by [gerchowl]()

_Posted on August 20, 2026 at 06:58 PM_

**P0 + P1 (except the `aux/provenance.json` wiring) landed in #404** (ADR-0057 Phase 0), commit
`feat(cli): tessera info`.

```
$ tessera info
tessera 0.1.0-alpha.1  (format tessera_version 0.0.0)
backends:   blob · dicom 0.9.1 · dicom-series 0.9.1 · hdf-compound 1.14.6 · nifti · raw
disabled:   (none)
build:      static-hdf5=no · cloud=no · sql=no
```

**P0**
- [x] `tessera info` — version, format version, enabled backends, disabled backends, build-mode flags
- [x] `BACKENDS_ALL` / `BACKENDS_ENABLED` in `tessera-ingest::backends`, one grep locating every
      backend. Plus `backend_name(&FormatOptions)`, an exhaustive match, so adding a variant without
      naming it is a compile error.
- [x] a `.trycmd` case (`tests/cmd/info.trycmd`), covering both the human and `--json` forms

**P1**
- [x] `tessera info --json`
- [x] decoder versions, not just names — from the two places that actually know: `hdf-compound`
      reports the libhdf5 **actually linked** (runtime query — that number differs between a nix build
      and a `static-hdf5` release binary, which a compile-time pin could not distinguish), and `dicom`
      reports the pin `build.rs` reads out of the workspace `Cargo.lock`. The lockfile scan is
      deliberately dependency-free: a `[build-dependencies]` entry would move the resolved feature
      graph, i.e. trip Gate B (#400), which is a disproportionate price for reading one string.
- [ ] wire the JSON form into `aux/provenance.json` at ingest — still open; it belongs with the
      ingest path, not with the verb.

On the pitfalls this issue raised:

- *"the `.trycmd` snapshot will contain version numbers, so it churns on every dependency bump"* —
  every version in `info.trycmd` is wildcarded with `[..]`. The *semantics* are pinned by unit tests
  instead: build modes are asserted against `cfg!` rather than hard-coded strings, so the human line
  cannot claim a feature the binary does not have.
- *"the output must stay honest when features are off — drive both lists from one source"* — both
  lists live in `tessera-ingest::backends`, and `BACKENDS_ENABLED` is written in exactly the shape
  Phase 1 needs (`#[cfg(feature = "…")]` on each element), so the gate and the report cannot disagree.
- *"do not seal this into the manifest"* — not sealed. ADR-0056 §6.2 seals `ingest_decoder`; the
  flavor goes to `aux/`, per ADR-0042.
- *"resist making it a capabilities dumping ground"* — four lines, pasteable verbatim.

Acceptance criteria status:

- [x] `.trycmd` coverage that does not churn on dependency bumps
- [x] Output is short enough to paste into an issue verbatim
- [ ] `tessera info` on a `--no-default-features` build reports the reduced backend set — the code
      path exists (`backends_disabled()` and the `disabled:` line), but there is nothing to disable
      until Phase 1's decoder features exist, so today it correctly reports `(none)`.
- [ ] The not-compiled-in backend error references `tessera info` — that error is ADR-0057 §7 /
      Phase 2. `tessera info` now exists for it to point at, which was the ordering constraint.

Judgment call: §7's example prints `(format tessera_version v0)`; this prints the literal value the
build stamps into a manifest (`0.0.0`), which is the number a reader can actually compare against.

P2 (`--version --verbose` alias) remains open.

---

# [Comment #2]() by [gerchowl]()

_Posted on August 20, 2026 at 07:04 PM_

Phase-1 follow-up (from #404 review, non-blocking): `build.rs lock_version()` returns the FIRST `[[package]]` matching a decoder name; harden with an "expected exactly one" assertion so a future dup/rename in Cargo.lock can't silently pin the wrong version (`option_env!` currently masks this at read time).

---

# [Comment #3]() by [gerchowl]()

_Posted on September 28, 2026 at 05:55 PM_

Closing as **done** — verified on `origin/dev` in the 2026-09-28 backlog triage.

Evidence: commit 2b7821c added tessera-cli/src/info.rs + tests/cmd/info.trycmd (#404).

https://claude.ai/code/session_01XdERKMVDAwfMJSKdTytNnK

