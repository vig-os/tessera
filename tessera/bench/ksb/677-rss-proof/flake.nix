# ADR-0064 §8.1's RSS proof (#715), as amended by #760: the consumer reproducer on push_table, swept and evaluated
# by run.py in two jobs, each exiting 1 on any failed criterion. Results: ADR-0026 and FEATURE-MATRIX. tessera has
# no ksb project with a scratch prefix, so this borrows dogplet-analysis's, as the #677 jobs (t677b-*) did.
#
# Criteria 1-5, staging on disk-backed --scratch (the guard fails the job if the session dir is tmpfs):
#
#   P=/mnt/HDD/larsgerchow/dogplet_analysis_scratch
#   ksb run --project dogplet-analysis --name tessera-715-r3-sweep-N --cpu 4 --exclusive-cpus --mem 4Gi --time 360 \
#     --scratch $P/s715-session --scratch $P/s715-out "path:$PWD#default" -- \
#     --session-root $P/s715-session --output-root $P/s715-out
#
# Criterion 6, session and output on /dev/shm (A.3: the converse guard fails the job unless the session dir IS
# tmpfs; tmpfs pages count toward --mem, sized by A.3's formula):
#
#   ksb run --name tessera-715-r3-par-N --cpu 4 --exclusive-cpus --mem 6Gi --time 30 "path:$PWD#parallel" -- \
#     --session-root /dev/shm/s715-session --output-root /dev/shm/s715-out
#
# Fail-before: copy this dir, `nix flake lock --override-input tessera github:vig-os/tessera/<pre-fix rev>`, and
# pass `--workers 1` (the pre-#759 writer, a841745, fails criterion 2 at W = 1).
{
  description = "ksb job: ADR-0064 §8.1 RSS proof of push_table with the consumer reproducer (#715)";

  inputs = {
    # The consumer's toolchain: the nixpkgs its devkit pins (MorePET/visionx-rs flake.lock), rustc 1.95.
    nixpkgs.url = "github:NixOS/nixpkgs/cf5e76507c6e23b59f7e0ffcc7baa2a39ddd8442";
    crane.url = "github:ipetkov/crane/469fd08d0bcf6926321fa973c6777fbc87785dd7"; # the root flake's pin
    # The tessera under test: this PR changes no tessera crate, so its head's crates are its base's (dev with the
    # #759 fix, PR #765).
    tessera = {
      url = "github:vig-os/tessera/26159de0d618b57e2fa7aa9e0b24c4a4e74be3fa";
      flake = false;
    };
    # MorePET/visionx-rs PR #41 (chore/39-tessera-677-repro). The repo is private: the submitter's own
    # git+ssh fetch puts it in the store and ksb ships the store path, so no credential enters the job.
    visionx = {
      url = "git+ssh://git@github.com/MorePET/visionx-rs?rev=c4aff942e4cfc7876b966e9d8c01c12ad6e7740d";
      flake = false;
    };
  };

  outputs =
    { nixpkgs, crane, tessera, visionx, ... }:
    let
      pkgs = import nixpkgs { system = "x86_64-linux"; };
      version = (builtins.fromTOML (builtins.readFile "${tessera}/tessera/Cargo.toml")).workspace.package.version;
      # The consumer's own build (docs/implementation/39-memory.md): its Cargo.lock with tessera-core and
      # tessera-io moved from their git pin to the path patch below, the only change `cargo update -p
      # tessera-core -p tessera-io` makes (every other dependency of the tree under test is locked already).
      # So vortex is 0.75.0 from crates.io: tessera's fork pin is a workspace [patch] no dependent inherits.
      pinned = builtins.readFile "${visionx}/Cargo.lock";
      cargoLock = builtins.replaceStrings [
        ''
          version = "0.1.0-alpha.3"
          source = "git+https://github.com/vig-os/tessera?branch=dev#9424f90c88cee918f2c68784b2bb8d98291a4c53"
        ''
      ] [ "version = \"${version}\"\n" ] pinned;
      lockFile =
        assert pkgs.lib.assertMsg (cargoLock != pinned) "visionx Cargo.lock no longer pins tessera at 9424f90";
        pkgs.writeText "Cargo.lock" cargoLock;
      # stream_memory.patch switches the `pairs` job to push_table (exact nullable schema), lets --rows 0
      # through, adds --session-dir and a 10 ms CSV sampler (RssAnon, the job cgroup's memory.current and
      # memory.stat anon/file/shmem, rows generated), and sets `partition: None` in visionx-tsra's TableSpec
      # literal, which ADR-0064 PR A's new field breaks.
      src = pkgs.runCommand "visionx-stream-memory-src" { } ''
        cp -r ${visionx} $out && chmod -R u+w $out && cd $out
        patch -p1 < ${./stream_memory.patch}
        cp ${lockFile} Cargo.lock
        mkdir .cargo && cat > .cargo/config.toml <<EOF
        [patch."https://github.com/vig-os/tessera"]
        tessera-core.path = "${tessera}/tessera/crates/tessera-core"
        tessera-io.path = "${tessera}/tessera/crates/tessera-io"
        EOF
      '';
      craneLib = crane.mkLib pkgs;
      stream-memory = craneLib.mkCargoDerivation {
        pname = "visionx-stream-memory";
        version = "0.0.0";
        inherit src;
        cargoArtifacts = null;
        cargoVendorDir = craneLib.vendorCargoDeps { cargoLockContents = cargoLock; };
        nativeBuildInputs = [ pkgs.pkg-config pkgs.rustPlatform.bindgenHook ];
        buildPhaseCargoCommand = "cargo build --release --locked -p visionx-tsra --example stream_memory";
        installPhaseCommand = "install -Dm755 target/release/examples/stream_memory $out/bin/stream_memory";
        doCheck = false;
      };
      job = pkgs.writeShellApplication {
        name = "rss-proof";
        runtimeInputs = [ pkgs.python3 pkgs.coreutils ];
        text = ''exec python3 ${./run.py} "$@"'';
      };
      # One app per job: run.py's `job` (criteria 1-5) and `parallel` (criterion 6) subcommands.
      app = cmd: {
        type = "app";
        program = "${pkgs.writeShellScript "rss-proof-${cmd}" ''
          exec ${job}/bin/rss-proof ${cmd} --binary ${stream-memory}/bin/stream_memory "$@"
        ''}";
      };
    in
    {
      packages.x86_64-linux = { inherit stream-memory; default = job; };
      apps.x86_64-linux = { default = app "job"; parallel = app "parallel"; };
    };
}
