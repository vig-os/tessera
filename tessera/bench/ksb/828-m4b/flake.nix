# #828 round 2: A3 re-measure (tessera-table-v1 vs -v2) and the M4b sweep, in a reserved ksb slice.
#
# Builds tessera-io's `table_profile` example twice in crane derivations (never in the pod): A = dev at the
# last v1 commit, B = the #828 head. Both take the SAME harness source (`./table_profile.rs`, a copy of
# tessera/crates/tessera-io/examples/table_profile.rs made by `submit.sh`). B's job-only source copy gets the
# M4b seam: the Pco level, Pco values per page and the numeric chunk framing read `T828_PCO_LEVEL`,
# `T828_PAGE`, `T828_CHUNK_ROWS` (each defaulting to the committed profile constant, so with none set B is
# exactly the #828 profile). The seam is never committed to product code.
#
# The de-identified #822 fixture (`tessera-vendor-2026-11`, owner-approved private data; not a ksb dataset) is
# staged by submit.sh into the job's own results volume ($KSB_JOB_OUT/fixture) once the job is submitted; the job
# verifies, decompresses it to local scratch and deletes the staged copy. Shipping it inside the nix snapshot timed
# out in preparation (`nix copy` > 1800 s, job tessera-828-m4b-probe-1). It is never committed.
#
#   ./submit.sh <job-name>
{
  description = "ksb job: tessera #828 A3 re-measure + M4b sweep";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/567a49d1913ce81ac6e9582e3553dd90a955875f";
    rust-overlay = {
      url = "github:oxalica/rust-overlay/77a8263847fb02dc49dbe377278ef6b952f1c6bb";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    crane.url = "github:ipetkov/crane/469fd08d0bcf6926321fa973c6777fbc87785dd7";
    # A: dev at the last tessera-table-v1 commit. B: the #828 head (tessera-table-v2).
    base = {
      url = "github:vig-os/tessera/f390528eb2bfa3fc4ea474d9129c242b906e8e0a";
      flake = false;
    };
    head = {
      url = "github:vig-os/tessera/d0e8749814e2ba0022f2c4d5650b31a3dae60fde";
      flake = false;
    };
    # The vortex fork both sides pin; ksb evaluates offline, so the [patch] is pointed at this by path.
    vortex = {
      url = "github:gerchowl/vortex/5c55cf7fa35bac652849cf1a0e76a1a97035f715";
      flake = false;
    };
  };

  outputs =
    { nixpkgs, rust-overlay, crane, base, head, vortex, ... }:
    let
      pkgs = import nixpkgs {
        system = "x86_64-linux";
        overlays = [ (import rust-overlay) ];
      };
      vortexRev = "5c55cf7fa35bac652849cf1a0e76a1a97035f715";
      build =
        label: rev: seam:
        let
          gitSource = "git+https://github.com/gerchowl/vortex?rev=${vortexRev}#${vortexRev}";
          lock = builtins.replaceStrings [ "source = \"${gitSource}\"\n" ] [ "" ] (
            builtins.readFile "${rev}/tessera/Cargo.lock"
          );
          toolchain = pkgs.rust-bin.fromRustupToolchainFile "${rev}/tessera/rust-toolchain.toml";
          craneLib = (crane.mkLib pkgs).overrideToolchain toolchain;
          src = pkgs.runCommand "tessera-828-${label}-src" { nativeBuildInputs = [ pkgs.python3 ]; } ''
            cp -r ${rev}/tessera $out && chmod -R u+w $out
            cp ${./table_profile.rs} $out/crates/tessera-io/examples/table_profile.rs
            ${pkgs.lib.optionalString seam "python3 ${./m4b_seam.py} $out/crates/tessera-io/src/table.rs"}
            git_dep='{ git = "https://github.com/gerchowl/vortex", rev = "${vortexRev}" }'
            grep -q "$git_dep" $out/Cargo.toml
            for crate in $(grep -F "$git_dep" $out/Cargo.toml | cut -d' ' -f1); do
              manifest=$(grep -rlx --include=Cargo.toml "name = \"$crate\"" ${vortex} | head -1)
              test -n "$manifest"
              sed -i "s|^$crate = $git_dep|$crate = { path = \"$(dirname "$manifest")\" }|" $out/Cargo.toml
            done
            ! grep -qF "$git_dep" $out/Cargo.toml
            cp ${pkgs.writeText "Cargo.lock" lock} $out/Cargo.lock
          '';
        in
        craneLib.mkCargoDerivation {
          pname = "tessera-828-${label}";
          version = "0.0.0";
          inherit src;
          cargoArtifacts = null;
          cargoVendorDir = craneLib.vendorCargoDeps { cargoLockContents = lock; };
          nativeBuildInputs = with pkgs; [ clang pkg-config cmake ];
          buildInputs = with pkgs; [ stdenv.cc.cc.lib hdf5 ];
          LIBCLANG_PATH = "${pkgs.libclang.lib}/lib";
          buildPhaseCargoCommand = ''
            cargo build --release --locked -p tessera-io --example table_profile
          '';
          installPhaseCommand = ''
            install -Dm755 target/release/examples/table_profile $out/bin/table_profile
          '';
          doCheck = false;
        };
      v1 = build "v1" base false;
      v2 = build "v2" head true;
      job = pkgs.writeShellApplication {
        name = "tessera-828-m4b";
        runtimeInputs = with pkgs; [
          coreutils gnugrep gawk util-linux zstd python3 procps linuxPackages.perf
        ];
        text = ''
          export V1=${v1}/bin/table_profile V2=${v2}/bin/table_profile
          exec python3 ${./run.py} "$@"
        '';
      };
    in
    {
      packages.x86_64-linux = { inherit v1 v2; default = job; };
      apps.x86_64-linux.default = {
        type = "app";
        program = "${job}/bin/tessera-828-m4b";
      };
    };
}
