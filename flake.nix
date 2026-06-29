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
        # test validates it) — cleanCargoSource would otherwise strip these non-Rust files so the
        # hermetic build/test can't find them.
        src = pkgs.lib.cleanSourceWith {
          src = ./tessera;
          filter = path: type:
            (craneLib.filterCargoSources path type)
            || (pkgs.lib.hasInfix "/corpus/" path)
            || (pkgs.lib.hasInfix "/docs/examples/" path);
          name = "source";
        };
        commonArgs = {
          inherit src;
          strictDeps = true;
          pname = "tessera";
          version = "0.0.0";
          # The Vortex table backend pulls a transitive dep (`custom-labels`) whose build script
          # runs bindgen (needs libclang) and links libstdc++. The tessera-ingest GE-HDF5 reader
          # links libhdf5 (found via pkg-config — `hdf5-metno-sys` reads PKG_CONFIG_PATH when
          # HDF5_DIR is unset). Provide all to every crane derivation (deps/clippy/test).
          nativeBuildInputs = with pkgs; [ clang pkg-config ];
          buildInputs = with pkgs; [ stdenv.cc.cc.lib hdf5 ];
          LIBCLANG_PATH = "${pkgs.libclang.lib}/lib";
        };
        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        # The Python extension module (#210). crane doesn't install cdylibs by default, so copy the
        # built `libtessera.so` → `tessera.so` (the abi3 import name) into $out/lib. pyo3's abi3
        # feature builds without a runtime interpreter, so no python is needed at compile time.
        tessera-py-lib = craneLib.buildPackage (commonArgs // {
          inherit cargoArtifacts;
          pname = "tessera-py";
          cargoExtraArgs = "-p tessera-py";
          doCheck = false;
          postInstall = ''
            mkdir -p $out/lib
            cp target/release/libtessera.so $out/lib/tessera.so
          '';
        });
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
          # nextest does NOT run doctests, so gate them separately — the "docs-as-tests" layer: every
          # `///` example compiles + runs, so public-API docs can never drift from behaviour.
          workspace-doctest = craneLib.cargoTest (commonArgs // {
            inherit cargoArtifacts;
            cargoTestExtraArgs = "--doc";
          });
          workspace-fmt = craneLib.cargoFmt { inherit src; };

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
              for i in $(seq 1 80); do
                ${pkgs.curl}/bin/curl -sf http://127.0.0.1:9101/minio/health/ready && break
                sleep 0.25
              done
              ${pkgs.awscli2}/bin/aws --endpoint-url http://127.0.0.1:9101 \
                s3 mb s3://tessera-test
            '';
            nativeBuildInputs = commonArgs.nativeBuildInputs
              ++ [ minio pkgs.awscli2 pkgs.curl pkgs.cacert ];
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

          # The Python bindings import + verify the committed corpus through the real .so (#210):
          # proves the abi3 extension loads on CPython and the read/verify surface works end-to-end.
          tessera-py-import = pkgs.runCommand "tessera-py-import"
            { nativeBuildInputs = [ (pkgs.python312.withPackages (ps: [ ps.numpy ])) ]; } ''
            cp ${tessera-py-lib}/lib/tessera.so ./tessera.so
            export PYTHONPATH=$PWD
            python3 ${./tessera/crates/tessera-py/tests/smoke.py} ${./tessera/corpus/files}
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
