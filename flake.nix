{
  description = "fd5 / Tessera — FAIR data-product format · Rust + Python dev environment, governed by guardrails";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";

    # Pinned Rust toolchain (reads tessera/rust-toolchain.toml).
    rust-overlay.url = "github:oxalica/rust-overlay";
    rust-overlay.inputs.nixpkgs.follows = "nixpkgs";

    # Shared code-quality / agent-drift governance: prek (Rust pre-commit), the gate
    # toolbelt, and sccache. The devShell is built through its mkDevShell.
    guardrails.url = "github:gerchowl/guardrails";
    guardrails.inputs.nixpkgs.follows = "nixpkgs";
    guardrails.inputs.flake-utils.follows = "flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay, guardrails }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) ];
        };
        # One source of truth for the compiler + components — see tessera/rust-toolchain.toml.
        rustToolchain = pkgs.rust-bin.fromRustupToolchainFile ./tessera/rust-toolchain.toml;
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

        # CI = a shim over `nix flake check`: the dev shell must build. (Existing project
        # workflows in .github/workflows/ remain authoritative for language-specific CI.)
        checks.default = self.devShells.${system}.default;

        formatter = pkgs.nixpkgs-fmt;
      });
}
