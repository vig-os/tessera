# #793 reproducer: tessera-io's two "hanging" tests (`stream::tests::content_hash_is_identical_across_workers_and_windows`,
# `session_leaf_memory`) under ksb-pod-like constraints, A/B'd in one job (run.sh):
#   A  tmpfs, 1 process                                   the baseline
#   B1 tmpfs, 32 processes on `taskset -c 0-1`            CPU-starved (hypothesis 2: a window/commit-order deadlock)
#   B2 B1 + `ulimit -u 256`                               thread spawns fail with EAGAIN (hypothesis 1)
#   C  disk-backed --scratch, 1 process                   real fsync latency
#   D  C + 2 buffered `dd` writers on the same disk       a CI pod's /nix volume under concurrent build writes
#   E  disk-backed --scratch, 32 processes                nextest's process fan-out
# with strace's fsync count per test and the stuck thread's kernel wait channel. tessera has no ksb project with a
# scratch prefix, so this borrows dogplet-analysis's, as the #677/#715 jobs did:
#
#   P=/mnt/HDD/larsgerchow/dogplet_analysis_scratch
#   ksb run --project dogplet-analysis --scratch $P/s793 --cpu 4 --mem 8Gi --time 150 \
#     --name tessera-793-rN "path:$PWD#default" -- --scratch $P/s793
#
# To run another tessera, `nix flake lock --override-input tessera github:vig-os/tessera/<rev>` in a copy of this dir.
#
# The second app, `#footprint`, measures what the fix (TMPDIR on /dev/shm, `withRamTmpdir` in the repo's flake.nix)
# has to hold: it rebuilds every protected check with TMPDIR on the build's private /dev/shm and footprint.py sampling
# the peak bytes in use there (statvfs, ~1 ms), at CI's 4-thread test budget. Results: <check> peak lines in
# footprints.txt, and a per-second series per check.
#
#   ksb run --cpu 2 --mem 4Gi --time 60 --name tessera-793-footprint "path:$PWD#footprint"
{
  description = "ksb job: #793 tessera-io test stall reproducer";

  inputs = {
    # The tessera under test (its nixpkgs, rust-overlay and crane locks are reused below).
    tessera.url = "github:vig-os/tessera/6a3078079f8a49476bf101ea05464bebe24beb64";
  };

  outputs = { tessera, ... }:
    let
      system = "x86_64-linux";
      pkgs = import tessera.inputs.nixpkgs {
        inherit system;
        overlays = [ (import tessera.inputs.rust-overlay) ];
      };
      rustToolchain = pkgs.rust-bin.fromRustupToolchainFile "${tessera}/tessera/rust-toolchain.toml";
      craneLib = (tessera.inputs.crane.mkLib pkgs).overrideToolchain rustToolchain;
      src = "${tessera}/tessera";
      # The vortex fork pin as a fixed-output fetchgit: the compute VM evaluates the job and can't
      # builtins.fetchGit, but a FOD fetches at build time, where it has egress.
      vendor = craneLib.vendorCargoDeps {
        inherit src;
        outputHashes."git+https://github.com/gerchowl/vortex?rev=608d8a1162cf40dbea70f8dd72781ec243754282#608d8a1162cf40dbea70f8dd72781ec243754282" =
          "sha256-9SESpWfGs7YqIqi7opdo7LZfa36xo9nGFfF1Dqu0/SM=";
      };
      # The release-profile test binaries of tessera-io, as `workspace-test` (crane's nextest) builds them.
      tests = craneLib.mkCargoDerivation {
        pname = "tessera-io-tests";
        version = "0.0.0";
        inherit src;
        cargoArtifacts = null;
        cargoVendorDir = vendor;
        nativeBuildInputs = with pkgs; [ clang pkg-config cmake jq ];
        buildInputs = with pkgs; [ stdenv.cc.cc.lib ];
        LIBCLANG_PATH = "${pkgs.libclang.lib}/lib";
        buildPhaseCargoCommand =
          "cargo test --release --locked -p tessera-io --no-run --message-format=json-render-diagnostics > msgs.json";
        installPhaseCommand = ''
          mkdir -p $out/bin
          jq -r 'select(.reason == "compiler-artifact" and .executable != null and .profile.test) | .executable' msgs.json |
            while read -r f; do install -Dm755 "$f" "$out/bin/$(basename "$f")"; done
        '';
        doCheck = false;
      };
      job = pkgs.writeShellApplication {
        name = "repro-793";
        runtimeInputs = with pkgs; [ coreutils util-linux procps strace gdb python3 bc findutils gnugrep gawk ];
        text = ''TESTS=${tests}/bin PROBE=${./fsync_probe.py} exec bash ${./run.sh} "$@"'';
      };
      # ── The tmp footprint of every check that gets a RAM-backed TMPDIR (#793 r2) ──
      # tessera's own checks, unchanged except that (a) TMPDIR moves to the build's private /dev/shm
      # and footprint.py records the peak bytes in use there, (b) the test budget is CI's
      # (`--cores 4`), (c) the vortex git dep comes from the FOD `vendor` above (the compute VM can't
      # builtins.fetchGit) and (d) the 6 GB target archive isn't installed. `--keep-going` style: a
      # failing check still records its footprint (`|| true` would hide it, so the app reports it).
      deps = tessera.legacyPackages.${system}.cargoArtifacts.overrideAttrs { cargoVendorDir = vendor; };
      measure = hook: name: (tessera.checks.${system}.${name}).overrideAttrs (o: {
        pname = "${o.pname or "check"}-footprint";
        cargoVendorDir = vendor;
        cargoArtifacts = deps;
        doInstallCargoArtifacts = false;
        ${hook} = ''
          export TMPDIR=/dev/shm/tmp; mkdir -p "$TMPDIR"
          echo "footprint: TMPDIR on $(stat -f -c %T "$TMPDIR"), $(df -h --output=size /dev/shm | tail -1) (${name})"
          ${pkgs.python3}/bin/python3 -I ${./footprint.py} /dev/shm "$NIX_BUILD_TOP/peak" &
          fp_pid=$!
        '' + (o.${hook} or "") + ''
          export NEXTEST_TEST_THREADS=4 RUST_TEST_THREADS=4 RAYON_NUM_THREADS=4
        '';
        postInstall = (o.postInstall or "") + ''
          kill $fp_pid; mkdir -p $out; cp "$NIX_BUILD_TOP"/peak* $out/
        '';
      });
      preCheckChecks = [ "workspace-test" "workspace-doctest" "sql-tests" "s3-range-read" ];
      preBuildChecks = [ "seal-profile-determinism" "ingest-gate-a" "ingest-gate-a-sql"
        "ingest-gate-a-workspace" "ingest-gate-a-reduced" ];
      footprints = pkgs.linkFarm "tessera-793-footprints"
        (map (n: { name = n; path = measure "preCheck" n; }) preCheckChecks
          ++ map (n: { name = n; path = measure "preBuild" n; }) preBuildChecks);
      footprintJob = pkgs.writeShellApplication {
        name = "footprint-793";
        runtimeInputs = with pkgs; [ coreutils gawk ];
        text = ''
          OUT=''${KSB_JOB_OUT:-/tmp/out}; mkdir -p "$OUT"
          for d in ${footprints}/*; do
            n=$(basename "$d"); cp "$d/peak.series" "$OUT/$n.series"
            awk -v n="$n" '{ printf "%-26s peak %8.1f MiB at %7.1f s\n", n, $1 / 1048576, $2 }' "$d/peak"
          done | tee "$OUT/footprints.txt"
        '';
      };
    in
    {
      packages.${system} = { inherit tests vendor footprints; default = job; footprint = footprintJob; };
      apps.${system} = {
        default = { type = "app"; program = "${job}/bin/repro-793"; };
        footprint = { type = "app"; program = "${footprintJob}/bin/footprint-793"; };
      };
    };
}
