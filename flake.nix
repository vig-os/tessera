{
  description = "fd5 / Tessera — FAIR data-product format · Rust + Python dev environment, governed by guardrails";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";

    # Pinned Rust toolchain (reads tessera/rust-toolchain.toml).
    rust-overlay.url = "github:oxalica/rust-overlay";
    rust-overlay.inputs.nixpkgs.follows = "nixpkgs";

    # Hermetic Rust builds for the flake checks (vendored deps → offline build/test).
    crane.url = "github:ipetkov/crane";

    # Shared code-quality / agent-drift governance: prek (Rust pre-commit), the gate
    # toolbelt, and sccache. The devShell is built through its mkDevShell.
    guardrails.url = "github:gerchowl/guardrails";
    guardrails.inputs.nixpkgs.follows = "nixpkgs";
    guardrails.inputs.flake-utils.follows = "flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay, crane, guardrails }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) ];
        };
        # One source of truth for the compiler + components — see tessera/rust-toolchain.toml.
        rustToolchain = pkgs.rust-bin.fromRustupToolchainFile ./tessera/rust-toolchain.toml;

        # crane, pinned to our toolchain. The Rust workspace lives in ./tessera; deps are
        # vendored once (buildDepsOnly) so clippy/test/fmt run hermetically & offline in CI.
        craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;
        # Cargo sources + the conformance corpus (tests/conformance.rs reads corpus/corpus.json,
        # which cleanCargoSource would otherwise strip → the hermetic test can't find it).
        src = pkgs.lib.cleanSourceWith {
          src = ./tessera;
          filter = path: type:
            (craneLib.filterCargoSources path type) || (pkgs.lib.hasInfix "/corpus/" path);
          name = "source";
        };
        commonArgs = {
          inherit src;
          strictDeps = true;
          pname = "tessera";
          version = "0.0.0";
          # The Vortex table backend pulls a transitive dep (`custom-labels`) whose build script
          # runs bindgen (needs libclang) and links libstdc++. Provide them to every crane
          # derivation (deps/clippy/test) so the hermetic build matches the devShell env.
          nativeBuildInputs = with pkgs; [ clang ];
          buildInputs = with pkgs; [ stdenv.cc.cc.lib ];
          LIBCLANG_PATH = "${pkgs.libclang.lib}/lib";
        };
        cargoArtifacts = craneLib.buildDepsOnly commonArgs;
      in
      {
        # guardrails.mkDevShell brings the governance toolbelt (prek + gates + gitleaks +
        # cargo-deny/-mutants/-bloat + sccache) and auto-installs the prek commit/push hooks.
        # We add the project toolchain via `extra` and project env via `env`.
        devShells.default = guardrails.lib.${system}.mkDevShell {
          inherit pkgs;
          name = "tessera-dev";
          extra = [ rustToolchain ] ++ (with pkgs; [
            # Rust workflow on top of the pinned toolchain
            cargo-nextest

            # Python: interpreter + uv. Packages live in uv.lock — the heavy bench stack
            # (pcodec / vortex-data / zarr / duckdb / lance) isn't in nixpkgs, so uv owns it,
            # not Nix. Nix just guarantees the same python + uv everywhere.
            python312
            uv

            # Project tooling
            just
            gh
            jq
            ripgrep

            # Native build deps the storage/ingest crates link once implemented
            # (object_store→openssl, hdf5-sys→hdf5+libclang, zarrs/codec FFI). Present now so a
            # subagent implementing P3/P5 (see tessera/docs/ROADMAP.md) doesn't hit a wall.
            pkg-config
            openssl
            cmake
            clang
            zstd
            lz4
            hdf5
          ]);
          env = {
            # bindgen (hdf5-sys / dicom-rs) needs libclang
            LIBCLANG_PATH = "${pkgs.libclang.lib}/lib";
            HDF5_DIR = "${pkgs.hdf5}";
            # uv uses the Nix python and never downloads its own → reproducible
            UV_PYTHON = "${pkgs.python312}/bin/python3.12";
            UV_PYTHON_DOWNLOADS = "never";
          };
          hook = ''
            echo "[tessera] rust $(rustc --version 2>/dev/null | cut -d' ' -f2) · python ${pkgs.python312.version} · uv $(uv --version 2>/dev/null | cut -d' ' -f2)"
            echo "[tessera] cargo workspace lives in ./tessera  (cd tessera && cargo test)"
          '';
        };

        # ── CI = a shim over `nix flake check`. The logic lives HERE so the exact same command
        #    runs on a dev's machine and in CI — no "passes locally / fails in CI" drift. ──
        checks = {
          # Hermetic Rust gates over the tessera workspace.
          workspace-clippy = craneLib.cargoClippy (commonArgs // {
            inherit cargoArtifacts;
            cargoClippyExtraArgs = "--all-targets --all-features -- -D warnings";
          });
          workspace-test = craneLib.cargoNextest (commonArgs // {
            inherit cargoArtifacts;
            partitions = 1;
            partitionType = "count";
          });
          workspace-fmt = craneLib.cargoFmt { inherit src; };

          # guardrails agent-drift gates over the Rust source (code gates) + repo-wide structural
          # gates. cargo-deny stays a pre-commit gate (needs network for the advisory DB).
          guardrails-gates = pkgs.runCommand "guardrails-gates"
            { nativeBuildInputs = [ guardrails.lib.${system}.gates ]; } ''
            cd ${./.}
            guardrails-no-fake-impl tessera/crates \
              && guardrails-no-debug-leftovers tessera/crates \
              && guardrails-no-commented-code tessera/crates \
              && guardrails-no-conflict-markers . \
              && guardrails-derived-docs . \
              && touch $out
          '';

          # The dev shell itself must build (toolbelt + toolchain resolve).
          dev-shell = self.devShells.${system}.default;
        };

        formatter = pkgs.nixpkgs-fmt;
      });
}
