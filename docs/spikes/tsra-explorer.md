# Spike — a `.tsra` data explorer / editor (TUI vs Vite+TS web)

Status: **Design spike** (2026-07-02). One-lens design pass on "what should a human-facing
explorer/editor for `.tsra` products be, and what does it build on." Grounded in the code that exists
today on `spike/tessera-core` (this branch's base). **No production code changed by the spike** — it
recommends a form factor and a phased build plan, to be confirmed before implementation.

Related: [[signing-trust-model]] (the verify/sign surface an explorer surfaces), ADR-0036 (CoW
versioning — the only sanctioned "edit" path), ADR-0043 (unified recursive hierarchy — what the tree
view renders).

## The ask

A "data explorer / editor for `.tsra`." Two axes were open: **form factor** (terminal TUI vs a
Vite+TS web app) and **build vs. design-first**. Decision taken up front: *design-first*, then build
the recommended form factor. This doc is that design.

An "editor" needs care up front: a sealed `.tsra` is **content-addressed and immutable**. You do not
mutate bytes in place. The sanctioned edit is a **copy-on-write metadata commit** (ADR-0036): the
`tsra commit --set/--add-block/--remove-block` verbs write a new version of a lineage, reusing every
unchanged block by digest, with a full audit trail (`log`/`diff`). So "editor" here means: *stage a
metadata delta and compose a new sealed version* — not a hex/pixel mutator. Block payloads are edited
only by re-ingesting or deriving (e.g. `project`, `pyramid`).

## What exists today (the substrate an explorer sits on)

Four read/verify surfaces already ship on `spike/tessera-core`. The explorer picks one as its data
plane.

### 1. `tessera-io` — the Rust library (fullest, lowest-level)
`crates/tessera-io/src/container.rs` `Reader`:
- `open(path)` / `open_url(url)` (`cloud` feature: `s3://`, `http(s)://` via range-GET)
- `manifest()`, `block_names()`, `block_group(prefix)`
- `read_block(name)` / `stream_block(name, w)` (bounded-memory, digest-verified) / `read_aux(name)`
- `LogicalTableView` (`multiblock.rs`): cross-block query — `column()`, `logical_table(prefix)`,
  `block_rows()`, prune-before-fetch via the chunk index
- array/table/pyramid helpers (`array.rs`, `table.rs`), world-affine referencing (`referencing.rs`)

This is what the CLI is built on. A Rust TUI links it directly — single static binary, no runtime.

### 2. `tessera-cli` (`tsra`) — the operator surface (full, process-per-call)
Subcommands (from `crates/tessera-cli/src/main.rs`): `inspect · verify · tree · ls · read · stats ·
slice · project · pyramid · init · import · commit · log · diff · seal · publish · forget · gc · push
· pull · extract · unpack · pack · schema · ingest · export · keygen · sign · verify-sig · trust ·
sql` (DataFusion, feature-gated) `· bench`. `tree`/`ls`/`read` already implement the exact navigation
- cross-block projection an explorer needs; `stats`/`slice`/`project` cover arrays; `commit`/`log`/
`diff` are the edit+audit path. Cloud URLs supported where the `cloud` feature is built.

### 3. `tessera-py` — the ergonomic bindings (richest for rapid UI)
`crates/tessera-py/python/tessera/__init__.py` `Reader`: `manifest()` (dict), `block_names()`,
`array(name)`→`np.ndarray`, `array_roi(name, origin, shape)`, `column(block, name)`,
`logical_column(prefix, name)`, `table(name)`→`polars.DataFrame` / `table_arrow()`→`pyarrow.Table` /
`table_dict()`, `read_block()`, `verify()`. numpy hard dep; polars/pyarrow lazy. **Zero glue** to get
render-ready arrays/frames — ideal if the UI layer is Python (Textual).

### 4. `tessera-wasm` — browser (currently metadata-only)
`crates/tessera-wasm/src/lib.rs` exports **only** `verify_manifest`, `verify_signature`,
`product_id`, `version`. It cannot read array/table payloads: the decode path (zarrs/vortex codecs)
does not cross-compile to wasm today (zstd-sys / getrandom / linux-raw-sys blockers, per the runtime
notes). **A pure in-browser reader is blocked on real work here.**

## Architecture: DRY / SOLID / SSOT — the compute · data · viz split

The sharpest design question isn't "TUI or web" — it's **where the single source of truth lives, and
which layer each consumer is allowed to duplicate.** Three concerns can live in different places:

- **Data** — the `.tsra` bytes: local disk, or remote (`s3://` / `http(s)://`, already supported via
  the `cloud` feature's range-GET).
- **Compute** — decode · cross-block query · slice · project · verify · CoW commit. This is the codecs
  (zarrs/vortex) and the manifest logic. It exists **once**, in native Rust (`tessera-core` +
  `tessera-io`).
- **Viz** — pixels on a screen: a terminal `Rect` (ratatui), a browser `<canvas>`, or a GPU render
  pipeline (WebGL / **wgpu** / WebGPU).

**The SSOT rule (refined):** compute is *always* `tessera-io`, reached by every consumer. Viz is
*mostly* per-consumer — a ratatui `Rect` and a WebGL context share nothing, so forcing literal
code-reuse across *those* is where DRY turns into damage. **One exception matters: GPU rendering via
`wgpu`.** `wgpu` is Rust-native and targets a native window, WebGPU-in-browser (via wasm), and WebGL2
(fallback) from **one render pipeline** — so volume ray-march / MIP / windowing shaders can be a
*second* SSOT, shared between a native GPU viewer and the browser. So: share at the **query/compute**
layer always, and at the **GPU-render** layer *if* we build GPU rendering — never at the terminal/DOM
pixel layer.

### "Is the Vite app just wasm over the same tooling the TUI uses?"

It *could* be — wasm is the literal-code-sharing option: compile `tessera-io` to wasm and the browser
runs the same Rust. But that decides the **compute·data·viz split** the wrong way for medical imaging,
and it's blocked today. Compare the two ways a browser can reach the one Rust core:

| Consumer | Data lives | Compute runs | What travels the wire |
|---|---|---|---|
| `tsra` CLI | local/remote | native `io` (local) | bytes → local compute |
| `tessera-tui` | local/remote | native `io`, same host | bytes → local; pixels local |
| Web via **wasm** | fetched to browser | `io`-in-wasm (client) | **whole block** (a 3 GB volume) |
| Web via **`tsra serve`** | at/near server | native `io` (server) | **only the view** (PNG ≈ KB / Arrow) |

wasm ships raw volumes across the network and decodes client-side — wrong for big arrays, and blocked
anyway (codecs don't cross-compile). `serve` keeps compute next to the data and sends only the
*rendered* result — which is the **same prune/compute-before-fetch principle the repo already lives by**
(chunk-index prune-before-fetch, cohort prune). So the web frontend shares the core through a **thin
`tsra serve` HTTP boundary**, not wasm. `tessera-wasm` stays exactly what it is — the pure, offline
**verify/sign** path (no codecs) — *by design, not as a limitation to fix.*

### "wgpu instead of WebGL?" — yes for viz, and it moves the data-travel line

Keep two axes apart: **wasm** = where *compute* runs; **wgpu** = which *GPU API* the viz draws with.
The "not wasm" verdict above is about **decode** and is unchanged. `wgpu` is a strictly better viz
choice than raw WebGL (modern API, and Rust-native → shareable render code, per the SSOT refinement).
But rendering a volume *in the browser* means the volume must reach the browser's GPU — so client-side
`wgpu` re-opens "ship the data," *unless the server bounds it first*. It can: `tsra pyramid` / `tsra
project` (#260) already downsample. Three coherent web architectures:

| Web architecture | Decode | Render | Travels the wire | Interaction | Fits |
|---|---|---|---|---|---|
| **Server-render** | server | server (wgpu) → PNG | 2-D frames (KB) | round-trip/rotate | thin client, huge vols, locked browsers |
| **Client `wgpu`** | server | **browser (WebGPU)** | **bounded pyramid** (MB) | **local, no round-trip** | interactive 3-D, capable browsers |
| Full-wasm | browser (wasm) | browser | whole raw block (GB) | local | ❌ blocked + wrong split |

The middle row beats "ship PNG planes" for *interactive* 3-D: the server decodes + downsamples to a
budget-fitting pyramid level, ships that once (Arrow/binary), the browser ray-marches it with `wgpu` —
no per-frame round-trips, and decode stays native Rust. Only a *bounded, server-prepared* volume
travels. Caveats: **WebGPU availability** (Chrome/Edge/Safari 18+ solid, Firefox rolling out;
locked-down clinical browsers may lag — `wgpu` falls back to WebGL2 but loses compute shaders, so
server-render stays the compatibility floor); and a `wgpu` volume-render pipeline (ray-march + transfer
functions) is real Phase-3+ scope, not a `<canvas>` blitting a server PNG. **Bonus:** the shared render
core also enables an optional **native `wgpu` desktop viewer** — relevant because terminals top out at
slice/MIP images and can't do true 3-D.

```text
     ┌──────────────── SSOT: tessera-core + tessera-io (native Rust, compute lives here ONCE) ─────────────────┐
     │   decode · cross-block query · slice · project · verify · CoW commit · cloud range-read                 │
     └─────────────────────────────────────────────────────────────────────────────────────────────────────────┘
        ▲ links               ▲ links                 ▲ thin HTTP                 ▲ thin FFI          (separate, pure)
        │ directly            │ directly               │ (tsra serve)              │ (tessera-py)      ┌──────────────┐
   ┌────┴────┐          ┌─────┴─────┐          ┌────────┴────────┐         ┌───────┴───────┐          │ tessera-wasm │
   │  tsra   │          │ tessera-  │          │  Vite+TS SPA    │         │ Python /      │          │ verify+sign  │
   │  (CLI)  │          │   tui     │          │ (browser viz)   │         │ notebook      │          │ ONLY, no     │
   │  text   │          │  ratatui  │          │ PNG/Arrow +     │         │ numpy/polars  │          │ codecs       │
   └─────────┘          └───────────┘          │ wgpu, compute   │         └───────────────┘          └──────────────┘
                                               │ stays server    │
                                               └─────────────────┘
```

### The concrete SSOT debt to pay in Phase 1

Today `tessera-cli/src/nav.rs` (1,841 lines) *is* the derived-view layer — `tree`/`ls`/`read`/`stats`/
`slice`/`project`/`pyramid` + world-addressing + table paging — but every entry point is
`pub fn … out: &mut dyn Write`: it computes a view and **writes text**. That logic can't be reused by a
TUI (needs a *tree of nodes*, not a string) or a server (needs *Arrow/PNG*, not CSV). Building the TUI
"cleanly" therefore means **not reimplementing nav** — it means lifting the derived-view/compute out of
the CLI into the shared core (a `tessera-explore` view-model layer, or into `tessera-io`) that returns
**structured data**: a node tree, a page of rows, a decoded region as an `ndarray`, a projection as a 2-D
image buffer. Then:

- **CLI** = view-model + a *text* formatter (thinned; behaviour unchanged, snapshot-tested by `trycmd`)
- **TUI** = view-model + ratatui widgets
- **`tsra serve`** = view-model + an HTTP/PNG/Arrow encoder

One compute/view-model, three renderers. That extraction is the SSOT-respecting foundation *and* it
de-risks Phase 3 for free. It's more work than bolting a TUI onto the side — and it's the difference
between DRY-by-construction and three drifting copies of "how to read a `.tsra`."

### So — is a web frontend even justified?

Challenge it honestly. Three candidate reasons:

1. **Non-image terminals** — *weak.* Sixel/Kitty + an ASCII/heatmap fallback covers it inside the TUI.
2. **Shareable remote access** — *strong.* A URL a collaborator/clinician opens with no terminal, no
   Rust, no install. This is the FAIR-data story: "here is a link to explore the product."
3. **Rich 3-D viz** — *strong.* `wgpu`/WebGPU volume rendering / linked brushing a terminal can't do.

If (2) and (3) don't matter to the users, **drop the web phase** — the TUI is complete for local expert
use, and the `serve` view-model still exists for later. The web is a *viz+reach* play, not a
capability the TUI lacks.

## Form-factor analysis

| Dimension | Rust TUI (ratatui + `tessera-io`) | Python TUI (Textual + `tessera-py`) | Vite+TS web |
|---|---|---|---|
| Data access **today** | ✅ full (direct lib) | ✅ full (numpy/polars) | ❌ needs backend or wasm-read work |
| Runtime deps | none (static binary) | Python + numpy | Node/browser + a server |
| Fits Rust-first repo | ✅ in-tree crate | ◑ bindings-side | ◑ separate stack |
| Time-to-first-pixel | medium (render code) | **fast** (frames→widgets) | slow (server + UI + protocol) |
| Image rendering (CT slice/MIP) | ✅ Kitty/iTerm/Sixel via `ratatui-image` | ✅ same protocols | ✅✅ best (`wgpu`/WebGPU) |
| Shareable / remote | ◑ (ssh) | ◑ (ssh) | ✅✅ URL |
| Edit → CoW commit | ✅ direct repo verbs | ✅ via bindings/subprocess | ◑ via server |
| New blocker introduced | none | Python runtime | wasm-read **or** a serve endpoint |

Web is the only option that is *blocked* on new plumbing. It is not blocked *forever*: the cheap
unblock is not "make codecs compile to wasm" but a thin **`tsra serve`** localhost endpoint that
streams Arrow IPC for tables and PNG-encoded planes for arrays (reusing `tessera-io` + the existing
`slice`/`project` code). That keeps the hard decode in native Rust and makes the browser a pure view
layer. Worth its own phase — not phase 1.

## Recommendation

**Extract the view-model into the shared core, then ship a Rust `tessera-tui` (ratatui) as its second
renderer.** The form factor is a Rust TUI; the *foundation* is the nav extraction above — the order
matters, because building the TUI on the extracted layer is what makes this DRY-by-construction rather
than a fourth copy of "how to read a `.tsra`." Rationale:

1. **SSOT by construction.** One view-model in the core; CLI/TUI/serve are renderers. The extraction is
   pure refactor (CLI behaviour unchanged, held by `trycmd` snapshots) and de-risks Phase 3 for free.
2. **No new blocker.** Full local + `cloud` data access on day one, reusing the exact verified read
   paths — no Python runtime, no wasm decode, no server.
3. **Coherent with the repo.** All logic crates are Rust; `py`/`wasm` are bindings. The TUI is the
   natural next *consumer* of `tessera-io`, shipped as one static binary alongside `tsra`.
4. **Editing has a clean, safe model.** Drive `init`/`commit --set`/`log`/`diff` from the UI: metadata
   deltas compose new CoW versions with an audit trail; sealed bytes are never mutated.
5. **Images are not a blocker.** `ratatui-image` renders real pixels in Kitty/iTerm2/Sixel terminals
   (CT axial plane via `slice`, MIP via `project`), with an ASCII/heatmap fallback elsewhere.

Textual/Python is the runner-up — faster to a first screen because `tessera-py` hands back numpy/polars
— but it drags a Python runtime into a Rust-first tool, can't share the Rust view-model (it'd reach it
by subprocess/FFI), and duplicates the consumer story the bindings already own. Choose it only if the
explorer should live in notebook/Python land.

## Phased build plan

- **Phase 0 — this doc.** Design + architecture + recommendation. *(done)*
- **Phase 1a — extract the view-model (SSOT foundation).** Lift the derived-view/compute out of
  `tessera-cli/src/nav.rs` into the shared core (`tessera-explore`, or a module in `tessera-io`) as
  functions returning **structured data** — a node tree, a page of rows, a decoded region (`ndarray`),
  a projection (2-D image buffer) — *decoupled from any `Write` sink*. Re-express the CLI's
  `tree/ls/read/stats/slice/project` as thin text formatters over it. Acceptance: existing `trycmd`
  snapshots pass unchanged (behaviour identical, structure now reusable).
- **Phase 1b — read-only `tessera-tui` (ratatui).** New crate `crates/tessera-tui`, a *renderer* over
  the Phase-1a view-model.
  - Open a `.tsra` (+ later a repo lineage / `cloud` URL). Left pane: the ADR-0043 hierarchy tree
    (root status — product · schema · sealed · signed —, `meta`, each block, `sources`).
  - Right pane, block-typed: **Table** → paged viewer over `LogicalTableView` (cross-block `events`);
    **Array** → `stats` header + interactive `slice`/`project` via `ratatui-image`; **Blob** → header
    - `extract` action; **meta** → key/value inspector.
  - Status bar: `verify()` result, signature (embedded/sidecar) + provenance (`read_aux`) summary.
  - Tests: drive it headless with the `tui-probe` skill (tmux screenshots + assertions) against the
    committed conformance corpus (`tessera/corpus`).
- **Phase 2 — metadata editor (CoW).** Edit `meta` fields → stage a delta → `commit --set` into a
  repo; show `log` (version history) and `diff` (this vs parent). Guardrail: refuse to present block
  bytes as editable; block-level change = re-ingest/derive.
- **Phase 3 — GPU viz (deferred; justified only by interactive 3-D / remote-reach — see above).** Two
  tiers over a `tsra serve` boundary (Arrow for tables, bounded pyramid volumes + JSON manifest), both
  keeping decode native (`io`), never in wasm:
  - **3a — server-render floor:** `serve` ships flat PNG planes; a thin Vite+TS SPA blits them.
    Compatibility floor (works without WebGPU), shareable URLs.
  - **3b — client `wgpu` (recommended if 3-D matters):** a **shared `wgpu` render core** (volume
    ray-march / MIP / transfer functions) compiled native *and* to wasm→WebGPU; `serve` ships a
    budget-fitting pyramid level once, the browser renders interactively with no round-trips. Falls
    back to 3a (WebGL2) on browsers without WebGPU. The same render core also enables an **optional
    native `wgpu` desktop viewer** (true 3-D the terminal can't do). `tessera-wasm` stays the offline
    verify/sign path, by design.

## Open questions (to confirm before Phase 1)

1. **TUI language** — Rust ratatui (recommended) or Python Textual? (Locks the crate + deps.)
2. **Editor scope in v1** — read-only Phase 1b first, or fold basic metadata `commit` into it?
3. **Image protocol baseline** — assume a Kitty/iTerm/Sixel-capable terminal, or make ASCII/heatmap
   the default and image an opt-in?
4. **Repo vs single-file** — open standalone `.tsra` only in v1, or also browse a CoW repository
   lineage (needs `repo.rs` wiring)?
5. **Web phase — build it at all, and which tier?** Only if remote-reach (a shareable URL) or
   interactive 3-D matters. If yes: server-render floor (3a) for max compatibility, or invest in the
   shared `wgpu` core (3b) for interactive 3-D + a native GPU viewer? If neither matters, Phase 3 is
   dead scope; the `serve` view-model still exists for whenever it isn't.
