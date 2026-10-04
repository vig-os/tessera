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

    # vigOS devkit toolchain, pinned to the release this repo adopts in
    # `.vig-os` (#364). Its overlay supplies `vig-utils`, the console scripts
    # the devkit-managed `ci.yml` calls: in `direnv` mode the commit-checks job
    # runs `uv run validate-commit-range` and resolves it off THIS dev-shell's
    # PATH, so without the input that job fails with "Failed to spawn". Keep the
    # pin in step with DEVKIT_VERSION.
    vigos.url = "github:vig-os/devkit/1.17.0";
    vigos.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay, crane, guardrails, vigos }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) vigos.overlays.default ];
        };
        # One source of truth for the compiler + components — see tessera/rust-toolchain.toml.
        rustToolchain = pkgs.rust-bin.fromRustupToolchainFile ./tessera/rust-toolchain.toml;

        # crane, pinned to our toolchain. The Rust workspace lives in ./tessera; deps are
        # vendored once (buildDepsOnly) so clippy/test/fmt run hermetically & offline in CI.
        craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;
        # Cargo sources + the conformance corpus (tests/conformance.rs reads corpus/corpus.json) +
        # docs/examples (tessera-ingest::spec embeds the example ingest TOML via include_str! and a
        # test validates it) + the CLI docs-as-tests (`tests/cmd/*.trycmd` walkthroughs + their `.in/`
        # fixtures) + Gate B's committed feature snapshots (`tests/feature-snapshots/*.txt`, ADR-0057
        # §5) — cleanCargoSource would otherwise strip these non-Rust files, so the trycmd
        # docs-as-tests would silently run ZERO cases in the hermetic gate.
        src = pkgs.lib.cleanSourceWith {
          src = ./tessera;
          filter = path: type:
            (craneLib.filterCargoSources path type)
            || (pkgs.lib.hasInfix "/corpus/" path)
            || (pkgs.lib.hasInfix "/docs/examples/" path)
            || (pkgs.lib.hasInfix "/docs/dictionaries/" path)
            || (pkgs.lib.hasInfix "/tests/cmd/" path)
            || (pkgs.lib.hasInfix "/tests/feature-snapshots/" path)
            # The GENERATED product-schema reference the book includes (#389). Its drift test compares
            # this committed copy against `SchemaRegistry::builtin()`, so the file has to reach the
            # sandbox — the `derived-docs` gate cannot do the job here because it runs without cargo.
            || (pkgs.lib.hasInfix "/tests/derived-docs/" path);
          name = "source";
        };
        # Every feature declared anywhere in the workspace **except `static-hdf5`** (ADR-0057 §4), in
        # cargo's `package/feature` form so one invocation from the virtual-manifest root covers all
        # members. This is what the PR clippy gate builds instead of `--all-features`.
        #   tessera-core   array-zarr, table-arrow   (= `full`)
        #   tessera-io     cloud
        #   tessera-cli    cloud (→ tessera-io/cloud), sql
        #   tessera-ingest arrow, parquet, csv       → in `default`, so already on; `static-hdf5`
        #                  static-hdf5                    → deliberately absent (ADR-0057 §4)
        #   tessera-py / tessera-wasm                → no features
        workspaceFeatures = builtins.concatStringsSep "," [
          "tessera-core/full"
          "tessera-io/cloud"
          "tessera-cli/cloud"
          "tessera-cli/sql"
        ];

        # One Gate A derivation per feature configuration (see `checks.ingest-gate-a*` for the why).
        # Regenerates the ingest conformance corpus under `features` and requires byte-equality with the
        # committed goldens.
        #
        # No `-p`: cargo only accepts the cross-package `--features pkg/feat` form from the workspace
        # root, and cross-package unification is the entire point — `tessera-cli/sql` has to be able to
        # reach the shared arrow tree for the H1 door to be under test at all. The example is unambiguous
        # (only tessera-ingest declares it), so no `-p` is needed.
        ingestGateA = tag: features:
          craneLib.mkCargoDerivation (commonArgs // {
            inherit cargoArtifacts;
            # The prebuilt dependency artifacts are needed, but must not be re-installed into `$out` —
            # this derivation's output is a pass/fail marker, not a build cache, and inheriting the
            # install hook without disabling it fails the check on
            # `doCompressAndInstallFullArchive: unbound variable` *after* the gate has passed.
            doInstallCargoArtifacts = false;
            pnameSuffix = "-ingest-gate-a-${tag}";
            buildPhaseCargoCommand = ''
              set -euo pipefail
              echo "[gate A/${tag}] cargo run --example gen_ingest_corpus ${features}" >&2
              cargo run -q --example gen_ingest_corpus ${features} > "$TMPDIR/corpus.json"
              if ! cmp -s "$TMPDIR/corpus.json" corpus/ingest-corpus.json; then
                echo "" >&2
                echo "Gate A (ADR-0057 §5): the ingest corpus under the '${tag}' feature" >&2
                echo "configuration does NOT match the committed corpus/ingest-corpus.json." >&2
                diff -u corpus/ingest-corpus.json "$TMPDIR/corpus.json" >&2 || true
                echo "" >&2
                echo "A content_hash that moved means a decoder extracted DIFFERENT VALUES — say which" >&2
                echo "ADR-0056 §5 H-rule changed behaviour and why the new values are correct." >&2
                echo "A manifest_hash that moved is routine ONLY alongside a moved ingest_decoder." >&2
                echo "Regenerate deliberately with:" >&2
                echo "    cargo run -p tessera-ingest --example gen_ingest_corpus > corpus/ingest-corpus.json" >&2
                exit 1
              fi
              echo "[gate A/${tag}] agrees with the committed corpus" >&2
            '';
          });

        commonArgs = {
          inherit src;
          strictDeps = true;
          pname = "tessera";
          version = "0.0.0";
          # The Vortex table backend pulls a transitive dep (`custom-labels`) whose build script
          # runs bindgen (needs libclang) and links libstdc++. The tessera-ingest GE-HDF5 reader
          # links libhdf5 (found via pkg-config — `hdf5-metno-sys` reads PKG_CONFIG_PATH when
          # HDF5_DIR is unset). Provide all to every crane derivation (deps/clippy/test).
          # `cmake` is kept for the `static-hdf5` feature (hdf5-metno-src builds libhdf5 from source via
          # CMake). No *PR* check enables it any more (ADR-0057 §4 — see `workspace-clippy` below); it is
          # exercised release-only, by the cargo-dist channel (`dist-workspace.toml`). The default
          # (pkg-config) builds don't invoke CMake, so keeping it here costs them nothing.
          nativeBuildInputs = with pkgs; [ clang pkg-config cmake ];
          buildInputs = with pkgs; [ stdenv.cc.cc.lib hdf5 ];
          LIBCLANG_PATH = "${pkgs.libclang.lib}/lib";
        };
        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        # The Python extension module (#210). crane doesn't install cdylibs by default, so copy the
        # built `lib_native.so` → `_native.so` into $out/lib — the `tessera._native` extension that the
        # pure-Python `tessera/__init__.py` wraps. pyo3's abi3 feature builds without a runtime
        # interpreter, so no python is needed at compile time.
        tessera-py-lib = craneLib.buildPackage (commonArgs // {
          inherit cargoArtifacts;
          pname = "tessera-py";
          cargoExtraArgs = "-p tessera-py";
          doCheck = false;
          postInstall = ''
            mkdir -p $out/lib
            cp target/release/lib_native.so $out/lib/_native.so
          '';
        });

        # Cloud-featured `tessera` binary for the registry round-trip check (pulls reqwest — the
        # in-Rust OCI distribution client). Shares cargoArtifacts; the cloud-only crates build on top
        # (same pattern as `minio-range-read`).
        tessera-cli-cloud = craneLib.buildPackage (commonArgs // {
          inherit cargoArtifacts;
          pname = "tessera-cli-cloud";
          cargoExtraArgs = "-p tessera-cli --features cloud";
          doCheck = false;
        });

        # The installable `tessera` CLI (the nix-native distribution channel — `nix run` /
        # `nix profile install`, complementing cargo-dist's prebuilt binaries for non-nix users).
        # Default features: libhdf5 comes from the nix closure (buildInputs), so — unlike the
        # cargo-dist binaries — this does NOT need the `static-hdf5` vendored build; nix ships hdf5
        # in the runtime closure. Reproducible by construction. `mainProgram` lets `nix run` resolve
        # the binary name (`tessera`) without an explicit `#`-attr.
        tessera-cli = craneLib.buildPackage (commonArgs // {
          inherit cargoArtifacts;
          pname = "tessera-cli";
          cargoExtraArgs = "-p tessera-cli";
          doCheck = false;
          meta.mainProgram = "tessera";
        });

        # The reproducible tessera-py Python wheel (#210). tessera-py is a pure pyo3/abi3 extension
        # with NO native deps (tessera-core + tessera-io only — no libhdf5), so the wheel is just the
        # crane-built `_native.so` + the pure-Python `tessera/` wrapper, assembled and `wheel pack`ed.
        # abi3-py39 → one wheel serves CPython ≥3.9 (tag `cp39-abi3`). The platform tag is the honest
        # `linux_<arch>` — this is a nix build, not a manylinux one; auditwheel/manylinux repair for a
        # PyPI upload is a deliberate follow-up (the wheel installs & imports in a compatible-glibc env
        # today, which the `tessera-wheel-import` check proves).
        tessera-wheel = let
          pyVersion = "0.1.0a1"; # PEP 440 form of the workspace 0.1.0-alpha.1
          arch = pkgs.stdenv.hostPlatform.parsed.cpu.name; # x86_64 / aarch64
          wheelName = "tessera-${pyVersion}-cp39-abi3-linux_${arch}.whl";
        in
        pkgs.runCommand "tessera-wheel-${pyVersion}"
          {
            nativeBuildInputs = [ (pkgs.python312.withPackages (ps: [ ps.wheel ])) ];
            passthru = { inherit wheelName; };
          } ''
          root=$PWD/wheel
          mkdir -p "$root/tessera" "$root/tessera-${pyVersion}.dist-info"
          cp -r ${./tessera/crates/tessera-py/python/tessera}/. "$root/tessera/"
          cp ${tessera-py-lib}/lib/_native.so "$root/tessera/_native.so"

          cat > "$root/tessera-${pyVersion}.dist-info/METADATA" <<EOF
          Metadata-Version: 2.1
          Name: tessera
          Version: ${pyVersion}
          Summary: FAIR data products — read / verify / write .tsra from Python (pyo3, abi3)
          License: Apache-2.0
          Home-page: https://github.com/vig-os/tessera
          Project-URL: Source, https://github.com/vig-os/tessera
          Project-URL: Documentation, https://github.com/vig-os/tessera#readme
          Classifier: License :: OSI Approved :: Apache Software License
          Classifier: Programming Language :: Python :: 3
          Classifier: Programming Language :: Rust
          Requires-Python: >=3.9
          Requires-Dist: numpy
          Provides-Extra: tables
          Requires-Dist: polars ; extra == 'tables'
          Requires-Dist: pyarrow ; extra == 'tables'
          EOF

          cat > "$root/tessera-${pyVersion}.dist-info/WHEEL" <<EOF
          Wheel-Version: 1.0
          Generator: tessera-nix
          Root-Is-Purelib: false
          Tag: cp39-abi3-linux_${arch}
          EOF

          # `wheel pack` (re)generates the RECORD (sha256 + size per file) and zips deterministically.
          mkdir -p "$out"
          python -m wheel pack --dest-dir "$out" "$root"
        '';
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
            # Hook binaries. These MUST come from Nix: prek's own installers fetch
            # generic-linux dynamically-linked binaries (typos) or build wheels whose
            # interpreter tag must match (shellcheck-py), and neither works on NixOS —
            # a hook that fails to *install* silently disables every hook declared after
            # it in .pre-commit-config.yaml.
            typos
            shellcheck
            ruff

            # devkit CI toolchain (vigos overlay): the managed ci.yml's
            # commit-checks job resolves `validate-commit-range` and
            # `check-pr-agent-fingerprints` off this dev-shell in direnv mode.
            vig-utils

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
          ]) ++ [
            # pymarkdown CLI, packaged by devkit (nix/pymarkdown.nix) rather than
            # exported on the overlay: the managed `.pre-commit-config.yaml` runs
            # the `pymarkdown` hook as a system command, and prek's own installer
            # cannot build it on NixOS.
            (import "${vigos}/nix/pymarkdown.nix" pkgs)
          ];
          env = {
            # bindgen (dicom-rs) needs libclang.
            LIBCLANG_PATH = "${pkgs.libclang.lib}/lib";
            # NOTE: do NOT set HDF5_DIR — nix splits hdf5 headers into a separate `dev` output, so a
            # single root lacks include/; `hdf5-metno-sys` finds hdf5 via pkg-config instead (the
            # hdf5 dev pkgconfig is on PKG_CONFIG_PATH from buildInputs).
            # uv uses the Nix python and never downloads its own → reproducible
            UV_PYTHON = "${pkgs.python312}/bin/python3.12";
            UV_PYTHON_DOWNLOADS = "never";
          };
          hook = ''
            # uv-installed manylinux wheels (numpy/h5py/pyarrow/...) dlopen libstdc++.so.6
            # and libz.so.1 at runtime; the pure devShell doesn't expose them on the loader
            # path, so prepend the C++ runtime + zlib without clobbering whatever's already set.
            export LD_LIBRARY_PATH="${pkgs.stdenv.cc.cc.lib}/lib:${pkgs.zlib}/lib''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
            echo "[tessera] rust $(rustc --version 2>/dev/null | cut -d' ' -f2) · python ${pkgs.python312.version} · uv $(uv --version 2>/dev/null | cut -d' ' -f2)"
            echo "[tessera] cargo workspace lives in ./tessera  (cd tessera && cargo test)"
          '';
        };

        # ── Installable artifacts (the nix-native distribution channel). ──
        #    `nix run github:vig-os/tessera`         → run the CLI without installing
        #    `nix profile install github:vig-os/tessera` → install `tessera` onto PATH
        #    `nix build .#wheel`                      → the reproducible tessera-py wheel
        #    Complements cargo-dist's prebuilt binaries (which target non-nix users); here nix
        #    supplies the whole runtime closure (incl. libhdf5), so these need no vendored static build.
        packages = {
          default = tessera-cli;
          tessera = tessera-cli;
          tessera-cloud = tessera-cli-cloud;
          wheel = tessera-wheel;
        };

        apps = rec {
          default = tessera;
          tessera = flake-utils.lib.mkApp {
            drv = tessera-cli;
            name = "tessera";
          };
        };

        # ── CI = a shim over `nix flake check`. The logic lives HERE so the exact same command
        #    runs on a dev's machine and in CI — no "passes locally / fails in CI" drift. ──
        # Every check must have a DISTINCT derivation name (#535). `nix flake check -L` prefixes each
        # log line with the derivation *name*, so two checks that share one are indistinguishable in
        # CI logs — nothing errors, the logs just interleave. That happened: `workspace-test`,
        # `sql-tests` and `minio-range-read` were all `tessera-nextest-0.0.0` (crane's default
        # `-nextest` suffix), and a whole diagnosis (#517/#518/#519) was built on counting three
        # builds as one. This fails EVALUATION, so it runs on every `nix flake check`, locally too.
        checks = let
          allChecks = {
          # Hermetic Rust gates over the tessera workspace.
          #
          # NOT `--all-features` (ADR-0057 §4): that pulls `static-hdf5`, which builds libhdf5 2.2.0
          # from vendored source via CMake and dominated the ~90 min x86_64 check. `static-hdf5` only
          # switches how libhdf5 *links* — there is not one `#[cfg(feature = "static-hdf5")]` in the
          # tree — so dropping it from the PR matrix costs **zero** clippy coverage: nix supplies
          # libhdf5 from the closure (`buildInputs`), and every line of hdf5 code still compiles here.
          # It stays exercised release-only, by the cargo-dist channel (`dist-workspace.toml` sets
          # `features = ["static-hdf5"]`).
          #
          # `workspaceFeatures` is therefore the explicit "every workspace feature EXCEPT static-hdf5"
          # set. It must be kept exhaustive by hand — cargo has no `--all-features-except`. Adding a
          # feature to any crate means adding it here (the `feature-snapshots` check below will also
          # notice, since a new feature moves the resolved graph).
          workspace-clippy = craneLib.cargoClippy (commonArgs // {
            inherit cargoArtifacts;
            cargoClippyExtraArgs =
              "--all-targets --features ${workspaceFeatures} -- -D warnings";
          });
          workspace-test = craneLib.cargoNextest (commonArgs // {
            inherit cargoArtifacts;
            partitions = 1;
            partitionType = "count";
          });
          # nextest does NOT run doctests, so gate them separately — the "docs-as-tests" layer: every
          # `///` example compiles + runs, so public-API docs can never drift from behaviour.
          workspace-doctest = craneLib.cargoTest (commonArgs // {
            inherit cargoArtifacts;
            cargoTestExtraArgs = "--doc";
          });
          workspace-fmt = craneLib.cargoFmt { inherit src; };

          # **The third determinism axis — the build configuration** (ADR-0057 §5, #468).
          #
          # Gate A varies the *feature configuration*; Gate B varies the *dependency graph*. Neither
          # varies the compiler's own cfg flags, and that is where #468 hid: vortex 0.75.0 writes
          # different container bytes for an integer column whose encoding produces patches, depending
          # on whether `debug_assertions` is enabled (an assertion with a side effect —
          # `vortex-array/src/patches.rs` evaluates `is_sorted` on the patch indices, which annotates
          # that array, and the written file reflects it).
          #
          # This gate closes the axis for the sealed corpus: the goldens are regenerated under BOTH the
          # dev and the release profile and must agree with each other and with the committed file. Note
          # that the committed `corpus/corpus.json` was historically produced by a bare `cargo run`
          # (dev profile) while `workspace-test` runs the same code release-built — so before #468 the
          # two halves of our own CI were checking different builds and neither compared them.
          #
          # Scoped to `-p tessera-io` on purpose: the point is the seal path, and a second full-workspace
          # profile would roughly double this leg's build time for no extra coverage.
          #
          # COST, stated rather than discovered later: `cargoArtifacts` (`buildDepsOnly`) pre-builds the
          # **release** dependency graph, so the release half of this check is warm while the dev half
          # compiles vortex and friends from source on every run. That is the bulk of this leg's time.
          # The fix would be a second `buildDepsOnly` pinned to the dev profile, which crane does not
          # make convenient (a derivation inherits one `cargoArtifacts`), so it is left as a known cost
          # rather than a speculative nix refactor. If this leg becomes the critical path, the cheaper
          # move is to run it release-only on PRs and keep both profiles for `main`.
          seal-profile-determinism = craneLib.mkCargoDerivation (commonArgs // {
            inherit cargoArtifacts;
            doInstallCargoArtifacts = false;
            pnameSuffix = "-seal-profile-determinism";
            buildPhaseCargoCommand = ''
              echo "[#468] conformance corpus under the dev and release profiles" >&2
              cargo run -q -p tessera-io --example gen_corpus > "$TMPDIR/corpus.dev.json"
              cargo run -q --release -p tessera-io --example gen_corpus > "$TMPDIR/corpus.release.json"

              if ! diff -u "$TMPDIR/corpus.dev.json" "$TMPDIR/corpus.release.json"; then
                echo "" >&2
                echo "The sealed corpus DEPENDS ON THE BUILD PROFILE (ADR-0057 §5, #468)." >&2
                echo "A content_hash must be a function of the data alone. Two builds of one commit" >&2
                echo "disagreeing means the format's identity is a function of how it was compiled." >&2
                echo "This is not a golden to regenerate — find what made the bytes configuration-" >&2
                echo "dependent. #468 is the worked example: an assertion with a side effect." >&2
                exit 1
              fi
              if ! cmp -s "$TMPDIR/corpus.dev.json" corpus/corpus.json; then
                echo "" >&2
                echo "Both profiles agree with each other but NOT with corpus/corpus.json." >&2
                echo "That is an ordinary moved golden — regenerate deliberately with:" >&2
                echo "    cargo run -p tessera-io --example gen_corpus > corpus/corpus.json" >&2
                exit 1
              fi

              # #468 is FIXED (carried by the vortex fork pin, #480). The test now asserts ONE byte
              # count per shape with no `cfg!(debug_assertions)` branch, so running it in both profiles
              # is what proves the two agree: a re-divergence fails in exactly one of these two lines.
              echo "[#468] the build-config independence guard under both profiles" >&2
              cargo test -q -p tessera-io --lib full_span_int_container_bytes_are_build_config_independent
              cargo test -q --release -p tessera-io --lib full_span_int_container_bytes_are_build_config_independent

              # #472's guard (the arch-dependent half): the persisted float Sum must be the canonical
              # NaN, not x86_64's default. Run in both profiles for symmetry with the above, though
              # this one's axis is the architecture — the aarch64 leg of this matrix is what pairs
              # with it, and aarch64 cannot fail it (it always produced the canonical value).
              echo "[#472] the canonical-NaN Sum guard under both profiles" >&2
              cargo test -q -p tessera-io --lib float_sum_stat_is_canonical_nan_not_the_platform_default
              cargo test -q --release -p tessera-io --lib float_sum_stat_is_canonical_nan_not_the_platform_default
            '';
          });

          # **Gate B — the feature-snapshot determinism gate** (ADR-0057 §5). Regenerates the resolved
          # feature graph of every crate on the seal path and diffs it against the committed baseline
          # in `tessera/tests/feature-snapshots/` — the hermetic equivalent of the ADR's
          # `git diff --exit-code` (there is no `.git` inside the nix sandbox).
          #
          # Gate A (Phase 1) catches a golden that moved; Gate B catches the *risk* on the PR that
          # introduced it. It is what would have flagged `sql` turning on `arrow-array/chrono-tz`
          # — ADR-0056 hazard H1 (tzdb: compiled-in vs. host `/usr/share/zoneinfo`) arriving through
          # a feature rather than a host — on the PR that added `sql`, instead of on the release that
          # shipped an ingest path through it. Feature unification is monotonic, so the same class of
          # hazard would silently re-register ALP in the Vortex float compressor (#380/#384).
          #
          # A failure is NOT automatically a bug: it is a deliberate corpus event. Regenerate with
          # `scripts/feature-snapshots.sh tessera/tests/feature-snapshots` and either show no golden
          # moved (stating why in the PR) or carry the corpus regeneration alongside.
          #
          # `cargo tree` reads the lockfile + the vendored manifests; it compiles nothing, so this
          # check is nearly free (`cargoArtifacts = null` — there is no target dir to inherit).
          feature-snapshots = craneLib.mkCargoDerivation (commonArgs // {
            cargoArtifacts = null;
            doInstallCargoArtifacts = false;
            pnameSuffix = "-feature-snapshots";
            buildPhaseCargoCommand = ''
              TESSERA_WORKSPACE="$PWD" bash ${./scripts/feature-snapshots.sh} "$TMPDIR/snapshots"
              # `--exclude='*.md'` skips the directory's README (the reviewer-facing explainer);
              # every `<crate>.txt` is still compared, and a snapshot that vanished still shows up
              # as an "Only in …" line.
              if ! diff -ru --exclude='*.md' tests/feature-snapshots "$TMPDIR/snapshots"; then
                echo "" >&2
                echo "Gate B (ADR-0057 §5): the resolved feature graph of a seal-path crate CHANGED." >&2
                echo "This is a deliberate corpus event — see the diff above, then regenerate with:" >&2
                echo "    scripts/feature-snapshots.sh tessera/tests/feature-snapshots" >&2
                echo "and justify it in the PR (no golden moved, or the corpus regen rides along)." >&2
                exit 1
              fi
            '';
          });

          # **Gate A — the ingest determinism gate** (ADR-0057 §5, ADR-0056 §5). Where Gate B above
          # catches the *risk* on the PR that introduces it, Gate A catches a golden that actually moved.
          #
          # The guarantee under test, stated so it can be cited (ADR-0057 §7):
          #
          #   For any input F and any tessera version V, `tessera ingest F` produces the same
          #   content_hash under every distributed build of V. Feature selection may change which
          #   formats are READABLE; it must never change the BYTES produced for a readable one.
          #
          # So the ingest corpus is regenerated under several feature configurations and every output
          # must be byte-identical to the committed `corpus/ingest-corpus.json`.
          #
          # **One derivation per configuration, not one derivation running all of them.** Each
          # configuration recompiles the crates its features touch, and doing four in a single sandbox
          # exhausted the disk on a host whose `/` was near full (`No space left on device`, after the
          # gate logic itself had passed). Splitting them lets nix schedule and reclaim between builds,
          # and a failure names the offending configuration in the derivation name instead of in a log.
          #
          # The `sql` configuration is the one that earns this gate. ADR-0057 §5's probe found that the
          # optional `sql` feature turns on `arrow-array/chrono-tz` — inside the shared arrow tree the
          # ingest table lane decodes through — which is ADR-0056 hazard **H1** (tzdb: compiled-in vs.
          # host `/usr/share/zoneinfo`) arriving through a *feature* rather than a host. The
          # `ingest_parquet_scalars` fixture carries a `timestamp(us, "America/New_York")` column
          # precisely so that door is watched. It is closed by construction as well (the type map reads
          # the raw i64 ticks and never calls a zone-aware arrow function) but "by construction" is a
          # claim, and this is the test of it.
          ingest-gate-a = ingestGateA "default" "";
          ingest-gate-a-sql = ingestGateA "sql" "--features tessera-cli/sql";
          ingest-gate-a-workspace = ingestGateA "workspace-features" "--features ${workspaceFeatures}";

          # The REDUCED configuration cannot be byte-compared (it produces fewer fixtures), so it is
          # checked by the corpus test itself, which compares field-wise over the fixtures it can run
          # AND asserts the declared count for that configuration (ADR-0057 §5's anti-vacuity guard).
          #
          # This is the leg that catches a `manifest_hash` depending on which lanes were compiled in.
          # An earlier derivation of the sealed `ingest_decoder` digest did exactly that — it hashed this
          # crate's own resolved feature set, so a build without the CSV lane sealed a different
          # `manifest_hash` for the same Parquet — and this configuration is what surfaced it. Keeping it
          # in CI is what stops that class of bug coming back.
          #
          # **All the reduced configurations share ONE derivation, deliberately** (#495). `nix flake check`
          # schedules derivations concurrently under a `max-jobs` cap, so an extra check does not raise
          # concurrency past that cap — but it does change *which* checks can be co-scheduled, and a new
          # one can create a heavier overlap than the previous mix allowed. Measured: splitting the
          # `npy`-without-`npz` configuration into its own derivation coincided with aarch64 peak memory
          # rising 11536M -> 15095M and free memory falling 4410M -> 852M, against runners that evict at
          # ~15.6G.
          #
          # **Those figures were measured under `--max-jobs 4`, before #514 halved it to 2, so the effect
          # they show is larger than what folding buys today.** With a co-scheduling window of two, there
          # are far fewer permutations for a new derivation to land a bad pairing in, and #514 is the
          # actual bound — adding derivations now costs wall-clock rather than peak memory. What remains
          # is one fewer permutation inside that smaller window, which is worth having and is not worth
          # overstating: this is tidiness with a measured origin, not the fix for #495.
          #
          # The trade accepted in exchange: these run sequentially, so a failure in an earlier
          # configuration hides the later ones until it is fixed. Cheap, because each is seconds of
          # `cargo test` against an already-built dependency graph, and the configurations are
          # independent enough that one failing rarely predicts another.
          ingest-gate-a-reduced = craneLib.mkCargoDerivation (commonArgs // {
            inherit cargoArtifacts;
            doInstallCargoArtifacts = false;
            pnameSuffix = "-ingest-gate-a-reduced";
            buildPhaseCargoCommand = ''
              # (1) The columnar lane without the text one — the leg described above.
              cargo test -p tessera-ingest --no-default-features --features parquet \
                --test ingest_corpus

              # (2)+(3) **The array lane with NO archive reader** (#386) — `npy` compiled without `npz`.
              #
              # `.npy` is parsed in-tree and `.npz` is that same parser behind `dep:zip`, so they are two
              # lanes rather than one feature (ADR-0056's #386 amendment): while they shared a feature,
              # every plain `.npy` seal committed to a zip library that never read its bytes — #477's
              # defect one scale down. Two lanes only mean something if the split is *exercised*, and
              # these are the only configurations where `zip` is genuinely absent from the graph.
              #
              # Two of them, because they fail differently. `npy` alone proves the parser needs no
              # archive reader; it caught `generic_column_meta` and
              # `warn_unclassified_identifying_columns` being gated on `any(parquet, arrow, csv)` while
              # the array arm calls both, plus three targets that hard-require a Parquet reader and now
              # declare `required-features`. `parquet,npy` is the one that reaches the CORPUS without
              # `npz` — the corpus module is gated on `parquet`, so `npy` alone never compiles it — and
              # it caught the `ingest_npz_member` fixture being declared `requires: ["npy"]` while its
              # builder calls `zip`. Neither configuration had ever been built before it was gated.
              cargo test -p tessera-ingest --no-default-features --features npy --lib
              cargo test -p tessera-ingest --no-default-features --features parquet,npy \
                --lib --test ingest_corpus
            '';
          });

          # **ADR-0056 §5's `ingest_parquet_producers` fixture** — the same logical table written by
          # **pyarrow**, **polars** and **DuckDB** must ingest to ONE `content_hash`.
          #
          # This is the check that catches what the in-Rust corpus can only approximate. The three
          # writers disagree about dictionary ordering (pandas' `Categorical` order is insertion-time)
          # and about whether a null-free column is *declared* nullable — the two leaks ADR-0056 §2
          # materialises dictionaries and decides nullability-by-data to prevent. arrow-rs's own writer
          # cannot test that the fix generalises beyond arrow-rs.
          #
          # It also asserts the three source files differ in size, so the comparison cannot pass
          # vacuously, and that `manifest_hash` DOES differ (the `ingested_from` edge pins the source
          # bytes) — so a bug that made every hash constant would fail rather than look perfect.
          ingest-producer-equality = pkgs.runCommand "ingest-producer-equality"
            {
              nativeBuildInputs = [
                (pkgs.python312.withPackages (ps: [ ps.pyarrow ps.polars ps.duckdb ]))
              ];
            } ''
            python3 ${./tessera/crates/tessera-ingest/tests/producer_equality.py}               ${tessera-cli}/bin/tessera
            touch $out
          '';

          # `tessera-core` must stay **wasm32-compatible** (#210): the pure-Rust spine — manifest /
          # identity / hash / inclusion+consistency proofs / referencing / ed25519 *verify* — has zero
          # C deps and no getrandom in its production graph (getrandom enters only via the proptest
          # dev-dep), so it compiles to wasm for in-browser verification. We build the spine ONLY here as
          # the minimal verified boundary; today's RS↔TS data hop is Arrow-JS. NB: Vortex itself IS
          # wasm-capable (`vortex-io` ships a `WasmRuntime`); the real wasm blockers live in the *zarrs
          # array* path (`zstd-sys` C + `linux-raw-sys` filesystem) + `getrandom` (needs `wasm_js`), not in
          # Vortex — so a richer core+Vortex-table-decode wasm build is reachable (tracked in #224).
          # Builds the lib only, no test run (wasm can't execute natively here); deps scoped to the subtree.
          wasm-core = let
            wasmArgs = {
              inherit src;
              strictDeps = true;
              pname = "tessera-core-wasm";
              version = "0.0.0";
              cargoExtraArgs = "--package tessera-core --lib";
              CARGO_BUILD_TARGET = "wasm32-unknown-unknown";
              doCheck = false;
            };
          in
          craneLib.buildPackage (wasmArgs // {
            cargoArtifacts = craneLib.buildDepsOnly wasmArgs;
          });

          # The **wasm-bindgen JS bindings** (`tessera-wasm`, #210) must build to wasm AND load + execute
          # in a JS runtime: build the cdylib for wasm32, generate the Node bindings with `wasm-bindgen`
          # (pinned to the same 0.2.121 as the crate — they MUST match), then run the Node smoke test
          # (functions callable, panic-safe boundary). Proves in-browser verification end-to-end.
          wasm-bindgen-smoke = let
            wasmArgs = {
              inherit src;
              strictDeps = true;
              pname = "tessera-wasm-bindings";
              version = "0.0.0";
              cargoExtraArgs = "--package tessera-wasm --lib";
              CARGO_BUILD_TARGET = "wasm32-unknown-unknown";
              doCheck = false;
            };
            wasmLib = craneLib.buildPackage (wasmArgs // {
              cargoArtifacts = craneLib.buildDepsOnly wasmArgs;
              # crane doesn't install a cdylib's `.wasm` by default — copy it out for wasm-bindgen.
              postInstall = ''
                mkdir -p $out/wasm
                find target -name 'tessera_wasm.wasm' -exec cp {} $out/wasm/ \;
              '';
            });
          in
          pkgs.runCommand "wasm-bindgen-smoke"
            { nativeBuildInputs = [ pkgs.wasm-bindgen-cli pkgs.nodejs ]; } ''
            wasm-bindgen --target nodejs --out-dir "$PWD/glue" ${wasmLib}/wasm/tessera_wasm.wasm
            node ${./tessera/crates/tessera-wasm/tests/smoke.cjs} "$PWD/glue/tessera_wasm.js"
            touch $out
          '';

          # **OCI artifact distribution** (#209): a sealed `.tsra` round-trips through a real OCI registry
          # — push it as an artifact (Tessera `artifactType` + `.tsra` layer media type), confirm the
          # registry-stored manifest matches the `tessera_io::oci` constants (artifactType · layer media
          # type · the OCI empty-config digest), then pull it back byte-for-byte. Spins up a local
          # `distribution` registry on loopback (the nix-CI service mock — only a *production* registry is
          # external), so the OCI push/pull capability is gated, not just the manifest-construction unit.
          oci-roundtrip = pkgs.runCommand "oci-roundtrip"
            { nativeBuildInputs = [ pkgs.distribution pkgs.oras pkgs.curl ]; } ''
            export HOME=$TMPDIR
            cat > $TMPDIR/config.yml <<EOF
            version: 0.1
            storage:
              filesystem:
                rootdirectory: $TMPDIR/data
            http:
              addr: 127.0.0.1:5050
            EOF
            registry serve $TMPDIR/config.yml > $TMPDIR/reg.log 2>&1 &
            for i in $(seq 1 80); do curl -sf http://127.0.0.1:5050/v2/ > /dev/null && break; sleep 0.25; done
            cp ${./tessera/corpus/files}/recon_int16.tsra $TMPDIR/p.tsra
            cd $TMPDIR
            oras push --plain-http 127.0.0.1:5050/tessera/recon:v1 \
              --artifact-type application/vnd.tessera.product.v1+json \
              p.tsra:application/vnd.tessera.tsra.v1
            M=$(oras manifest fetch --plain-http 127.0.0.1:5050/tessera/recon:v1)
            echo "$M" | grep -q 'application/vnd.tessera.product.v1+json'   # ARTIFACT_TYPE
            echo "$M" | grep -q 'application/vnd.tessera.tsra.v1'           # TSRA_MEDIA_TYPE
            echo "$M" | grep -q 'sha256:44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a'  # OCI empty config
            mkdir out && oras pull --plain-http 127.0.0.1:5050/tessera/recon:v1 -o out
            cmp p.tsra out/p.tsra
            touch $out
          '';

          # **In-Rust OCI registry push/pull** (the `tessera push`/`pull` distribution client — NOT
          # oras): a cloud-featured `tessera` binary pushes a corpus `.tsra` to a loopback
          # `distribution` registry as an OCI artifact (reqwest blob-upload + manifest PUT), then
          # pulls it back (manifest GET → sha256-verified blob GET) byte-for-byte and verifies the
          # seal. Complements `oci-roundtrip` (which proves the manifest *shape* via oras) by proving
          # our actual transport client end to end.
          registry-roundtrip = pkgs.runCommand "registry-roundtrip"
            {
              nativeBuildInputs = [ pkgs.distribution pkgs.curl ];
              # reqwest's client builder loads the system CA store even for a plain-http loopback
              # target (same as `minio-range-read`); point it at the nixpkgs bundle.
              SSL_CERT_FILE = "${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt";
            } ''
            export HOME=$TMPDIR
            cat > $TMPDIR/config.yml <<EOF
            version: 0.1
            storage:
              filesystem:
                rootdirectory: $TMPDIR/data
            http:
              addr: 127.0.0.1:5055
            EOF
            registry serve $TMPDIR/config.yml > $TMPDIR/reg.log 2>&1 &
            for i in $(seq 1 80); do curl -sf http://127.0.0.1:5055/v2/ > /dev/null && break; sleep 0.25; done
            cp ${./tessera/corpus/files}/recon_int16.tsra $TMPDIR/p.tsra
            cd $TMPDIR
            # Sign with the LEGACY detached form (`--sidecar`), so push must carry `p.tsra.sig.json`
            # through the registry as a second layer (ADR-0037 §0 bug #2 regression test). The
            # embedded default (ADR-0042) rides inside the `.tsra` bytes and is trivially proven
            # by the `cmp p.tsra out.tsra` line above.
            printf '0101010101010101010101010101010101010101010101010101010101010101' > key.hex
            ${tessera-cli-cloud}/bin/tessera sign p.tsra --key key.hex --sidecar --signer https://orcid.org/0000-0002-1825-0097
            test -f p.tsra.sig.json
            ${tessera-cli-cloud}/bin/tessera push p.tsra 127.0.0.1:5055/tessera/recon:v1 --plain-http
            ${tessera-cli-cloud}/bin/tessera pull 127.0.0.1:5055/tessera/recon:v1 out.tsra --plain-http
            ${tessera-cli-cloud}/bin/tessera verify out.tsra
            cmp p.tsra out.tsra
            # The signature sidecar survived the registry hop, byte-identical (bug #2 fixed).
            test -f out.tsra.sig.json
            cmp p.tsra.sig.json out.tsra.sig.json
            touch $out
          '';

          # **Cloud range-read** (#225, landed): a sealed `.tsra` is range-readable directly from
          # object storage — prune-before-fetch over the wire, not a full download — AND a query
          # that does not overlap a product's stat range never fetches that product's data block
          # (the cohort prune-before-fetch capstone, proven across two products in the same
          # bucket). Spins up a local MinIO on loopback (S3-compatible; same "real service mock"
          # pattern as `oci-roundtrip` above), creates a bucket, then runs every test in the
          # `cloud` module of `tessera-io` PLUS the `tessera-cli` integration test that drives
          # `tessera verify <s3://...>` against the real binary. Mirrors the `oci-roundtrip`
          # idle-wait loop verbatim.
          #
          # The tests cover:
          #   - `cloud::tests::s3_range_read_does_not_fetch_whole_archive`  (single-product range)
          #   - `cloud::tests::cohort_prune_before_fetch_skips_non_matching_product` (capstone)
          #   - `cloud::tests::tail_prefetch_*` (in-memory; runs always under the cloud feature)
          #   - `tessera-cli::tests/cloud.rs::cli_inspect_and_verify_over_s3_url` (CLI end-to-end)
          #
          # NB: nixpkgs marks `minio` insecure (abandoned upstream); we whitelist *just here* by
          # stripping `knownVulnerabilities`. The test mock isn't network-reachable — it binds
          # 127.0.0.1 inside the nix sandbox — so the upstream-CVE exposure surface is nil.
          minio-range-read = let
            # nixpkgs marks this minio INSECURE (`knownVulnerabilities`), so evaluating it refuses
            # outright: "Refusing to evaluate package 'minio-…' because it is marked as insecure".
            # Stripping the marker is a deliberate, narrow decision, recorded here at the point it is
            # made rather than left implicit (#483):
            #
            #   - it is a TEST FIXTURE, never shipped and never part of any tessera artifact. The
            #     check starts a MinIO on 127.0.0.1:9101 inside the nix build sandbox, which has no
            #     network access and no persistent state, and throws it away with the derivation.
            #   - the CVEs are in minio's server/console surface reached over a network by untrusted
            #     clients. Here the only client is the test itself, over loopback, with a throwaway
            #     credential pair that is also in this file.
            #   - the alternative is dropping the check, which would mean the cloud range-read path
            #     (ADR-0002 §4, prune-before-fetch over real S3 semantics) has no test against a real
            #     object store at all. A sandboxed known-CVE fixture is the lesser risk.
            #
            # `meta` is not part of the derivation, so this override changes no output hash: the
            # binary built here is byte-identical to the one nixpkgs would give.
            #
            # Revisit if minio ever becomes reachable from outside the sandbox, or if the check starts
            # handling anything that is not synthetic test data.
            minio = pkgs.minio.overrideAttrs (old: {
              meta = (old.meta or { }) // { knownVulnerabilities = [ ]; };
            });
          in
          craneLib.cargoNextest (commonArgs // {
            inherit cargoArtifacts;
            # Distinct from `workspace-test` so CI logs attribute its lines (#535). Via `pname`, not
            # `pnameSuffix`: crane's cargoNextest hard-sets `pnameSuffix = "-nextest${extraSuffix}"`
            # and silently discards a caller's value -> derivation name `tessera-cloud-nextest-0.0.0`.
            pname = "tessera-cloud";
            # Run both crates' cloud tests in one MinIO session (shared bucket): the `cloud::`
            # module in tessera-io for unit-level range/cohort/tail-prefetch coverage, AND the
            # `cloud_cli_inspect_and_verify_over_s3_url` test in tessera-cli for the binary
            # end-to-end. Single `cloud` positional filter catches both — every test name with
            # cloud coverage starts with `cloud` (module or function), nothing unrelated does.
            # `tessera-cli/cloud` transitively enables `tessera-io/cloud` (the CLI feature
            # depends on the IO feature), so a single `--features` flag covers both crates.
            cargoExtraArgs = "-p tessera-io -p tessera-cli --features tessera-cli/cloud cloud";
            # Top-level env vars become derivation env vars (Nixpkgs stdenv default) — propagate to
            # `cargo nextest run`, which the test reads via `std::env::var`.
            TESSERA_S3_ENDPOINT = "http://127.0.0.1:9101";
            TESSERA_S3_BUCKET = "tessera-test";
            AWS_ACCESS_KEY_ID = "minioadmin";
            AWS_SECRET_ACCESS_KEY = "minioadmin";
            AWS_REGION = "us-east-1";
            # object_store/aws is reqwest-backed; reqwest's client builder loads the system CA trust
            # store even for a plain-http (allow_http) loopback target, and the hermetic sandbox has
            # none → "No CA certificates were loaded". Point it at the nixpkgs CA bundle.
            SSL_CERT_FILE = "${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt";
            # Start MinIO right before `cargo nextest run` (build doesn't need it). Same loopback +
            # idle-wait pattern as `oci-roundtrip` above. Bucket creation via awscli2's `s3 mb` —
            # MinIO requires the bucket to exist before `PutObject`.
            preCheck = ''
              export HOME=$TMPDIR
              export MINIO_ROOT_USER=minioadmin
              export MINIO_ROOT_PASSWORD=minioadmin
              mkdir -p $TMPDIR/miniodata
              ${minio}/bin/minio server $TMPDIR/miniodata --address 127.0.0.1:9101 \
                > $TMPDIR/minio.log 2>&1 &
              for i in $(seq 1 160); do
                ${pkgs.curl}/bin/curl -sf http://127.0.0.1:9101/minio/health/ready && break
                sleep 0.25
              done
              # `health/ready` can go green before the S3 API actually accepts `CreateBucket`
              # (notably on the slower aarch64 runner — `XMinioServerNotInitialized`), so retry the
              # bucket op itself until it lands rather than firing it once and racing.
              for i in $(seq 1 120); do
                ${pkgs.awscli2}/bin/aws --endpoint-url http://127.0.0.1:9101 \
                  s3 mb s3://tessera-test 2>/dev/null && break
                sleep 0.5
              done
              # Fail the check clearly if the bucket never materialised (vs a confusing later error).
              ${pkgs.awscli2}/bin/aws --endpoint-url http://127.0.0.1:9101 \
                s3 ls s3://tessera-test > /dev/null
            '';
            nativeBuildInputs = commonArgs.nativeBuildInputs
              ++ [ minio pkgs.awscli2 pkgs.curl pkgs.cacert ];
          });

          # **DataFusion SQL surface** (#251 spike): the `sql` feature adds `tessera sql <FILE>
          # "<QUERY>"` — Vortex → Arrow → DataFusion in-process. The unit tests under `sql::tests`
          # are `#[cfg(feature = "sql")]`, so the default `workspace-test` gate does NOT run them
          # (default features exclude the ~200-crate DataFusion tree). This dedicated check runs
          # the CLI's sql-feature tests only — same pattern as `minio-range-read` for the cloud
          # feature. Clippy already covers `--all-features` at compile-time, so this test is what
          # proves the query actually EXECUTES (WHERE + ORDER BY + LIMIT round-trip).
          sql-tests = craneLib.cargoNextest (commonArgs // {
            inherit cargoArtifacts;
            # Distinct from `workspace-test` so CI logs attribute its lines (#535). Via `pname`, not
            # `pnameSuffix`: crane's cargoNextest hard-sets `pnameSuffix = "-nextest${extraSuffix}"`
            # and silently discards a caller's value -> derivation name `tessera-sql-nextest-0.0.0`.
            pname = "tessera-sql";
            # `sql::tests::*` runs the DataFusion SessionContext path end-to-end (register table,
            # execute query, collect batches). The positional `sql` filter matches every test
            # name in the `sql` module — DataFusion 54 pins arrow-58, so cargo unifies with the
            # arrow-58 the workspace already has (via Vortex 0.75); a version mismatch would
            # fail to link, not to test.
            cargoExtraArgs = "-p tessera-cli --features sql sql::";
          });

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
              && guardrails-adr-matrix docs/adr/README.md tessera/docs/FEATURE-MATRIX.md \
              && touch $out
          '';

          # The Python bindings import + verify the committed corpus through the real package (#210):
          # assembles the `tessera/` package (pure-Python `__init__.py` + the `_native.so` extension),
          # then runs the smoke test, which exercises the ERGONOMIC surface — numpy ndarrays, polars
          # DataFrames, pyarrow Tables — proving the abi3 extension loads on CPython AND the wrapper
          # returns native objects end-to-end.
          tessera-py-import = pkgs.runCommand "tessera-py-import"
            {
              nativeBuildInputs =
                [ (pkgs.python312.withPackages (ps: [ ps.numpy ps.polars ps.pyarrow ])) ];
            } ''
            mkdir -p tessera
            cp -r ${./tessera/crates/tessera-py/python/tessera}/. tessera/
            cp ${tessera-py-lib}/lib/_native.so tessera/_native.so
            export PYTHONPATH=$PWD
            python3 ${./tessera/crates/tessera-py/tests/smoke.py} ${./tessera/corpus/files}
            # Docstring-vs-behaviour drift gate (#412): probes every dtype code against the live
            # module and asserts the accepted sets exactly match what the docstrings advertise.
            python3 ${./tessera/crates/tessera-py/tests/api_drift.py}
            # Array-codec round-trip + pinned default content_hash (#533).
            python3 ${./tessera/crates/tessera-py/tests/codec_roundtrip.py}
            # The WRITE path as documentation (#389): the book `{{#include}}`s anchored regions of this
            # script, so running it here means a snippet that stopped working cannot reach the docs.
            python3 ${./tessera/crates/tessera-py/tests/write_example.py}
            touch $out
          '';

          # The RELEASE wheel (packages.wheel) must be pip-installable AND functional — not just the
          # raw `_native.so` (that's tessera-py-import above). Installs the actual `.whl` into a target
          # dir, then runs the same smoke test through it. Proves the assembled wheel's layout + RECORD
          # + metadata are valid and importable. `LD_LIBRARY_PATH` supplies libstdc++ (the nix-built
          # extension needs it — the same reason the wheel isn't manylinux-portable until auditwheel'd).
          tessera-wheel-import = pkgs.runCommand "tessera-wheel-import"
            {
              nativeBuildInputs =
                [ (pkgs.python312.withPackages (ps: [ ps.numpy ps.polars ps.pyarrow ps.pip ])) ];
            } ''
            export HOME=$TMPDIR
            python3 -m pip install --no-index --no-deps --target=$TMPDIR/site \
              ${tessera-wheel}/${tessera-wheel.wheelName}
            export PYTHONPATH=$TMPDIR/site
            export LD_LIBRARY_PATH=${pkgs.stdenv.cc.cc.lib}/lib
            python3 ${./tessera/crates/tessera-py/tests/smoke.py} ${./tessera/corpus/files}
            # Same drift gate as tessera-py-import, proven through the installed wheel.
            python3 ${./tessera/crates/tessera-py/tests/api_drift.py}
            # Array-codec round-trip + pinned default content_hash (#533).
            python3 ${./tessera/crates/tessera-py/tests/codec_roundtrip.py}
            # …and the documented write path, so the book's example works through the real wheel too.
            python3 ${./tessera/crates/tessera-py/tests/write_example.py}
            touch $out
          '';

          # The mdBook how-to must build (docs-as-tests layer 3). Its CLI transcripts are `{{#include}}`d
          # from the trycmd files the test suite runs, so the published docs can't drift from behaviour.
          mdbook-build = pkgs.runCommand "mdbook-build"
            { nativeBuildInputs = [ pkgs.mdbook ]; } ''
            mdbook build ${./.}/docs/book -d $out
          '';

          # The dev shell itself must build (toolbelt + toolchain resolve).
          dev-shell = self.devShells.${system}.default;
        };
          names = map (c: c.name) (builtins.attrValues allChecks);
          dupes = pkgs.lib.unique (builtins.filter (n: pkgs.lib.count (m: m == n) names > 1) names);
        in
          if dupes == [ ] then allChecks
          else throw ''
            flake checks share derivation names: ${builtins.concatStringsSep ", " dupes}
            `nix flake check -L` labels log lines by derivation name, so these checks would be
            indistinguishable in CI logs. Give each a distinct `pname`/`pnameSuffix` (#535).
          '';

        formatter = pkgs.nixpkgs-fmt;
      });
}
