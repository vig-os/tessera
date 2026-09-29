---
type: issue
state: open
created: 2026-07-03T10:44:36Z
updated: 2026-09-28T17:58:54Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/326
comments: 2
labels: none
assignees: none
milestone: 0.1.0-beta
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:49.217Z
---

# [Issue 326]: [feat(tui): tessera-tui read-only explorer (Phase 1b) — generic config-driven shell over the view-model](https://github.com/vig-os/tessera/issues/326)

Build the ratatui TUI against the signed-off wireframes (docs/spikes/tsra-explorer-wireframes.md). Thin renderer over tessera-explore (Phase 1a done). GENERIC + config-driven shell (layout = TOML data: default_mode/pinned/keymap/policy; personas are preset configs, not code).

Build order (each fmt+clippy+test green; tui-probe headless tests against tessera/corpus):
- [ ] 1b-a: new crate tessera-tui (ratatui + crossterm + ratatui-image); shared chrome (verdict strip + NodeTree navigator + tabbed content + footer); event loop; config-driven layout + preset loading; NodeTree extraction into tessera-explore (structural hierarchy from the manifest) → Navigate + Inspect modes.
- [ ] 1b-b: Data mode (adapts to block kind) — table: paged read_page + optional SQL; array: array_stats + histogram + slice_region/project_axis image via ratatui-image (sixel/kitty + ASCII/braille fallback).
- [ ] 1b-c: Verify mode + ArtifactVerdict extraction (integrity + signature + trust + schema, over tessera-io verify primitives); then Compare (A/B or prior).

New view-model shapes (the only two Phase-1b additions): NodeTree, ArtifactVerdict — both concretely specified by the wireframes.

Refs: #286
---

# [Comment #1]() by [gerchowl]()

_Posted on July 3, 2026 at 11:25 AM_

**1b-a landed** on `feature/286-tsra-explorer` (commits dc36c55, ae68767, d75e71d):

- `tessera-explore::hierarchy` — the `NodeTree` structural view-model (typed `Node`/`NodeKind`/`NodeHandle`, derives Serialize for MCP/serve), built from the manifest + aux names; no decode, no re-hash. The block one-liner + number formatters moved here as the SSOT (nav.rs now imports them; `tree`/`ls` output byte-identical, trycmd green).
- `tessera-tui::config` — config-driven layout ('a layout is data, not code'): `Mode`/`InspectTab`/`Policy`/`Layout` + the four shipped presets (analyst · auditor · steward · fair) as embedded TOML; `deny_unknown_fields` → config typos are hard errors.
- The generic shell (`app`/`ui`/`run`): verdict strip + NodeTree navigator (expand/collapse, selection) + mode-adaptive content pane + footer mode-switcher; the full mode union present. Logic is terminal-free and snapshot-tested against a ratatui `TestBackend`; only `run` touches a TTY (panic-safe raw-mode guard).
- Wired as `tsra tui <file> [--layout <preset|path>]`.

Content depth by mode today: Navigate = node detail card (full); Inspect = Integrity tab + pinned-aware tab bar (other tab bodies → 1b-c); Data = block shape/spec summary (paged/array viewers → 1b-b); Verify = structural checklist, explicitly labelled not-a-deep-verify (ArtifactVerdict → 1b-c); Compare = empty state.

**Honesty caveat:** the shell renders and is fully unit/snapshot-tested, but it has not been driven in a live terminal yet (no TTY in the build env) — smoke-test `tsra tui` against a real `.tsra` in a terminal.

37 tests across the three crates. Header/verify status is manifest-only by design ('sealed'/'sig present', never 'verified') — deep verify is 1b-c. Next: 1b-b (Data mode).

---

# [Comment #2]() by [gerchowl]()

_Posted on July 3, 2026 at 12:43 PM_

**Live smoke-test passed** — the 1b-a caveat is cleared. Drove `tsra tui` in a real PTY via tmux (`tmux new-session` running the binary in the devShell, `send-keys` + `capture-pane -p` to read the rendered screen):

- Renders correctly at the terminal (header verdict strip · navigator tree · mode panes · footer).
- All 5 modes via number keys; j/k navigation moves selection and the **Data pane follows the selection live** (array `volume` → shape/chunks; table `roi` → cols/rows + columns).
- **Config-driven layout confirmed live**: `--layout auditor` opens on Verify with Integrity/Provenance/Trust/Governance pinned (vs balanced default Schema/Referencing) — presets are live data.
- `q` quits cleanly, terminal restored.

Zero bugs — live behavior matches the TestBackend snapshots exactly. Ready to proceed to 1b-b.

