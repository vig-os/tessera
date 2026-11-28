# #905: what one small durable commit costs through the write path tessera has today
# (`table_block` + `WriteSession::append_block`), at 1, 100 and 10,000 rows per commit, on the SSD
# scratch and on tmpfs (tmpfs = CPU cost only: a sync there is free). Beside it a raw row log as a
# lower bound. Each arm is judged against the budget for small commits (`budget` in commit_cost.rs,
# proposed as the acceptance line of #905); the verdicts are the last records of the report and
# `verdict.jsonl`, which is rewritten after every stage.
# Builds `commit_cost.rs` as a tessera-io example at one tessera revision, after the pattern of
# bench/ksb/635-vortex-ab. To measure another revision, change the `head` input (and `vortex-head`
# with `vortexRev` if its fork pin moved) and run `nix flake update head`.
#
#   ksb run --name tessera-905-commit-N --cpu 4 --exclusive-cpus --mem 8Gi --time 240 "path:$PWD#default"
#   ksb out tessera-905-commit-N --path commit-cost.jsonl --dest ./commit-cost.jsonl
#   ksb out tessera-905-commit-N --path verdict.jsonl --dest ./verdict.jsonl
#
# Runs reported on #905: tessera-905-commit-2 (2026-11-18, 20,000-commit growth run, no verdict),
# tessera-905-commit-3 (2026-11-19, hit a 120-minute deadline in the tmpfs growth run) and
# tessera-905-commit-5 (2026-11-19).
{
  description = "ksb job: tessera commit cost for small keyed commits (#905)";

  inputs = {
    # The root flake's pins (flake.lock at the head revision).
    nixpkgs.url = "github:NixOS/nixpkgs/567a49d1913ce81ac6e9582e3553dd90a955875f";
    rust-overlay = {
      url = "github:oxalica/rust-overlay/77a8263847fb02dc49dbe377278ef6b952f1c6bb";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    crane.url = "github:ipetkov/crane/469fd08d0bcf6926321fa973c6777fbc87785dd7";
    head = {
      url = "github:vig-os/tessera/81914f64ea5fefcf3b066690ccbb19535adaacf1";
      flake = false;
    };
    # The vortex fork revision the head pins. ksb evaluates offline without `git`, so cargo's git
    # source can't be fetched there; the job's source copy points the [patch] at this input by path.
    vortex-head = {
      url = "github:gerchowl/vortex/5c55cf7fa35bac652849cf1a0e76a1a97035f715";
      flake = false;
    };
  };

  outputs =
    { nixpkgs, rust-overlay, crane, head, vortex-head, ... }:
    let
      pkgs = import nixpkgs {
        system = "x86_64-linux";
        overlays = [ (import rust-overlay) ];
      };
      vortexRev = "5c55cf7fa35bac652849cf1a0e76a1a97035f715";
      gitSource = "git+https://github.com/gerchowl/vortex?rev=${vortexRev}#${vortexRev}";
      lock = builtins.replaceStrings [ "source = \"${gitSource}\"\n" ] [ "" ] (
        builtins.readFile "${head}/tessera/Cargo.lock"
      );
      toolchain = pkgs.rust-bin.fromRustupToolchainFile "${head}/tessera/rust-toolchain.toml";
      craneLib = (crane.mkLib pkgs).overrideToolchain toolchain;
      src = pkgs.runCommand "tessera-905-src" { } ''
        cp -r ${head}/tessera $out && chmod -R u+w $out
        cp ${./commit_cost.rs} $out/crates/tessera-io/examples/commit_cost_905.rs
        # [patch] git -> path: each crate's directory in the fork checkout, by its manifest's package name.
        git_dep='{ git = "https://github.com/gerchowl/vortex", rev = "${vortexRev}" }'
        grep -q "$git_dep" $out/Cargo.toml
        for crate in $(grep -F "$git_dep" $out/Cargo.toml | cut -d' ' -f1); do
          manifest=$(grep -rlx --include=Cargo.toml "name = \"$crate\"" ${vortex-head} | head -1)
          test -n "$manifest"
          sed -i "s|^$crate = $git_dep|$crate = { path = \"$(dirname "$manifest")\" }|" $out/Cargo.toml
        done
        ! grep -qF "$git_dep" $out/Cargo.toml
        cp ${pkgs.writeText "Cargo.lock" lock} $out/Cargo.lock
      '';
      probe = craneLib.mkCargoDerivation {
        pname = "tessera-905-commit-cost";
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
          cargo build --release --locked -p tessera-io --example commit_cost_905
        '';
        installPhaseCommand = ''
          install -Dm755 target/release/examples/commit_cost_905 $out/bin/commit_cost
        '';
        doCheck = false;
      };
      job = pkgs.writeShellApplication {
        name = "tessera-905-commit-cost";
        runtimeInputs = [ pkgs.coreutils pkgs.strace ];
        excludeShellChecks = [ "SC2129" ];
        text = ''
          out="''${KSB_JOB_OUT:-$PWD/out}"
          mkdir -p "$out"
          res="$out/commit-cost.jsonl"
          : > "$res"
          bin=${probe}/bin/commit_cost
          echo "# #905 commit cost, tessera ${head.rev}, nproc=$(nproc), $(date -u +%FT%TZ)" | tee "$out/run.txt"

          # The verdicts over the report so far. Rewritten after every stage, so that a job that runs
          # into its deadline still leaves the verdicts of the stages it finished.
          judge() {
            "$bin" verdict "$res" > "$out/verdict.jsonl"
          }

          run_medium() {
            medium="$1"
            parent="$2"
            if [ ! -d "$parent" ] || [ ! -w "$parent" ]; then
              echo "skip $medium: $parent is not writable" | tee -a "$out/run.txt"
              return 0
            fi
            dir=$(mktemp -d "$parent/tessera-905.XXXXXX")
            echo "medium $medium at $dir ($(stat -f -c %T "$dir"))" | tee -a "$out/run.txt"
            "$bin" sync-cost "$dir" "$medium" >> "$res"
            for trial in 1 2 3; do
              echo "trial $trial on $medium, load $(cut -d' ' -f1-3 /proc/loadavg)" | tee -a "$out/run.txt"
              "$bin" tessera "$dir" 1 2000 "$medium" seal >> "$res"
              "$bin" rowlog "$dir" 1 2000 "$medium" >> "$res"
              "$bin" tessera "$dir" 100 2000 "$medium" seal >> "$res"
              "$bin" rowlog "$dir" 100 2000 "$medium" >> "$res"
              "$bin" tessera "$dir" 10000 200 "$medium" seal >> "$res"
              "$bin" rowlog "$dir" 10000 200 "$medium" >> "$res"
            done
            judge
            # Growth with the number of commits already in the session: the budget's flatness line.
            # The cheap arm first; the existing path took 88 minutes here in run tessera-905-commit-3.
            echo "growth on $medium, load $(cut -d' ' -f1-3 /proc/loadavg)" | tee -a "$out/run.txt"
            "$bin" rowlog "$dir" 1 100000 "$medium" >> "$res"
            "$bin" tessera "$dir" 1 100000 "$medium" >> "$res"
            judge
            # A writer killed with SIGKILL: every acknowledged commit must be recovered.
            "$bin" crash "$dir" 1 500 "$medium" >> "$res"
            "$bin" crash "$dir" 100 200 "$medium" >> "$res"
            # Sync calls per commit (200 commits each; not timed).
            strace -f -c -e trace=fsync,fdatasync -o "$out/strace-tessera-$medium.txt" \
              "$bin" tessera "$dir" 1 200 "$medium" > /dev/null \
              || echo "strace failed for tessera on $medium" | tee -a "$out/run.txt"
            strace -f -c -e trace=fsync,fdatasync -o "$out/strace-rowlog-$medium.txt" \
              "$bin" rowlog "$dir" 1 200 "$medium" > /dev/null \
              || echo "strace failed for rowlog on $medium" | tee -a "$out/run.txt"
            rm -rf "$dir"
          }

          run_medium ssd /data/ssd-outputs
          run_medium tmpfs /dev/shm
          judge
          cat "$out/verdict.jsonl" >> "$res"
          echo "done $(date -u +%FT%TZ)" | tee -a "$out/run.txt"
          cat "$res"
        '';
      };
    in
    {
      packages.x86_64-linux = { inherit probe; default = job; };
      apps.x86_64-linux.default = {
        type = "app";
        program = "${job}/bin/tessera-905-commit-cost";
      };
    };
}
