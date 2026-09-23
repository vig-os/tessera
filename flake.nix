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
            || (pkgs.lib.hasInfix "/tests/feature-snapshots/" path);
          name = "source";
        };
        # Every feature declared anywhere in the workspace **except `static-hdf5`** (ADR-0057 §4), in
        # cargo's `package/feature` form so one invocation from the virtual-manifest root covers all
        # members. This is what the PR clippy gate builds instead of `--all-features`.
        #   tessera-core   array-zarr, table-arrow   (= `full`)
        #   tessera-io     cloud
        #   tessera-cli    cloud (→ tessera-io/cloud), sql
        #   tessera-ingest static-hdf5 only          → deliberately absent
        #   tessera-py / tessera-wasm                → no features
        workspaceFeatures = builtins.concatStringsSep "," [
          "tessera-core/full"
          "tessera-io/cloud"
          "tessera-cli/cloud"
          "tessera-cli/sql"
        ];

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
        checks = {
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
            minio = pkgs.minio.overrideAttrs (old: {
              meta = (old.meta or { }) // { knownVulnerabilities = [ ]; };
            });
          in
          craneLib.cargoNextest (commonArgs // {
            inherit cargoArtifacts;
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

        formatter = pkgs.nixpkgs-fmt;
      });
}
