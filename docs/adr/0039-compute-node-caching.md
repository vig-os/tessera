# ADR-0039 — Compute-node caching architecture

Status: **Proposed** (2026-06-30, spike for #236). Builds on ADR-0033 (raw=Compliance / derived=Governance
WORM roles + flat content-addressed products), ADR-0036 (versioning + `objects/<digest>` store), the OCI
artifact transport (`tessera-io::oci` / `registry.rs`, #209), and the cloud range-read reader (`cloud.rs`,
#225). Companion to the standalone example flake under `tessera/docs/examples/cache-node/`.

## Context — one canonical store, many compute hands

Tessera's storage shape is *one S3/MinIO bucket is the truth* (ADR-0033's flat content-addressed product
prefix), distributed-as-OCI for naming/auth/replication (#209), and *every sealed `.tsra` opens directly
over a range-GET* (#225 — `open_url("s3://…")` with a 64 KiB tail-prefetch + `select_blocks_overlapping`
prune-before-fetch). That works because identity is content-addressed and every read verifies the seal +
the block digest end-to-end.

Compute does NOT have the same shape. A real PET/CT site looks like:

- one institutional **MinIO** behind a slow link (or, today, **no MinIO yet** — products live on a NAS or a
  laptop's SSD, with MinIO arriving "later this quarter");
- a small handful of **compute nodes** (workstations, an HPC head node, the recon box) on a fast local
  LAN with NVMe; the same dataset is opened by N notebooks per week, mostly from one researcher's hand;
- a hard **archival anchor** — once a derived product is sealed and signed, the bytes are stable forever
  (content-addressed; ADR-0036). Caching is *trivially correct* because cache keys are digests.

The bug to avoid: **inventing a Tessera cache daemon, baking it into the format, or shipping it as a
`tessera` subcommand.** Caching is *infrastructure* (where the bytes live for fast re-reads) and
*orthogonal* to the format. Tessera ships verbs that produce + consume content-addressed objects; how
those objects get to a compute node's NVMe is a deployment question.

The follow-on bug to avoid: **conflating "MinIO ILM by creation-age" with "cache eviction by access
recency."** Object stores (MinIO/S3 ILM, AWS lifecycle) decay by *upload time*, which is the wrong axis
for a cache — a 6-month-old dataset queried twice this week must stay hot, and a yesterday-uploaded but
never-touched product is the right thing to evict.

## Decision

### §0 — Caching is infrastructure, not a `tessera` subcommand

The Tessera CLI exposes the **producer/consumer** verbs (`init`/`import`/`commit`/`gc`/`push`/`pull`/
`inspect`/`verify`/`extract`/`read`). It does **not** ship `tessera cache start`, a daemon, a
"cache-aware open," or a config file that names a cache. A cache is a deployment artifact — a flake, a
systemd unit, a directory.

This keeps three properties:

1. **Verifiability is unchanged.** Every byte that reaches the reader is still digest-verified at open
   (`Reader::from_reader` re-checks magic + seal; `ObjectStoreReader` is just a `Read+Seek` adapter). A
   cache that lies → integrity error, not silent corruption.
2. **The format never depends on a cache being present.** A reader with no cache works exactly the same
   as one with a cache; the cache is a path-rewriting / range-serving optimisation only.
3. **The cache layer is *swappable* by deployment.** Single node → a versioning repo. Multi-node → a
   pull-through registry. Operator picks the tier; the Tessera client and the producer flow don't change.

The **one** new in-scope verb is `tessera get <ref> → path` (§3) — a thin, cache-agnostic *fetch*: take a
ref (`oci://…`, `s3://…`, `http(s)://…`, or a local path) and return a verified local `.tsra` path the
caller can open. It owns no cache; it consults `$TESSERA_CACHE` (a repository directory) when set and
otherwise writes to a tempfile. This is the *one* place where "do I have this already?" lives in the CLI,
and it lives there because that question is unavoidable for any non-pasta script — it has nothing to do
with the format and everything to do with making the Python / shell ergonomics survive a real workflow.

### §1 — Two tiers, picked by the deployment, not by the format

The two real shapes the field actually has. Both are infra; both leave the Tessera client unchanged.

#### Tier A — **Lightweight: a Tessera versioning repo as the cache.** Single node, zero infra.

The `objects/<digest>` store (ADR-0036) already *is* a content-addressed local cache. It de-dups across
projects for free (the same block-digest path is shared between every product that references it), it has
a built-in sweep (`tessera gc`), and it is the same code path a producer uses to commit versions — no
new component, no daemon, no cache protocol.

The shape:

```text
$TESSERA_CACHE = ~/.cache/tessera/repo/
  objects/blake3/<aa>/<rest>     # de-duped blocks + manifests (the cache lines)
  refs/<lineage>                 # which version is current for each id
  log/<lineage>.jsonl            # version history cache
```

Usage from a notebook (no daemon, no cache verb in the format):

- `tessera init ~/.cache/tessera/repo` (once)
- `tessera get s3://bucket/path/recon-2025-04-12.tsra` → local path (writes the `.tsra` into the cache
  repo's `objects/`, returns the materialised path; if absent, fetches; if present, returns immediately)
- Python opens the local path the normal way (`tessera.open(path)`).

Decay: an **atime-LRU sweep** (a small companion script + a systemd timer in the example flake) bounds
the cache directory in bytes. The sweep is access-recency, not creation-age: walks `objects/`, sorts by
`atime`, deletes oldest until `du -s` is under the cap. `tessera gc` is *separate*: it reclaims
objects unreachable from any `ref` (i.e. a `forget`-ed lineage's leaves). Both run from the same timer.

Why this is the default tier:

- **Zero infra.** No daemon, no registry, no extra port. Cargo-install + a cron line.
- **De-dup is free.** Two products that share a block share the cache line; this is the point of
  content-addressed storage and it's already implemented.
- **`tessera gc` and the cache sweep don't fight.** GC operates on the version-DAG (reachable from
  refs); the LRU sweep operates on `atime`. Because both *only* delete from `objects/` (never `refs/` or
  `log/`), running both is safe: GC reclaims what the operator explicitly forgot; the sweep reclaims
  whatever hasn't been touched in N weeks regardless of reachability — a hard byte ceiling for the
  weeks-to-month decay window the workstation actually needs.
- **It works *today*** — no MinIO required to start using Tessera; the cache is the local repository the
  user already has.

#### Tier B — **Registry-grade: a `zot` pull-through cache on the compute host.** Multi-node, OCI-native.

When products live in a registry-fronted MinIO and multiple compute nodes pull the same artifacts, the
ergonomic substrate is **`zot` configured as a pull-through cache** of the upstream registry.
[`zot`](https://zotregistry.dev/) is a CNCF OCI registry written in Go; `extensions.sync` with
`onDemand: true` fetches an artifact (and all its referenced blobs) on first pull and serves subsequent
pulls from the local NVMe `storage.rootDirectory`. Tessera's `tessera push` / `tessera pull` already
speak the OCI distribution API, and a sealed `.tsra` rides as an OCI 1.1 artifact (`oci.rs`), so:

- `tessera push file.tsra reg.example.com/lab/recon:v1` lands the artifact in the upstream registry.
- A compute node's `tessera pull localhost:5000/lab/recon:v1` hits the local zot, which transparently
  fetches it from `reg.example.com` once and caches it on local NVMe — every subsequent compute-node
  pull is LAN-speed.

The eviction shape that matches a cache (the key insight): zot's
`storage.retention.policies[].keepTags[].pulledWithin` is **access-recency**, not creation-age. A tag
that was last *pulled* > N days ago is eligible for eviction; a six-month-old artifact pulled this morning
stays hot. This is the right axis for a cache (and exactly what MinIO ILM doesn't give you — see §2).

Why a registry (zot) and not "a smarter S3 cache":

- Tessera already speaks OCI on push/pull (#209). The compute node sees a registry; it doesn't need to
  know "this is a cache vs the upstream." The Tessera client doesn't change.
- OCI's content-addressing matches Tessera's: the registry de-dups blobs by sha256 across artifacts (the
  empty config blob is referenced by every artifact and stored once), and the artifact manifest carries
  the signature sidecar as a 2nd layer (ADR-0037) — caching preserves the signature for free.
- `zot` is a single Go binary, MIT-licensed, supports `onDemand` sync + retention by `pulledWithin` +
  basic-auth — no extra software wins on simplicity for the same shape.

#### When to pick which

| Situation | Tier |
|---|---|
| One workstation, one researcher, no MinIO yet | **A** (start here) |
| MinIO is live but only one node ever pulls | A (S3 range-read + a local Tessera repo cache) |
| Multiple compute nodes share the same dataset | **B** (each node runs zot pull-through) |
| HPC: one shared NVMe scratch + many job nodes | B (one zot on the scratch host, jobs pull from it) |
| Air-gapped acquisition site | Neither — `tessera seal`/`publish` writes the local repo *is* the cache |

The two tiers compose: a multi-node site that runs zot for distribution can *also* run Tier A's
versioning repo on each compute node for the "I edited the metadata twice today, only the manifest
changes" inner loop. The same `tessera get` verb (§3) consults the local repo first, then falls through
to a `pull localhost:5000/...` against zot.

### §2 — Why `pulledWithin` beats MinIO ILM (the rejected alternative)

The temptation is to skip a registry entirely: "just point MinIO at NVMe, set an ILM policy, done." It
doesn't work as a cache. **MinIO ILM transitions / expires objects by their `LastModified` (upload
time), not by access time** — the `AccessTier` rules are about object class, and there is no
"`accessedWithin`" predicate in the S3 lifecycle spec. (S3 has Intelligent-Tiering for *cost class*
moves, but that's a transition, not an eviction by recency suitable for a deletable cache; and it's
AWS-only.) Concretely:

- A 6-month-old dataset queried twice this week would be evicted by an "expire after 30 days" ILM rule.
  Wrong direction for a cache.
- A yesterday-uploaded but never-pulled artifact stays under any age-based rule. Also wrong.
- The S3 ETag is the sha256 of the object — useful for revalidation, but the *eviction* decision still
  has nowhere to put "this was pulled an hour ago."

`zot`'s `storage.retention.policies[].keepTags[].pulledWithin: 720h` (30 days) is exactly the predicate a
cache needs: tag was last *pulled* (not pushed) within N hours → keep; otherwise eligible for sweep. It
is the one feature MinIO doesn't have and the reason this ADR picks a registry-grade cache over an
S3-prefix cache for Tier B.

Tier A's atime-LRU is the same principle implemented for the lighter case: `atime` *is* "pulled within"
for a filesystem repo, and a `find -atime +30 -delete` style sweep (gated by a `du` ceiling for a hard
byte cap) is the trivially-correct local equivalent.

### §3 — The one new in-scope verb: `tessera get <ref> → path`

A thin **fetch** that resolves a ref to a local verified `.tsra` path. *Cache-agnostic*: it doesn't own
a cache; it consults `$TESSERA_CACHE` (a Tier-A repository) when set and otherwise writes to a
tempfile / `--out`. Sketch (do **not** implement in this spike — design only):

```text
tessera get <ref> [--out PATH] [--cache REPO]

<ref> is one of:
  s3://bucket/key                       # cloud range-read (open_url, #225)
  http(s)://host/key                    # cloud range-read
  oci://host[:port]/repo:tag            # OCI artifact pull (registry.rs, #209)
  ./local.tsra                          # passthrough (returns the path)
  <digest>                              # look up in $TESSERA_CACHE only (offline)

Behavior:
  1. If <ref> is a local path → print path, exit 0 (no-op).
  2. If --cache REPO is set (or $TESSERA_CACHE) AND ref resolves to a manifest_hash already in
     the repo (oci: read the manifest annotation `vnd.tessera.manifest_hash` *before* fetching
     blobs; s3: HEAD + tail-prefetch the manifest cheaply) → materialise from the cache, print path.
  3. Otherwise fetch:
       oci://…  → registry::pull (existing)
       s3://…   → cloud::open_url + stream to local, OR a verbatim object_store::get
       http://… → ditto
     Verify magic + manifest seal on the destination path. Print path.
  4. If --cache is set, also `import` the fetched .tsra into the cache repo (one-shot
     deposit so the next call hits step 2). The repo's content-addressing makes this a no-op
     when every block is already stored.

Exit: 0 on success, prints the local path on stdout (one line, scriptable).
```

`get` is **the** verb the Python/shell ergonomics need (see Tier A workflow above). It is *not* a cache
implementation — every cache decision lives in `--cache` / `$TESSERA_CACHE`, both of which point at an
existing first-class Tessera artifact (a repository). There is no daemon, no protocol, no extra service.

Why it earns its place in the CLI even though caching doesn't: every external user — Python
notebook, shell pipeline, Snakemake rule — needs a "URL → local path" step before opening, and writing
that step ad-hoc in every script is the kind of friction that pushes people back to "just `aws s3 cp`."
Owning the one-liner in `tessera` makes verify-on-fetch + de-dup-into-cache automatic and consistent.

### §4 — The standalone example flake (Tier B, the harder one)

A NixOS module + a `nix run`-able app under `tessera/docs/examples/cache-node/`:

- `flake.nix` exposes `nixosModules.tessera-cache` — `services.tessera-cache = { enable; upstream;
  cacheDir; decay; }` → renders a `zot-config.json` with `extensions.sync` (`onDemand: true`, upstream
  URL + creds), `storage.retention.policies[]` (`pulledWithin: <decay>`), `storage.rootDirectory = <cacheDir>`,
  and a `systemd.services.zot` unit.
- `apps.cache-node` — `nix run .#cache-node` renders the same config and `exec`s `zot serve` (so a non-NixOS
  Linux host can use the same shape without nix-darwin / NixOS).
- A committed sample rendered `zot-config.json` (reviewable without nix).
- `README.md` with the staged workflow + the Python notebook snippet for both access paths.

The example flake is **separate from the repo root flake** (its own `flake.nix`, in
`tessera/docs/examples/cache-node/`), so it can't break the repo's hermetic gate. The repo's `crane`
`src` filter already includes `docs/examples/` (so the ingest-spec example survives `cleanCargoSource`),
which means this example dir is carried along but its `flake.nix` is *not* a flake-input of the root
flake — it's only ever evaluated standalone.

### §5 — Rejected / deferred

- **Tessera-native cache daemon.** Hard NO. Would invent a new component, a new protocol, a new
  failure mode, and tie the format to a service. The two tiers compose every workflow we have, with
  zero new code on the format side.
- **MinIO ILM as the cache eviction policy.** Rejected — wrong predicate (§2). MinIO is great as the
  canonical store; it is not the cache tier.
- **Squid / NGINX as an OCI proxy.** Possible but the eviction semantics aren't OCI-aware (no
  `pulledWithin`-on-tags, no manifest-vs-blob distinction for replication). `zot` is purpose-built for
  exactly this and one binary lighter than the alternative.
- **Storing decoded blocks (decompressed Vortex/Zarr/array) in a "decoded cache."** Not in scope — the
  cache layer holds *bytes* (`.tsra` archives and their blob layers). Decoding is the reader's job,
  and Tessera's per-block parallel decode + tail-prefetch make a per-block decoded cache unnecessary
  for the workloads measured so far.
- **A read-side `tessera cache verify` sweep.** Unnecessary — every `Reader::open` verifies magic +
  seal, and `tessera verify` recursively checks every block digest. The cache cannot serve corrupt
  bytes without the reader detecting it. (`tessera gc` already exists for the repo; `zot` has its own
  GC.)

### §6 — Staged workflow (the path the field actually walks)

A single rollout, not a flag day:

1. **Today — no MinIO, one workstation.** `tessera init ~/.cache/tessera/repo` + `tessera import` the
   `.tsra` files the researcher already has. The repo IS the cache. The atime-LRU sweep (Tier A in the
   example) bounds it.
2. **MinIO comes online.** Add `tessera push file.tsra reg.example.com/lab/x:v1` on the producer side
   (`reg.example.com` is the MinIO-backed upstream registry). Consumer behavior unchanged — they still
   `tessera get`/`open` local paths from the repo.
3. **Second compute node joins.** Stand up `services.tessera-cache` (Tier B) on whichever node owns the
   shared NVMe; point both compute nodes at it via `tessera pull localhost:5000/lab/x:v1`. The first
   pull from each node populates the local zot; thereafter both nodes share LAN-speed pulls.
4. **Cohort-wide query, no MinIO down.** `tessera read s3://bucket/cohort/* …` (#225 prune-before-fetch).
   The cache tier is irrelevant to this path — it's a *direct* range-read against the canonical store.
   This is the right shape when one query touches many products and only fetches the blocks it needs;
   pulling whole `.tsra`s through the cache would defeat that.

Two access paths coexist:

- **Path A (cohort-scale / one-shot reads):** `tessera.open("s3://bucket/key")` → range-GET + prune.
- **Path B (repeated reads of the same products):** `tessera pull oci://localhost:5000/...` → local file
  → `tessera.open(path)`. The cache earns its keep on N>1 opens of the same artifact.

## Why

- The two real shapes the field has (single workstation; multi-node lab) need *different* infra; one
  cache abstraction would either be a daemon (too much) or a script (too little). Picking one tier per
  deployment, both composable, is the smallest answer.
- `pulledWithin` is the one piece of registry tech MinIO lacks and a cache needs — it's the entire
  reason this ADR mentions zot by name rather than "a registry of your choice." (Harbor, GHCR, ECR
  retention is creation-age too, with policy hooks.)
- Keeping the cache out of the `tessera` CLI keeps the format independent of any deployment choice. A
  user with no cache, a Tier-A user, and a Tier-B user all run the same Tessera binaries; the cache is
  a directory + a registry URL.
- The one new verb (`tessera get`) does *one* thing — fetch + verify + deposit into a configured cache
  — and is the unavoidable URL-to-path step every script writes anyway. Owning it in the CLI is what
  makes "use a cache" a zero-friction decision instead of an architectural one.

## Consequences

- **No new format surface.** The container, the manifest, the seal, the signature envelope — all
  unchanged. A Tier-B compute node and a no-cache laptop read byte-identical `.tsra`s.
- **`tessera get` is the only CLI addition** (additive, no flag day). It honors `$TESSERA_CACHE` and
  defaults to a tempfile when unset.
- **Two example artifacts ship under `tessera/docs/examples/cache-node/`**: the standalone Nix flake +
  module + `nix run` app, and a rendered `zot-config.json` for hosts that aren't NixOS. They are
  documentation + a deployable starting point; **not** Cargo-linked, not part of the gate.
- **`tessera gc` and the atime-LRU sweep are documented as siblings** that target the same `objects/`
  directory with different policies (reachability vs recency). Operators can run either, both, or
  neither.

## Open questions

- **`tessera get` cache-hit fast path for `oci://`.** Step 2 in §3's pseudocode wants to check
  `vnd.tessera.manifest_hash` from the OCI artifact's annotations *before* pulling the blob. The
  `registry.rs` client today fetches the manifest then the blob; making the manifest-only fetch a
  public helper (`registry::head_manifest_hash`) is the smallest enabling change.
- **`$TESSERA_CACHE` discovery.** The default location (`$XDG_CACHE_HOME/tessera/repo` or
  `~/.cache/tessera/repo`) and whether `tessera get` should auto-`init` an absent cache repo. Auto-init
  is friendlier; explicit-only is safer for shared hosts.
- **Tier B authentication.** The example flake passes basic-auth creds to zot's `extensions.sync` via
  `credentialsFile`; the *upstream* MinIO-backed registry's auth shape (Harbor robot accounts? a
  static htpasswd?) is site-specific and noted in the README, not in this ADR.
- **Decoded cache (chunk-level).** Out of scope here. If a workload re-decodes the same Vortex chunk
  thousands of times, an in-memory LRU at the `Reader` layer might earn its keep — but that's a
  reader-layer optimisation orthogonal to the *bytes* cache this ADR is about.
- **WORM interaction.** Tier B's zot is *not* WORM (caches must evict). The upstream MinIO is the
  WORM tier (ADR-0033 raw=Compliance). The compute node only ever has a cache copy; the canonical
  retention story lives on the storage server. Worth a sentence in the README.
