---
type: issue
state: open
created: 2026-10-05T00:05:41Z
updated: 2026-10-05T00:05:41Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/545
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-05T08:17:42.662Z
---

# [Issue 545]: [flake: eachDefaultSystem still includes x86_64-darwin, which pinned nixpkgs 26.11 dropped — --all-systems eval fails](https://github.com/vig-os/tessera/issues/545)

`nix flake check --no-build --all-systems` fails immediately on current `dev`:

```
error: Nixpkgs 26.11 has dropped support for x86_64-darwin.
```

The flake uses `flake-utils.lib.eachDefaultSystem`, which still includes `x86_64-darwin`, while the pinned nixpkgs no longer supports it. Evaluation of that system's outputs therefore throws.

**Why it hasn't been noticed:** CI only ever evaluates `x86_64-linux` and `aarch64-linux` (each leg runs `nix flake check` for its own system), and `nix flake check` without `--all-systems` evaluates only the current system. The breakage is invisible until someone evaluates every system — which is exactly what a whole-flake evaluation step would want to do. Found while designing #517.

**Options:** replace `eachDefaultSystem` with an explicit system list that matches what nixpkgs supports (`x86_64-linux aarch64-linux aarch64-darwin`), or drop darwin entirely if no darwin consumer exists. Small, but it changes the flake's output schema, so it's a separate decision rather than something to slip into a CI PR.

