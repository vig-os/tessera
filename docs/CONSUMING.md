# Consuming Tessera (pre-release)

Tessera is pre-1.0 and iterates on the `dev` branch; the first release (`0.1.0-alpha.1`) is deliberately
**held** (see `release-plz.toml`). Until it is cut, Tessera is **not** published to crates.io / PyPI /
npm — consume it directly from git or the Nix flake. All three paths below are verified working.

> **Point at `dev`, not `main`.** The flake packages and the library crates live on `dev`; `main` is
> release-only and far behind, with none of this yet. Pin a specific commit (or, once the alpha is cut,
> the `v0.1.0-alpha.1` tag) for reproducible builds — `dev` is a moving target.

## Run the CLI (no install)

```bash
nix run github:vig-os/tessera/dev#tessera -- inspect study.tsra
```

## Add Tessera to your flake

```nix
{
  inputs.tessera.url = "github:vig-os/tessera/dev";   # or pin ?rev=<commit>
  outputs = { self, nixpkgs, tessera }:
    let
      system = "x86_64-linux";
      tsr = tessera.packages.${system};
    in {
      packages.${system}.default = tsr.tessera;   # the CLI as your package / devShell PATH
      packages.${system}.wheel = tsr.wheel;       # the reproducible Python wheel
    };
}
```

The flake exposes `packages.<system>.{tessera,tessera-cloud,wheel}` and `apps.<system>.tessera`
(`nix run`). Nix supplies the whole runtime closure (including libhdf5), so a flake consumer needs no
system native dependencies.

## Depend on the library crates (Rust)

```toml
[dependencies]
tessera-core = { git = "https://github.com/vig-os/tessera", branch = "dev" }
tessera-io   = { git = "https://github.com/vig-os/tessera", branch = "dev" }
```

The cargo workspace is nested under `tessera/`; plain cargo `git` dependencies discover the member
crates fine (no root `Cargo.toml` needed). Native build dependencies a Rust consumer inherits:

- `tessera-core` — pure Rust, no native deps.
- `tessera-io` — pulls the Vortex stack, so it needs `libclang` (bindgen) at build time.
- `tessera-ingest` — additionally links `libhdf5` via `pkg-config`, or build with
  `--features static-hdf5` to vendor HDF5 (needs CMake + a C compiler).

The Nix dev shell provides all of these; outside Nix, install them yourself.

## What isn't wired yet

crates.io, PyPI, and npm publishing are intentionally deferred until the format settles. When wired,
`cargo add tessera-io` / `pip install tessera` will work; until then, use the git/flake refs above.
