---
type: issue
state: closed
created: 2026-09-29T13:06:47Z
updated: 2026-09-29T16:08:55Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/518
comments: 1
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-30T07:57:06.361Z
---

# [Issue 518]: [ci(nix): cargoArtifacts never codegens test-target deps — nextest recompiles 96 crates deps already built](https://github.com/vig-os/tessera/issues/518)

## The finding

On a green CI leg (#500, run 36543170354, x86_64), `tessera-deps` compiled **535** crates. `tessera-nextest` — which inherits those `cargoArtifacts` — then compiled **194** more:

| cohort | count | examples |
|---|---|---|
| same name **and** version as something `tessera-deps` already compiled | **96** | `arrow-array 58.3.0`, `chrono 0.4.45`, `dicom-object 0.9.1`, `ahash 0.8.12` |
| not built by `tessera-deps` at all | 59 | `datafusion 54.0.0`, `arrow-csv/json/row`, `comfy-table`, `bigdecimal`, `chrono-tz` |
| workspace crates (expected) | 14 | `tessera-*` |

That makes `workspace-test` **~96% compilation**: 58.5 minutes wall-clock, of which test execution is ~30 seconds across three batches (last `Compiling` at 09:52:57 against a 09:55:05 end).

## The cause

From the generated derivations, `buildDepsOnly` and `cargoNextest` run materially different commands:

```
tessera-deps:     cargo check --locked --all-targets     <- METADATA (.rmeta) for every target
                  cargo build --locked                   <- CODEGEN for lib/bin targets only
tessera-nextest:  cargo nextest run                      <- must LINK real test binaries
```

Confirmed against crane's own source (`lib/buildDepsOnly.nix`):

```nix
cargoCheckExtraArgs = args.cargoCheckExtraArgs or (if doCheck then "--all-targets" else "");
cargoBuildExtraArgs ? ""            # <- the build step gets NO --all-targets
buildPhaseCargoCommand =
  ${cargoCheckCommand} ${cargoExtraArgs} ${cargoCheckExtraArgs}
  ${cargoBuildCommand} ${cargoExtraArgs} ${cargoBuildExtraArgs}
```

So the dependency graph reachable **only from test targets** is type-checked but never compiled to `.rlib`. `cargo nextest` links real test binaries, so it must codegen all of it — which is exactly the two cohorts above: the 96 were checked but never built, and the 59 are dev-dependency-only (`datafusion` pulls `comfy-table`, `arrow-csv/json/row`, `bigdecimal`).

## The fix

```nix
cargoArtifacts = craneLib.buildDepsOnly (
  commonArgs // { cargoBuildExtraArgs = "--all-targets"; }
);
```

Verified at the derivation level — the deps build step becomes `cargo build --locked --all-targets`.

This makes `tessera-deps` **slower** and `workspace-test` **faster**. The trade should be favourable because ~14 derivations inherit these artifacts and only one was paying the rebuild, but both directions are measured before this lands.

## Why it matters beyond one check

`nextest` is the **serial floor** for every CI parallelism design: `deps (19.4) + nextest (58.5) = 77.9 min` against an 84.8-minute leg, which is why #517 measured a maximum gain of 6.9 minutes from *any* job split. Lowering this floor is worth more than the split and the cache combined, and it bounds both of them.

## Verification required before merge

- `nix flake check` green on both arches, with before/after durations for `tessera-deps` and `workspace-test`.
- **Sealed bytes unchanged** — `seal-profile-determinism` (dev vs release corpus) and Gate B feature snapshots must both still pass. A change to how dependencies are compiled must not move any content hash; if it does, that is a format decision and stops here.
- Peak memory recorded (deps doing more codegen could raise it).

Refs: #517, #495

---

# [Comment #1]() by [gerchowl]()

_Posted on September 29, 2026 at 04:08 PM_

Measurement artifact, not a bug — see #519's closing comment and #535. Three checks shared the derivation name `tessera-nextest-0.0.0`, so their CI log lines were counted as one build.

