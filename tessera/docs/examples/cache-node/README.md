# Compute-node caching — Tier B (zot pull-through) example

A standalone, deployable example of the **registry-grade** cache tier from
[ADR-0039](../../../../docs/adr/0039-compute-node-caching.md): a single compute node fronts a slow upstream
MinIO-backed OCI registry with a local [`zot`](https://zotregistry.dev) pull-through cache on NVMe. First
`tessera pull` hits the upstream once; subsequent pulls (and every peer node's pulls) are LAN-speed.

## What's in this directory

| File | What it is |
|---|---|
| `flake.nix` | Standalone Nix flake. Exposes `nixosModules.tessera-cache` + `apps.cache-node` + `packages.exampleConfig`. NOT a flake-input of the repo root — it can't break the repo gate. |
| `zot-config.json` | Sample rendered config (reviewable without Nix). The flake's `checks.config-drift` keeps this file honest — it fails if the module renders something different. |
| `README.md` | This file. |

Run `nix flake check` from this directory to verify the flake (passes today; module evaluates,
sample config renders, drift check is green).

## The two access paths a compute node has

These coexist by design — different shapes for different workloads, picked per-query:

### Path A — direct S3 range-read (cohort-scale; one-shot)

Open the `.tsra` *directly* from the upstream object store. Tessera's cloud reader
([`tessera_io::open_url`](../../../crates/tessera-io/src/cloud.rs), #225) issues one HEAD + one 64 KiB
tail-GET, then prune-before-fetch over `select_blocks_overlapping` ensures only the blocks the query
actually overlaps are downloaded. No cache is in the path; the canonical store is the source.

Right shape for: one-off cohort scans, exploratory queries, anything that touches *many* products and a
*small slice* of each.

### Path B — OCI pull through the local zot cache (repeated reads)

`tessera pull localhost:5000/lab/recon:v1` hits the local zot. On the *first* pull, zot fetches the
artifact (the whole `.tsra` + the signature sidecar layer) from the upstream registry and caches it on
NVMe (`storage.rootDirectory`). Every subsequent pull, from *any* compute node pointed at this cache, is
LAN-speed. zot's `extensions.sync.onDemand: true` is what makes this transparent; the
`storage.retention.policies[].keepTags[].pulledWithin` window is what evicts cold artifacts on
access-recency (see ADR-0039 §2 for why this beats MinIO ILM).

Right shape for: repeated reads of the same product (an analysis re-running over a fixed dataset all
afternoon), shared datasets across N compute nodes, anything where "I already pulled this once today" is
the common case.

## Quick start — single host, no NixOS

The flake's `apps.cache-node` is a `nix run`-able wrapper that works on any Linux with Nix installed.
zot is **not** in nixpkgs today, so the wrapper requires `zot` on `$PATH`; the simplest install:

```bash
# Install zot once (Linux amd64; see https://zotregistry.dev/ for other platforms).
curl -L -o /tmp/zot \
  https://github.com/project-zot/zot/releases/latest/download/zot-linux-amd64
chmod +x /tmp/zot
export PATH=/tmp:$PATH

# Render the example config + boot zot against it (Ctrl-C to stop).
# The cache dir lands under $XDG_RUNTIME_DIR/tessera-cache-$$ for safety;
# edit zot-config.json to point at your real upstream + a persistent NVMe path.
nix run .#cache-node
```

`zot` listens on `127.0.0.1:5000` by default. From a second terminal:

```bash
# Push to the upstream registry (one-time, from the producer side).
tessera push file.tsra registry.example.com/lab/recon:v1

# Pull through the local zot. First pull hits upstream; subsequent pulls are NVMe-speed.
tessera pull oci://localhost:5000/lab/recon:v1 ./recon.tsra
tessera verify ./recon.tsra
```

## Production deploy — NixOS module

```nix
{ ... }: {
  imports = [ ./flake.nix ];  # or use it as a flake input
  services.tessera-cache = {
    enable = true;
    upstream.url = "https://registry.example.com";
    upstream.credentialsFile = "/run/credentials/tessera-cache/upstream.json";
    cacheDir = "/mnt/nvme/tessera-cache";
    decay = "720h";   # 30 days access-recency window
    listenAddress = "0.0.0.0";
    listenPort = 5000;
    openFirewall = true;

    # `zot` is not in nixpkgs today; supply a derivation that puts the binary at
    # `${zotPackage}/bin/zot`. Two common shapes:
    #   (a) wrap an operator-installed binary:
    zotPackage = pkgs.runCommand "zot" { } ''
      mkdir -p $out/bin
      ln -s /usr/local/bin/zot $out/bin/zot
    '';
    #   (b) build zot with buildGoModule (omitted; see the upstream repo for the
    #       vendored-deps hash you'll need).
  };
}
```

The unit hardens zot under `DynamicUser` + `ProtectSystem=strict`, with `ReadWritePaths=[cacheDir]`.

## Staged workflow — the path the field actually walks

Lifted from ADR-0039 §6 for at-a-glance reference:

1. **Today — no MinIO, one workstation.** Tier *A* (a Tessera versioning repo as the cache).
   `tessera init ~/.cache/tessera/repo` + `tessera import` the files you have. The repo IS the cache;
   an `atime`-LRU + `du`-ceiling sweep (cron / systemd timer) caps it.
2. **MinIO comes online.** Add `tessera push` on the producer side, pointing at a registry that fronts
   MinIO. Consumer behavior unchanged — they still open local paths from their repo.
3. **Second compute node joins.** Stand up *this example* on a node with shared NVMe. Both compute
   nodes `tessera pull localhost:5000/lab/x:v1`; first pull on each populates the local zot,
   thereafter LAN-speed.
4. **Cohort-wide queries.** Use Path A (`tessera.open("s3://…")`) directly against the canonical
   store. The cache tier is irrelevant — prune-before-fetch on the range-read is the right
   optimisation for many-products / small-slice queries.

## WORM interaction

This is a **cache**, not a WORM store. The upstream MinIO is the WORM tier (ADR-0033:
raw=Compliance-WORM / derived=Governance-WORM). The compute node only ever has cache copies, which are
free to evict on access-recency; the canonical retention story lives on the storage server.

## Python notebook snippet — both access paths

```python
# pip install tessera-py polars
import os
import subprocess
import tessera

# ----------------------------------------------------------------------
# Path A — direct S3 range-read against the canonical MinIO store.
# Use for cohort-scale / one-shot reads; only the blocks the query
# overlaps are fetched (prune-before-fetch). No cache is in the path.
# ----------------------------------------------------------------------
# Configure once; the Tessera cloud reader picks these up from the env.
os.environ.setdefault("AWS_REGION", "us-east-1")
os.environ.setdefault("AWS_ACCESS_KEY_ID", "minio-key")
os.environ.setdefault("AWS_SECRET_ACCESS_KEY", "minio-secret")
os.environ.setdefault("TESSERA_S3_ENDPOINT", "http://minio.lab:9000")

# (Today: the Python `tessera.open()` takes a local path. The cloud-URL
# entry point lives in the CLI / Rust API; the recommended ergonomic for
# notebooks is to shell out to `tessera get` — see Path B's wrapper.)
subprocess.run(
    ["tessera", "inspect", "s3://lab-products/recon/2025-04-12.tsra"],
    check=True,  # range-read inspect — pulls the manifest + small ranges
)

# ----------------------------------------------------------------------
# Path B — OCI pull through the local zot cache.
# Use for repeated reads of the same artifact. First pull hits upstream
# (zot caches it on NVMe); subsequent pulls are LAN-speed.
# ----------------------------------------------------------------------
LOCAL_CACHE = os.path.expanduser("~/.cache/tessera/repo")
os.makedirs(LOCAL_CACHE, exist_ok=True)
subprocess.run(["tessera", "init", LOCAL_CACHE], check=False)  # idempotent

def tessera_get(ref: str, out: str) -> str:
    """Resolve an OCI ref through the local zot, sha256-verified by `tessera pull`.

    This is the ergonomic the ADR-0039 §3 `tessera get` verb will eventually own
    natively (cache-aware fetch + de-dup-into-repo). Today: shell out to `tessera pull`.
    """
    subprocess.run(
        ["tessera", "pull", "--plain-http", ref, out],
        check=True,
    )
    # Once shipped, the natively-cache-aware form will be roughly:
    #   subprocess.run(["tessera", "get", "--cache", LOCAL_CACHE, ref, "--out", out], check=True)
    return out

local = tessera_get(
    "oci://localhost:5000/lab/recon:v1",       # → through the local zot
    out="/tmp/recon.tsra",
)

# Open the local file with the Python bindings (numpy / polars / arrow / dict).
reader = tessera.open(local)
print(reader.id, reader.block_names())
df = reader.table("events")       # → polars.DataFrame
img = reader.array("recon")       # → numpy.ndarray
```

## Where the moving parts live in the Tessera tree

| Concern | Code |
|---|---|
| OCI artifact manifest (push/pull layer shape) | `tessera/crates/tessera-io/src/oci.rs` |
| In-Rust OCI distribution client (`push`/`pull`) | `tessera/crates/tessera-io/src/registry.rs` |
| Versioning repo (`objects/<digest>` + `refs/` + `log/` + `gc`) | `tessera/crates/tessera-io/src/repo.rs` |
| Cloud range-read (`open_url`, tail-prefetch, `select_blocks_overlapping`) | `tessera/crates/tessera-io/src/cloud.rs` |
| `init` / `import` / `commit` / `gc` CLI verbs | `tessera/crates/tessera-cli/src/version.rs` |
| ADRs this depends on | [0033 (WORM/distribution)](../../../../docs/adr/0033-collections-and-derivation.md) · [0036 (versioning repo)](../../../../docs/adr/0036-versioning-audit-object-store.md) · [0037 (signing — sidecar carried as 2nd OCI layer)](../../../../docs/adr/0037-signing-trust-model.md) · [0039 (this design)](../../../../docs/adr/0039-compute-node-caching.md) |
