# #635: vortex 0.75 (fork) vs 0.93.0 (rebased fork), same machine, same job. Builds tessera-io's criterion benches
# (`codec`, `throughput`) and `size_probe.rs` at two tessera revisions and runs them alternately (A B A B), so a
# shared-host slowdown hits both sides. Sizes are machine-independent; timings are context only (shared HDD and
# host, exo-fleet#980).
#
#   ksb run --name tessera-635-ab-N --cpu 4 --exclusive-cpus --mem 8Gi --time 120 "path:$PWD#default"
#   ksb out tessera-635-ab-N --path report.txt --dest ./report.txt
#
# Run tessera-635-ab-2 (2026-11-09) built head at d33ee99, which differs from the pinned head only in tests, docs and
# CHANGELOG.md; its numbers are in PR #788.
{
  description = "ksb job: tessera vortex 0.75 vs 0.93 table bench + size A/B (#635)";

  inputs = {
    # The root flake's pins (flake.lock at the head revision).
    nixpkgs.url = "github:NixOS/nixpkgs/567a49d1913ce81ac6e9582e3553dd90a955875f";
    rust-overlay = {
      url = "github:oxalica/rust-overlay/77a8263847fb02dc49dbe377278ef6b952f1c6bb";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    crane.url = "github:ipetkov/crane/469fd08d0bcf6926321fa973c6777fbc87785dd7";
    # A = dev with the vortex 0.75 fork pin; B = this PR's bump to the 0.93.0 fork pin.
    base = {
      url = "github:vig-os/tessera/6a3078079f8a49476bf101ea05464bebe24beb64";
      flake = false;
    };
    head = {
      url = "github:vig-os/tessera/6bc2ff7bab9f4d95d26be3d9e2e08880240af1eb";
      flake = false;
    };
    # The vortex fork revision each side pins. ksb evaluates offline without `git`, so cargo's git source can't be
    # fetched there; these inputs are locked on the submit side and the job's source copy points the [patch] at
    # them by path. Same code, a different way for cargo to find it.
    vortex-base = {
      url = "github:gerchowl/vortex/608d8a1162cf40dbea70f8dd72781ec243754282";
      flake = false;
    };
    vortex-head = {
      url = "github:gerchowl/vortex/5c55cf7fa35bac652849cf1a0e76a1a97035f715";
      flake = false;
    };
  };

  outputs =
    { nixpkgs, rust-overlay, crane, base, head, vortex-base, vortex-head, ... }:
    let
      pkgs = import nixpkgs {
        system = "x86_64-linux";
        overlays = [ (import rust-overlay) ];
      };
      build =
        label: rev: vortex: vortexRev:
        let
          gitSource = "git+https://github.com/gerchowl/vortex?rev=${vortexRev}#${vortexRev}";
          lock = builtins.replaceStrings [ "source = \"${gitSource}\"\n" ] [ "" ] (
            builtins.readFile "${rev}/tessera/Cargo.lock"
          );
          toolchain = pkgs.rust-bin.fromRustupToolchainFile "${rev}/tessera/rust-toolchain.toml";
          craneLib = (crane.mkLib pkgs).overrideToolchain toolchain;
          src = pkgs.runCommand "tessera-${label}-src" { } ''
            cp -r ${rev}/tessera $out && chmod -R u+w $out
            cp ${./size_probe.rs} $out/crates/tessera-io/examples/size_probe_635.rs
            # [patch] git -> path: each crate's directory in the fork checkout, by its manifest's package name.
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
          pname = "tessera-635-${label}";
          version = "0.0.0";
          inherit src;
          cargoArtifacts = null;
          cargoVendorDir = craneLib.vendorCargoDeps {
            cargoLockContents = lock;
          };
          nativeBuildInputs = with pkgs; [ clang pkg-config cmake ];
          buildInputs = with pkgs; [ stdenv.cc.cc.lib hdf5 ];
          LIBCLANG_PATH = "${pkgs.libclang.lib}/lib";
          buildPhaseCargoCommand = ''
            cargo build --release --locked -p tessera-io --example size_probe_635
            cargo bench --locked -p tessera-io --no-run --message-format json > bench-build.json
          '';
          installPhaseCommand = ''
            install -Dm755 target/release/examples/size_probe_635 $out/bin/size_probe
            for b in codec throughput; do
              exe=$(grep '"executable"' bench-build.json | grep -o "\"executable\":\"[^\"]*/$b-[^\"]*\"" | cut -d'"' -f4 | head -1)
              install -Dm755 "$exe" "$out/bin/bench-$b"
            done
          '';
          doCheck = false;
        };
      a = build "base-vortex075" base vortex-base "608d8a1162cf40dbea70f8dd72781ec243754282";
      b = build "head-vortex093" head vortex-head "5c55cf7fa35bac652849cf1a0e76a1a97035f715";
      job = pkgs.writeShellApplication {
        name = "tessera-635-ab";
        runtimeInputs = [ pkgs.coreutils pkgs.gnugrep ];
        text = ''
          out="''${KSB_JOB_OUT:-$PWD/out}"
          mkdir -p "$out"
          report="$out/report.txt"
          {
            echo "# #635 A/B, nproc=$(nproc), $(date -u +%FT%TZ)"
            echo "## sizes A = vortex 0.75 fork (base ${base.rev})"
            ${a}/bin/size_probe
            echo "## sizes B = vortex 0.93 fork (head ${head.rev})"
            ${b}/bin/size_probe
          } > "$report"
          for round in 1 2; do
            for side in A B; do
              if [ "$side" = A ]; then bin=${a}/bin; else bin=${b}/bin; fi
              dir="$out/criterion-$side-$round"
              mkdir -p "$dir"
              for bench in codec throughput; do
                echo "## round $round side $side bench $bench" >> "$report"
                (cd "$dir" && "$bin/bench-$bench" --bench --noplot 2>&1 | grep -E '^[a-z_0-9]+/|time:|thrpt:' ) >> "$report"
              done
            done
          done
          cat "$report"
        '';
      };
    in
    {
      packages.x86_64-linux = { inherit a b; default = job; };
      apps.x86_64-linux.default = {
        type = "app";
        program = "${job}/bin/tessera-635-ab";
      };
    };
}
