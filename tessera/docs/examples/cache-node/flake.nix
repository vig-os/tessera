{
  # Tessera compute-node caching example — Tier B (zot pull-through cache).
  #
  # See ADR-0039 (docs/adr/0039-compute-node-caching.md) for the full design. This is a
  # **standalone** flake (NOT a flake-input of the repo root) so it can be evaluated and
  # tested in isolation without any risk to the repo's hermetic gate.
  #
  # What this exposes:
  #   nixosModules.tessera-cache  — declarative `services.tessera-cache = { … }` for NixOS
  #                                  / nix-darwin hosts (renders zot-config.json + a
  #                                  systemd service unit).
  #   apps.cache-node             — `nix run .#cache-node` renders the same config to a
  #                                  tempdir and execs `zot serve`. Works on any Linux
  #                                  with Nix installed; no NixOS required.
  #   packages.exampleConfig      — the rendered `zot-config.json` for the default
  #                                  settings; matches `zot-config.json` committed in
  #                                  this dir for review-without-nix.
  #
  # Quick checks (run from this directory):
  #   nix flake check                  # evaluates the module + builds exampleConfig
  #   nix build .#exampleConfig        # write the rendered config to ./result
  #   nix run .#cache-node             # render config + exec `zot` from $PATH
  #
  # Note on the `zot` binary: `zot` is **not** in nixpkgs today (only `zotero` etc.). We
  # therefore depend on the operator providing the `zot` binary via $PATH (the README
  # documents the one-liner install). The systemd module accepts a `zotPackage` override
  # for sites that *do* package zot in their overlay.
  #
  # Why a separate flake (not the repo root): caching is *deployment* infrastructure,
  # orthogonal to the format. Bundling it into the repo flake would bloat the Cargo gate
  # (we'd pull a whole Go toolchain) or split the gate's truth. This way, the cache-node
  # lives next to the format docs without touching them.

  description = "Tessera compute-node cache — zot pull-through (Tier B of ADR-0039)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    let
      # ---- The shared library: render a zot config from a tessera-cache option set ----
      #
      # Pulled out of the NixOS module so the `apps.cache-node` runner (non-NixOS) and the
      # `packages.exampleConfig` (review fixture) share one source of truth.
      mkConfig = lib: cfg:
        let
          # zot allows multiple sync registries; we expose the simple "one upstream" shape
          # because that's what a single compute node points at. Operators with two
          # upstreams (a public mirror + a private MinIO) edit the rendered file.
          syncRegistry = {
            urls = [ cfg.upstream.url ];
            onDemand = true;
            # `tlsVerify = false` is for plain-http dev MinIO registries; production
            # leaves it true (the default).
            tlsVerify = cfg.upstream.tlsVerify;
          } // (lib.optionalAttrs (cfg.upstream.credentialsFile != null) {
            credentialsFile = cfg.upstream.credentialsFile;
          }) // (lib.optionalAttrs (cfg.upstream.content != [ ]) {
            content = cfg.upstream.content;
          });
        in
        {
          distSpecVersion = "1.1.0";
          storage = {
            rootDirectory = cfg.cacheDir;
            # Dedup blobs across artifacts on disk — saves space when two products
            # share an OCI blob (e.g. the empty config). Tessera content-addresses
            # blocks inside the .tsra too; this is the registry-side dedup.
            dedupe = true;
            # `gc` enables zot's GC of orphaned blobs (separate from `retention`).
            gc = true;
            retention = {
              # The whole point of Tier B vs MinIO ILM: `pulledWithin` is
              # access-recency, not creation-age. A tag last *pulled* within the
              # window is kept; otherwise the sweep can evict it. See ADR-0039 §2.
              policies = [
                {
                  repositories = [ "**" ];
                  keepTags = [
                    {
                      patterns = [ ".*" ];
                      pulledWithin = cfg.decay;
                    }
                  ];
                }
              ];
            };
          };
          http = {
            address = cfg.listenAddress;
            port = builtins.toString cfg.listenPort;
          };
          log = {
            level = cfg.logLevel;
          };
          extensions = {
            sync = {
              enable = true;
              registries = [ syncRegistry ];
            };
            search.enable = true;
          };
        };

      # ---- The NixOS module ----
      tesseraCacheModule = { config, lib, pkgs, ... }:
        let
          cfg = config.services.tessera-cache;
          rendered = pkgs.writeText "zot-config.json"
            (builtins.toJSON (mkConfig lib cfg));
        in
        {
          options.services.tessera-cache = {
            enable = lib.mkEnableOption "Tessera zot pull-through cache (Tier B, ADR-0039)";

            zotPackage = lib.mkOption {
              # `zot` is not in nixpkgs today; the operator either provides an overlay
              # package or (the common path) installs the static binary out-of-band and
              # points this at `pkgs.runtimeShell`-friendly `/usr/local/bin/zot`. Either
              # way, the module reads `${zotPackage}/bin/zot` so the user has one place
              # to set it. The README shows both shapes.
              type = lib.types.package;
              example = lib.literalExpression ''
                pkgs.runCommand "zot" { } '''
                  mkdir -p $out/bin
                  ln -s /usr/local/bin/zot $out/bin/zot
                '''
              '';
              description = ''
                Package providing the `zot` binary at `''${zotPackage}/bin/zot`. `zot`
                is not in nixpkgs today — provide your own derivation, or wrap the
                operator-installed binary (see the example).
              '';
            };

            upstream = {
              url = lib.mkOption {
                type = lib.types.str;
                example = "https://registry.example.com";
                description = ''
                  URL of the upstream OCI registry that fronts the canonical MinIO
                  store. The cache fetches artifacts from this registry on demand
                  (first pull) and serves subsequent pulls from `cacheDir`.
                '';
              };
              tlsVerify = lib.mkOption {
                type = lib.types.bool;
                default = true;
                description = "Verify the upstream registry's TLS cert (set to false only for dev MinIO).";
              };
              credentialsFile = lib.mkOption {
                type = lib.types.nullOr lib.types.path;
                default = null;
                description = ''
                  Optional path to a JSON file with upstream registry credentials,
                  shape:
                    { "<host>": { "username": "...", "password": "..." } }
                  Loaded by zot at startup (kept out of the world-readable nix
                  store). Recommend a systemd `LoadCredential` or sops-nix.
                '';
              };
              content = lib.mkOption {
                type = lib.types.listOf lib.types.attrs;
                default = [ ];
                example = lib.literalExpression ''
                  [ { prefix = "lab/**"; tags = { regex = ".*"; semver = false; }; } ]
                '';
                description = ''
                  Optional content filters limiting which upstream repos this
                  cache will fetch (empty = no filter, fetch anything pulled).
                '';
              };
            };

            cacheDir = lib.mkOption {
              type = lib.types.path;
              default = "/var/lib/tessera-cache";
              description = ''
                Filesystem root where zot stores cached OCI blobs + manifests.
                Should live on local NVMe — this is the hot path for compute pulls.
              '';
            };

            decay = lib.mkOption {
              type = lib.types.str;
              default = "720h"; # 30 days
              example = "168h";
              description = ''
                Access-recency retention window. A tag last *pulled* within this
                window is kept; otherwise zot's retention sweep may evict it.
                Format is Go's `time.Duration` (e.g. "720h" = 30d). This is the
                predicate MinIO ILM lacks — see ADR-0039 §2.
              '';
            };

            listenAddress = lib.mkOption {
              type = lib.types.str;
              default = "127.0.0.1";
              description = "Bind address. Default is loopback; set to a LAN IP to share with peer nodes.";
            };

            listenPort = lib.mkOption {
              type = lib.types.port;
              default = 5000;
              description = "Listen port. Tessera CLI: `tessera pull localhost:5000/repo:tag`.";
            };

            logLevel = lib.mkOption {
              type = lib.types.enum [ "debug" "info" "warn" "error" ];
              default = "info";
              description = "zot log level.";
            };

            openFirewall = lib.mkOption {
              type = lib.types.bool;
              default = false;
              description = "Open `listenPort` in the firewall. Off by default (loopback-only is the safe shape).";
            };
          };

          config = lib.mkIf cfg.enable {
            systemd.services.zot = {
              description = "zot OCI registry (Tessera pull-through cache, Tier B)";
              wantedBy = [ "multi-user.target" ];
              after = [ "network-online.target" ];
              wants = [ "network-online.target" ];
              serviceConfig = {
                ExecStart = "${cfg.zotPackage}/bin/zot serve ${rendered}";
                Restart = "on-failure";
                RestartSec = "5s";
                # Hardening: zot only needs to read its config + read/write the cache dir.
                DynamicUser = true;
                StateDirectory = "tessera-cache";
                # If `cacheDir` was overridden, make sure the dynamic user can write it.
                ReadWritePaths = [ cfg.cacheDir ];
                # Standard hardening knobs.
                ProtectSystem = "strict";
                ProtectHome = true;
                PrivateTmp = true;
                NoNewPrivileges = true;
                ProtectKernelTunables = true;
                ProtectKernelModules = true;
                ProtectControlGroups = true;
                RestrictAddressFamilies = [ "AF_INET" "AF_INET6" "AF_UNIX" ];
                LockPersonality = true;
              };
            };

            networking.firewall.allowedTCPPorts =
              lib.mkIf cfg.openFirewall [ cfg.listenPort ];
          };
        };
    in
    {
      nixosModules.tessera-cache = tesseraCacheModule;
      nixosModules.default = tesseraCacheModule;
    }
    //
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };
        lib = pkgs.lib;

        # The defaults the example renders against. Override at call time for a real deploy.
        exampleCfg = {
          upstream = {
            url = "https://registry.example.com";
            tlsVerify = true;
            credentialsFile = null;
            content = [ ];
          };
          cacheDir = "/var/lib/tessera-cache";
          decay = "720h";
          listenAddress = "127.0.0.1";
          listenPort = 5000;
          logLevel = "info";
        };

        renderedConfig = pkgs.writeText "zot-config.json"
          (builtins.toJSON (mkConfig lib exampleCfg));

        # `nix run .#cache-node` — render the config to a tempdir, exec the operator's
        # `zot` binary from $PATH. We use $PATH (not a nix-store path) because zot
        # isn't in nixpkgs today; the README shows the install one-liner.
        cacheNodeApp = pkgs.writeShellApplication {
          name = "cache-node";
          runtimeInputs = [ pkgs.coreutils pkgs.jq ];
          text = ''
            set -euo pipefail
            if ! command -v zot >/dev/null 2>&1; then
              echo "tessera-cache: \`zot\` not on \$PATH." >&2
              echo "  install it from https://zotregistry.dev/ — for a quick test on Linux:" >&2
              echo "    curl -L -o /tmp/zot https://github.com/project-zot/zot/releases/latest/download/zot-linux-amd64" >&2
              echo "    chmod +x /tmp/zot && export PATH=/tmp:\$PATH" >&2
              exit 127
            fi
            workdir="''${XDG_RUNTIME_DIR:-/tmp}/tessera-cache-$$"
            mkdir -p "$workdir/cache"
            cfg="$workdir/zot-config.json"
            # Render the example config but point cacheDir at the per-run tempdir, so a
            # `nix run` smoke-test never collides with /var/lib/tessera-cache.
            jq --arg root "$workdir/cache" \
               '.storage.rootDirectory = $root' \
               ${renderedConfig} > "$cfg"
            echo "tessera-cache: rendered config → $cfg" >&2
            echo "tessera-cache: cache dir       → $workdir/cache" >&2
            echo "tessera-cache: upstream        → ${exampleCfg.upstream.url}" >&2
            echo "tessera-cache: edit \$cfg before re-running for a real upstream" >&2
            exec zot serve "$cfg"
          '';
        };

        # A smoke `nix flake check`: builds the rendered config + asserts the example
        # JSON shape is what we ship in this directory (so `zot-config.json` doesn't
        # drift from the module).
        configMatchesCommitted = pkgs.runCommand "config-matches-committed"
          { nativeBuildInputs = [ pkgs.jq pkgs.diffutils ]; }
          ''
            want=${./zot-config.json}
            got=${renderedConfig}
            # Compare as canonicalised JSON so whitespace doesn't matter.
            if ! diff -u <(jq -S . "$want") <(jq -S . "$got"); then
              echo "ERROR: committed zot-config.json drifted from the example flake's render"
              echo "       regenerate with:  nix build .#exampleConfig && cp result zot-config.json"
              exit 1
            fi
            touch "$out"
          '';

        # Eval-only assertion: the NixOS module's option set is well-typed (so a
        # `nix flake check` catches option drift before someone tries to deploy). We
        # stop short of a full NixOS eval — the `systemd.services` / `networking.firewall`
        # options come from the NixOS module set, which would balloon this example into a
        # full system import. The cheap signal is "options.services.tessera-cache exists
        # and has the documented sub-options"; the deploy-time NixOS eval covers the rest.
        moduleEvalSmoke = pkgs.runCommand "module-eval-smoke" { } ''
          : ${builtins.toString (
              let
                # Stub the NixOS option paths the module writes into, so we can
                # `lib.evalModules` without dragging in the whole NixOS module closure.
                # The module's option-set + the rendered systemd ExecStart still
                # type-check; we just don't run the full systemd module.
                stubNixosOptions = { lib, ... }: {
                  options.systemd.services = lib.mkOption {
                    type = lib.types.attrsOf lib.types.attrs;
                    default = { };
                  };
                  options.networking.firewall.allowedTCPPorts = lib.mkOption {
                    type = lib.types.listOf lib.types.port;
                    default = [ ];
                  };
                };
                evald = lib.evalModules {
                  modules = [
                    tesseraCacheModule
                    stubNixosOptions
                    { _module.args = { inherit pkgs; }; }
                    {
                      services.tessera-cache = {
                        enable = true;
                        upstream.url = "https://registry.example.com";
                        zotPackage = pkgs.writeShellScriptBin "zot" "exit 0";
                      };
                    }
                  ];
                };
                cfg = evald.config.services.tessera-cache;
                exec = evald.config.systemd.services.zot.serviceConfig.ExecStart;
                # Touch every documented option to force its type-check, plus the rendered
                # systemd ExecStart line.
                touched = builtins.concatStringsSep "|" [
                  (toString cfg.enable)
                  cfg.upstream.url
                  (toString cfg.upstream.tlsVerify)
                  cfg.cacheDir
                  cfg.decay
                  cfg.listenAddress
                  (toString cfg.listenPort)
                  cfg.logLevel
                  (toString cfg.openFirewall)
                  exec
                ];
              in
                builtins.stringLength touched
            )}
          touch "$out"
        '';
      in
      {
        packages.exampleConfig = renderedConfig;
        packages.default = renderedConfig;

        apps.cache-node = {
          type = "app";
          program = "${cacheNodeApp}/bin/cache-node";
        };
        apps.default = self.apps.${system}.cache-node;

        # `nix flake check` evaluates these — keeps the committed sample honest + verifies
        # the module evaluates under a synthesised NixOS config.
        checks.config-drift = configMatchesCommitted;
        checks.exampleConfig = renderedConfig;
        checks.module-eval = moduleEvalSmoke;
      });
}
