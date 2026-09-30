---
type: issue
state: closed
created: 2026-09-29T16:07:20Z
updated: 2026-09-29T17:28:43Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/535
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-30T07:57:00.654Z
---

# [Issue 535]: [ci(nix): three checks share the derivation name tessera-nextest, so their CI logs are indistinguishable](https://github.com/vig-os/tessera/issues/535)

## The defect

Three `nix flake check` derivations share one name:

```
workspace-test    -> tessera-nextest-0.0.0   (default features, whole workspace)
sql-tests         -> tessera-nextest-0.0.0   (-p tessera-cli --features sql)
minio-range-read  -> tessera-nextest-0.0.0   (-p tessera-io -p tessera-cli --features tessera-cli/cloud)
```

All three are `craneLib.cargoNextest`, which defaults `pnameSuffix` to `-nextest`, and none overrides it. The other 20 checks all have distinct names; this is the only collision (verified by evaluating `.#checks.x86_64-linux` and grouping by `.name`).

`nix build -L` / `nix flake check -L` prefix every log line with the derivation **name**, so the three are **indistinguishable in CI logs**. Every `tessera-nextest>` line is an unlabelled interleaving of three different builds.

## What it caused — measured wrong, and acted on

This is not cosmetic. I measured "nextest" from CI logs across #517/#518/#519 and every conclusion pooled the three:

| claim | where | what it actually was |
|---|---|---|
| nextest recompiles 96 crates `tessera-deps` already built | #518 | `sql-tests`/`minio-range-read` rebuilding arrow-*, chrono etc. **with the extra features** DataFusion and the cloud stack unify in — a different artifact, legitimately rebuilt |
| nextest compiles 59 dev-only crates (datafusion, comfy-table…) | #518 | `sql-tests` compiling the DataFusion tree **by design** — its own comment says default features "exclude the ~200-crate DataFusion tree" |
| nextest takes 58.5 min and is the serial floor | #517 | first-line-to-last-line span across **three** derivations, not one derivation's duration |
| nextest is ~96% compilation, ~30 s of test execution | #517 | three `Summary` lines from three different test runs |
| `cargoBuildExtraArgs = "--all-targets"` fixes the rebuild | #519 | fixed nothing — there was nothing to fix; its CI moved no number |

`cargo tree --workspace -e normal,dev -i datafusion` returns *"did not match any packages"* — DataFusion is not in the default-feature graph at all, so `workspace-test` cannot have compiled it. That is what finally exposed the pooling.

## Fix

Give each a distinct `pnameSuffix` so its log lines are attributable:

- `workspace-test`   → `tessera-nextest`         (unchanged)
- `sql-tests`        → `tessera-nextest-sql`
- `minio-range-read` → `tessera-nextest-cloud`

No behaviour or output changes — only the derivation names, which are test derivations whose outputs nothing consumes. Plus a guard that fails if two checks ever share a derivation name again, since the failure mode is silent: nothing errors, the logs just lie.

Then re-measure `workspace-test` on its own and correct #517.

Refs: #517, #518, #519

