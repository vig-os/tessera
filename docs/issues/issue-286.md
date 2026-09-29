---
type: issue
state: open
created: 2026-07-02T09:46:29Z
updated: 2026-09-28T17:58:56Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/286
comments: 3
labels: none
assignees: none
milestone: 0.1.0-beta
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:58.555Z
---

# [Issue 286]: [tsra data explorer/editor — TUI + shared view-model extraction](https://github.com/vig-os/tessera/issues/286)

## Goal
A human-facing explorer/editor for `.tsra` products. Design spike: `docs/spikes/tsra-explorer.md`.

## Decisions from the spike
- **Form factor:** Rust `tessera-tui` (ratatui) over `tessera-io` — the only option with full data access today (no Python runtime, no wasm decode, no server).
- **SSOT/DRY:** compute lives once in `tessera-core`/`tessera-io`; consumers (CLI, TUI, future `tsra serve`, py) are thin renderers. Web shares the core via a thin `tsra serve` HTTP boundary (compute-next-to-data), **not** wasm — shipping raw volumes to a browser is the wrong compute·data·viz split for medical imaging. `tessera-wasm` stays the pure offline verify/sign path *by design*.
- **Editor model:** a sealed `.tsra` is immutable/content-addressed; the only sanctioned edit is a copy-on-write metadata `commit` (ADR-0036).

## Concrete SSOT debt this surfaces
`tessera-cli/src/nav.rs` (~1841 lines) traps the derived-view/compute layer (`tree/ls/read/stats/slice/project/pyramid`) behind `pub fn … out: &mut dyn Write` — it computes a view and writes text. Not reusable by a TUI (needs a node tree) or a server (needs Arrow/PNG).

## Phases
- **1a** — extract the view-model out of `nav.rs` into the shared core, returning structured data; re-express the CLI as thin text formatters (existing `trycmd` snapshots must pass unchanged).
- **1b** — read-only `crates/tessera-tui` (ratatui) renderer over 1a; tree + table (LogicalTableView) + array stats/slice/MIP (ratatui-image) + verify/sig/provenance status bar. Headless tests via the `tui-probe` skill against `tessera/corpus`.
- **2** — CoW metadata editor (`commit --set` + `log`/`diff` view).
- **3** — (deferred; only if remote-reach / WebGL matters) `tsra serve` + Vite+TS SPA as a third renderer over the 1a view-model.

## Open questions
See the "Open questions" section of the spike doc.
---

# [Comment #1]() by [gerchowl]()

_Posted on July 2, 2026 at 12:14 PM_

Follow-up issues from the storage-format review in `docs/spikes/tsra-explorer.md`:
- #287 — ADR: storage-format posture (sealed product + OME-NGFF at the array layer; not Icechunk/OME-Zarr as the format)
- #288 — study Icechunk (Earthmover) multi-writer transaction model for concurrent CoW commits
- #289 — evaluate virtual/external-reference chunks (kerchunk/Icechunk pattern) — also validates the NGFF facade
- #290 — Icechunk interop bridge (import snapshot → .tsra; export lineage → Icechunk repo)

---

# [Comment #2]() by [gerchowl]()

_Posted on July 2, 2026 at 02:15 PM_

Compute/infra topology across scales (K8s fan-out, aggregated analysis, Ballista, scheduling) split out under epic **#295** (children #296–#299). Documented in `docs/spikes/tsra-explorer.md` § Compute & infrastructure topology; Tier 2 refined to local-first/cluster-optional and the explorer positioned as v0/substrate + provenance niche.

---

# [Comment #3]() by [gerchowl]()

_Posted on September 23, 2026 at 04:14 PM_

Backup pushed: the explorer work now lives on `origin/feature/286-tsra-explorer` @ `0a7342d` (28 commits ahead of dev, last commit 2026-07-08). It was local-only until now — no longer one disk failure from gone.

