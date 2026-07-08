//! The shell's state + input logic — pure and terminal-free, so it is fully unit-testable and the
//! [`crate::ui`] renderer + the `run` event loop are thin shells over it.
//!
//! [`App`] holds the opened product (manifest + aux names + the [`NodeTree`]), the active [`Mode`] and
//! [`Layout`], and the navigator's selection/collapse state. All mutation goes through methods that a
//! test drives directly ([`App::new`] from in-memory data, then `on_key`) — no file, no terminal.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use tessera_core::{Manifest, SchemaRegistry};
use tessera_explore::diff::{manifest_diff, ManifestDiff};
use tessera_explore::hierarchy::{hierarchy, Node, NodeTree};
use tessera_explore::inspect::{inspect_facets, InspectFacets};
use tessera_explore::verify::{artifact_verdict, ArtifactVerdict};

use crate::config::{InspectTab, Layout, Mode};
use crate::data::{self, DataView};

/// A loaded Compare target — the two products' display titles + their manifest [`ManifestDiff`].
#[derive(Debug, Clone)]
pub struct CompareView {
    /// Display title of side A (the opened file).
    pub a_title: String,
    /// Display title of side B (the `--compare` target).
    pub b_title: String,
    /// The field-level diff (A = opened, B = target).
    pub diff: ManifestDiff,
}

/// One visible row of the navigator — a node at a tree depth, plus whether it can expand and is
/// currently collapsed. Produced by [`App::rows`] against the current collapse set.
#[derive(Debug, Clone)]
pub struct Row<'a> {
    /// Path of child-indices from the root (`[]` = root, `[2, 0]` = root.children[2].children[0]).
    pub path: Vec<usize>,
    /// Indentation depth (root = 0).
    pub depth: usize,
    /// The node at this row.
    pub node: &'a Node,
    /// True if the node has children (can be expanded/collapsed).
    pub expandable: bool,
    /// True if the node is currently collapsed (its children are hidden).
    pub collapsed: bool,
}

/// A one-glance structural status for the header verdict strip — derived from the **manifest alone**
/// (no payload decode, no re-hash). It reports what is *declared* (sealed / signed-present / schema
/// known+conformant / PHI-declared), deliberately NOT a deep integrity proof — that is the Verify
/// mode's `ArtifactVerdict` (Phase 1b-c). Honest by construction: "sealed", not "verified".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeaderStatus {
    /// The manifest carries a seal (`manifest_hash`).
    pub sealed: bool,
    /// A signature member is present in the container (`aux/signatures/…`). Presence, not validity.
    pub signed: bool,
    /// Schema conformance state (known+conformant / known+non-conformant / open-world).
    pub schema: SchemaState,
    /// The product's schema declares one or more directly-identifying (PHI) fields.
    pub has_phi: bool,
    /// Number of storage blocks.
    pub blocks: usize,
}

/// Whether the product's schema is known and, if so, whether the manifest conforms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaState {
    /// Schema known (embedded or registry) and the manifest validates against it.
    Conformant,
    /// Schema known but the manifest fails validation.
    NonConformant,
    /// No schema for this product — open-world (validation not applicable).
    OpenWorld,
}

/// The shell application state.
pub struct App {
    /// Display title for the header (filename when opened from disk, else the product name).
    pub title: String,
    /// The product manifest.
    pub manifest: Manifest,
    /// The container's aux member names (`Reader::aux_names()`), for the structural signature check.
    pub aux: Vec<String>,
    /// The active layout (defaults + policy).
    pub layout: Layout,
    /// The active mode.
    pub mode: Mode,
    /// The structural hierarchy rendered by the navigator.
    pub tree: NodeTree,
    /// Collapse set — paths (see [`Row::path`]) whose children are hidden.
    collapsed: HashSet<Vec<usize>>,
    /// Selected navigator row (index into [`App::rows`]).
    selected: usize,
    /// The file this shell was opened from, for lazy Data-mode block reads. `None` for an in-memory
    /// (test/embed) shell — Data mode then reports it has no file to read.
    path: Option<PathBuf>,
    /// The loaded Data-mode view for the current block (see [`App::sync_data`]).
    data: DataView,
    /// The block name `data` was loaded for, so it reloads only when the selection changes block.
    data_key: Option<String>,
    /// Scroll offset (first visible row) within a table [`DataView`].
    data_offset: usize,
    /// Array Data sub-view: `false` = histogram (default), `true` = the MIP/plane image.
    show_image: bool,
    /// The deep verification verdict, computed lazily on first entering Verify mode and cached (a
    /// sealed `.tsra` is immutable). `None` until computed / when there is no file.
    verdict: Option<ArtifactVerdict>,
    /// The active Inspect tab (the metadata inspector the right pane shows in Inspect mode).
    active_tab: InspectTab,
    /// The cheap Inspect-tab facets, computed lazily on first entering Inspect mode and cached
    /// (manifest-only — no block decode). `None` until computed / when there is no file.
    facets: Option<InspectFacets>,
    /// The loaded Compare target, when opened with one; `None` = the empty Compare state.
    compare: Option<CompareView>,
    /// Set when the user asks to quit.
    pub should_quit: bool,
}

impl App {
    /// Build the shell from already-loaded data — the test/embed entry point (no file, no terminal).
    /// The initial mode is the layout's `default_mode`.
    pub fn new(
        title: impl Into<String>,
        manifest: Manifest,
        aux: Vec<String>,
        layout: Layout,
    ) -> App {
        let tree = hierarchy(&manifest, &aux);
        let mode = layout.default_mode;
        // Open Inspect on the layout's first pinned tab (the persona's headline concern), else Integrity.
        let active_tab = layout
            .pinned
            .first()
            .copied()
            .unwrap_or(InspectTab::Integrity);
        App {
            title: title.into(),
            manifest,
            aux,
            layout,
            mode,
            tree,
            collapsed: HashSet::new(),
            selected: 0,
            path: None,
            data: DataView::Unavailable(
                "Select a block in the navigator (1 Navigate), then press 3 to view its data."
                    .into(),
            ),
            data_key: None,
            data_offset: 0,
            show_image: false,
            verdict: None,
            active_tab,
            facets: None,
            compare: None,
            should_quit: false,
        }
    }

    /// Open a `.tsra` from disk and build the shell over it. Reads only the manifest + container
    /// directory (aux names) — no block payloads are decoded until Data mode asks for a block. When
    /// `compare` is given, its manifest is diffed against `path`'s for Compare mode (cheap — manifests
    /// only, no payload decode).
    pub fn open(path: &Path, layout: Layout, compare: Option<&Path>) -> tessera_core::Result<App> {
        let reader = tessera_io::Reader::open(path)?;
        let manifest = reader.manifest().clone();
        let aux = reader.aux_names();
        let title = file_label(path);
        let mut app = App::new(title, manifest, aux, layout);
        app.path = Some(path.to_path_buf());
        if let Some(other) = compare {
            let target = tessera_io::Reader::open(other)?.manifest().clone();
            app.compare = Some(CompareView {
                a_title: file_label(path),
                b_title: file_label(other),
                diff: manifest_diff(&app.manifest, &target),
            });
        }
        // Populate the active mode's view if the layout opens directly in Data (analyst), Verify
        // (auditor), or Inspect (steward / fair).
        app.sync_data();
        app.sync_verify();
        app.sync_inspect();
        Ok(app)
    }

    /// The currently-visible navigator rows, honouring the collapse set (depth-first, root first).
    pub fn rows(&self) -> Vec<Row<'_>> {
        let mut out = Vec::new();
        walk(&self.tree.root, Vec::new(), 0, &self.collapsed, &mut out);
        out
    }

    /// Index of the selected navigator row (always clamped into range by the movement methods).
    pub fn selected_index(&self) -> usize {
        self.selected
    }

    /// The node under the selection, if any (drives the Navigate content pane).
    pub fn selected_node(&self) -> Option<&Node> {
        self.rows().get(self.selected).map(|r| r.node)
    }

    /// The structural header status (see [`HeaderStatus`]) — manifest-only, no decode.
    pub fn header_status(&self) -> HeaderStatus {
        let m = &self.manifest;
        let embedded = m
            .schema
            .as_ref()
            .and_then(|v| tessera_core::ProductSchema::from_value(v).ok());
        let known = m.schema.is_some() || SchemaRegistry::builtin().get(&m.product).is_some();
        let schema = if !known {
            SchemaState::OpenWorld
        } else if tessera_core::validate_manifest(m).is_ok() {
            SchemaState::Conformant
        } else {
            SchemaState::NonConformant
        };
        // PHI is a schema property (does the product *declare* directly-identifying fields), cheap to
        // read from the embedded schema when present.
        let has_phi = embedded
            .map(|s| {
                s.fields.iter().any(|f| {
                    matches!(
                        f.sensitivity,
                        tessera_core::schema::Sensitivity::Identifying
                    )
                })
            })
            .unwrap_or(false);
        HeaderStatus {
            sealed: m.manifest_hash.is_some(),
            signed: self.aux.iter().any(|n| n.starts_with("signatures/")),
            schema,
            has_phi,
            blocks: m.blocks.len(),
        }
    }

    /// Move the selection down one visible row (saturating at the last).
    pub fn select_next(&mut self) {
        let n = self.rows().len();
        if n > 0 {
            self.selected = (self.selected + 1).min(n - 1);
        }
    }

    /// Move the selection up one visible row (saturating at the first).
    pub fn select_prev(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    /// Toggle expand/collapse on the selected row (no-op if it has no children). Selection stays on
    /// the same row (its path is unaffected by collapsing its own subtree).
    pub fn toggle_selected(&mut self) {
        let rows = self.rows();
        let Some(row) = rows.get(self.selected) else {
            return;
        };
        if !row.expandable {
            return;
        }
        let path = row.path.clone();
        if !self.collapsed.remove(&path) {
            self.collapsed.insert(path);
        }
    }

    /// Set the active mode.
    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
    }

    /// The loaded Data-mode view for the current selection (see [`App::sync_data`]).
    pub fn data(&self) -> &DataView {
        &self.data
    }

    /// The current table scroll offset (first visible row) for Data mode.
    pub fn data_offset(&self) -> usize {
        self.data_offset
    }

    /// Whether the array Data pane shows the image (`true`) or the histogram (`false`).
    pub fn show_image(&self) -> bool {
        self.show_image
    }

    /// The deep verification verdict, once computed (see [`App::sync_verify`]).
    pub fn verdict(&self) -> Option<&ArtifactVerdict> {
        self.verdict.as_ref()
    }

    /// The loaded Compare target, when opened with `--compare`; `None` = the empty Compare state.
    pub fn compare(&self) -> Option<&CompareView> {
        self.compare.as_ref()
    }

    /// Compute the deep [`ArtifactVerdict`] the first time Verify mode is entered, then cache it (the
    /// container is immutable). Reads + re-hashes every block — call from the event loop after input,
    /// never from render. A no-op outside Verify, once cached, or without a file.
    pub fn sync_verify(&mut self) {
        if self.mode != Mode::Verify || self.verdict.is_some() {
            return;
        }
        if let Some(p) = self.path.clone() {
            if let Ok(mut reader) = tessera_io::Reader::open(&p) {
                self.verdict = Some(artifact_verdict(&mut reader));
            }
        }
    }

    /// Inject a verdict directly — the test seam (normal operation goes through [`App::sync_verify`]).
    pub fn set_verdict(&mut self, verdict: ArtifactVerdict) {
        self.verdict = Some(verdict);
    }

    /// Compute the cheap Inspect-tab [`InspectFacets`] the first time Inspect mode is entered, then
    /// cache them (the container is immutable). Manifest + aux + the small signature member only — no
    /// block decode — so it is safe after input, but kept out of render like the other syncs. Prefers
    /// opening the container (so the Trust tab sees the embedded signature); falls back to a
    /// manifest-only projection for an in-memory shell / when the file can't be opened. A no-op outside
    /// Inspect and once cached.
    pub fn sync_inspect(&mut self) {
        if self.mode != Mode::Inspect || self.facets.is_some() {
            return;
        }
        let facets = self
            .path
            .clone()
            .and_then(|p| tessera_io::Reader::open(&p).ok())
            .map(|mut r| inspect_facets(&mut r))
            .unwrap_or_else(|| tessera_explore::inspect::facets_from(&self.manifest, None));
        self.facets = Some(facets);
    }

    /// Inject facets directly — the test seam (normal operation goes through [`App::sync_inspect`]).
    pub fn set_facets(&mut self, facets: InspectFacets) {
        self.facets = Some(facets);
    }

    /// The cached Inspect-tab facets, once computed (see [`App::sync_inspect`]).
    pub fn facets(&self) -> Option<&InspectFacets> {
        self.facets.as_ref()
    }

    /// The active Inspect tab (the metadata inspector shown in Inspect mode).
    pub fn active_tab(&self) -> InspectTab {
        self.active_tab
    }

    /// Cycle the active Inspect tab by `delta` (wrapping) through [`InspectTab::ORDER`] — the tab
    /// switcher bound to `h`/`l` (Left/Right) while in Inspect mode.
    pub fn cycle_tab(&mut self, delta: isize) {
        let order = InspectTab::ORDER;
        let cur = order
            .iter()
            .position(|&t| t == self.active_tab)
            .unwrap_or(0) as isize;
        let n = order.len() as isize;
        let next = ((cur + delta) % n + n) % n;
        self.active_tab = order[next as usize];
    }

    /// Inject a Compare view directly — the test seam (normal operation loads it in [`App::open`]).
    pub fn set_compare(&mut self, compare: CompareView) {
        self.compare = Some(compare);
    }

    /// Inject a Data view directly — the seam used by tests (and any out-of-band loader). Resets the
    /// scroll offset. Normal operation goes through [`App::sync_data`].
    pub fn set_data(&mut self, data: DataView) {
        self.data = data;
        self.data_offset = 0;
    }

    /// Load (or reload) the Data-mode view when the mode is Data and the selected block changed. Does
    /// the block read/decode — call it from the event loop after input, never from render. A no-op in
    /// other modes and when the selection's block is unchanged (the view is cached).
    pub fn sync_data(&mut self) {
        if self.mode != Mode::Data {
            return;
        }
        let key = self.selected_node().and_then(data::block_of);
        if key == self.data_key {
            return;
        }
        let view = match self.path.clone() {
            Some(p) => self
                .selected_node()
                .map(|n| DataView::load(&p, n))
                .unwrap_or_else(|| DataView::Unavailable("no selection".into())),
            None => DataView::Unavailable("Data mode: no file loaded (in-memory shell).".into()),
        };
        self.data = view;
        self.data_key = key;
        self.data_offset = 0;
    }

    /// Scroll the table Data view by `delta` rows, clamped to the loaded page.
    fn scroll_data(&mut self, delta: isize) {
        let max = self.data.table_len().saturating_sub(1) as isize;
        let next = (self.data_offset as isize + delta).clamp(0, max.max(0));
        self.data_offset = next as usize;
    }

    /// Advance to the next mode in footer order (wraps) — the `Tab` / `]` binding.
    pub fn next_mode(&mut self) {
        let i = Mode::ORDER
            .iter()
            .position(|&m| m == self.mode)
            .unwrap_or(0);
        self.mode = Mode::ORDER[(i + 1) % Mode::ORDER.len()];
    }

    /// Handle a decoded key press. Returns nothing; inspect [`App::should_quit`] / state afterwards.
    /// Kept independent of crossterm's event types via the small [`Key`] enum so it is trivially
    /// testable and the terminal layer does the crossterm→[`Key`] mapping.
    pub fn on_key(&mut self, key: Key) {
        match key {
            Key::Quit => self.should_quit = true,
            // In Data mode, Up/Down scroll the table page; elsewhere they move the navigator.
            Key::Down if self.mode == Mode::Data => self.scroll_data(1),
            Key::Up if self.mode == Mode::Data => self.scroll_data(-1),
            Key::Down => self.select_next(),
            Key::Up => self.select_prev(),
            // In Inspect mode the right pane is a horizontal tab strip, so Left/Right switch tabs
            // (the navigator tree still moves with Up/Down); elsewhere they expand/collapse the tree.
            Key::Expand if self.mode == Mode::Inspect => self.cycle_tab(1),
            Key::Collapse if self.mode == Mode::Inspect => self.cycle_tab(-1),
            Key::Expand | Key::Enter => self.toggle_selected(),
            Key::Collapse => self.collapse_or_parent(),
            Key::NextMode => self.next_mode(),
            Key::ToggleImage if self.mode == Mode::Data => self.show_image = !self.show_image,
            Key::ToggleImage => {}
            Key::Mode(d) => {
                if let Some(m) = Mode::from_digit(d) {
                    self.set_mode(m);
                }
            }
            Key::Other => {}
        }
    }

    /// `h` / Left: collapse the selected node if it is an expanded group; otherwise jump the selection
    /// to its parent row (the familiar tree-navigation gesture).
    fn collapse_or_parent(&mut self) {
        let rows = self.rows();
        let Some(row) = rows.get(self.selected) else {
            return;
        };
        if row.expandable && !row.collapsed {
            let path = row.path.clone();
            self.collapsed.insert(path);
            return;
        }
        // Move to the parent: the nearest earlier row with depth = this.depth - 1.
        let depth = row.depth;
        if depth == 0 {
            return;
        }
        if let Some(parent) = (0..self.selected)
            .rev()
            .find(|&i| rows[i].depth == depth - 1)
        {
            self.selected = parent;
        }
    }
}

/// A terminal-independent key event — the terminal layer maps crossterm keys onto this so [`App`] logic
/// never sees crossterm types (keeps it unit-testable).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// Quit the shell (`q` / `Esc`).
    Quit,
    /// Move selection down (`Down` / `j`).
    Down,
    /// Move selection up (`Up` / `k`).
    Up,
    /// Expand/toggle the selected group (`l` / `Right`).
    Expand,
    /// Collapse the selected group, else go to parent (`h` / `Left`).
    Collapse,
    /// Activate/toggle the selected row (`Enter` / `Space`).
    Enter,
    /// Cycle to the next mode (`Tab`).
    NextMode,
    /// Toggle the array Data pane between histogram and image (`m`).
    ToggleImage,
    /// Jump to mode by footer digit `1..=5`.
    Mode(u8),
    /// Any other key — ignored.
    Other,
}

/// A file's display label — its filename, or a placeholder when it has none.
fn file_label(path: &Path) -> String {
    path.file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("<tsra>")
        .to_string()
}

/// Depth-first walk of the tree into visible [`Row`]s, pruning the children of collapsed nodes.
fn walk<'a>(
    node: &'a Node,
    path: Vec<usize>,
    depth: usize,
    collapsed: &HashSet<Vec<usize>>,
    out: &mut Vec<Row<'a>>,
) {
    let expandable = !node.children.is_empty();
    let is_collapsed = collapsed.contains(&path);
    out.push(Row {
        path: path.clone(),
        depth,
        node,
        expandable,
        collapsed: is_collapsed,
    });
    if expandable && !is_collapsed {
        for (i, child) in node.children.iter().enumerate() {
            let mut cp = path.clone();
            cp.push(i);
            walk(child, cp, depth + 1, collapsed, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tessera_core::block::{BlockKind, BlockRef};

    fn sample_app() -> App {
        let mut m = Manifest::new("pet-ct", "study-014", "d", "2024-01-01T00:00:00Z");
        m.metadata.insert("modality".into(), json!("PT"));
        m.blocks.push(BlockRef {
            name: "events".into(),
            kind: BlockKind::Table,
            digest: Some("blake3:aa".into()),
            spec: json!({"columns": [{"name":"ms","dtype":"u4"}], "rows": 4u64}),
        });
        m.manifest_hash = Some("blake3:sealed".into());
        App::new("study.tsra", m, vec![], Layout::default())
    }

    #[test]
    fn opens_in_the_layouts_default_mode() {
        let mut app = sample_app();
        app.layout = Layout::preset("analyst").unwrap();
        let app = App::new(app.title, app.manifest, app.aux, app.layout);
        assert_eq!(app.mode, Mode::Data); // analyst preset opens on Data
    }

    #[test]
    fn inspect_active_tab_defaults_to_the_first_pinned_tab() {
        // Balanced default pins [Schema, Referencing] → Inspect opens on Schema.
        assert_eq!(sample_app().active_tab(), InspectTab::Schema);
        // The steward preset pins [fair, governance, provenance] → opens on FAIR.
        let mut app = sample_app();
        app.layout = Layout::preset("steward").unwrap();
        let app = App::new(app.title, app.manifest, app.aux, app.layout);
        assert_eq!(app.active_tab(), InspectTab::Fair);
    }

    #[test]
    fn cycle_tab_wraps_both_directions() {
        let mut app = sample_app(); // active = Schema (index 3 in ORDER)
        app.cycle_tab(-1);
        assert_eq!(app.active_tab(), InspectTab::Trust);
        app.cycle_tab(1);
        assert_eq!(app.active_tab(), InspectTab::Schema);
        // Walk forward through all seven and confirm we return to Schema (wrap).
        for _ in 0..InspectTab::ORDER.len() {
            app.cycle_tab(1);
        }
        assert_eq!(app.active_tab(), InspectTab::Schema);
    }

    #[test]
    fn left_right_switch_tabs_only_in_inspect_mode() {
        let mut app = sample_app();
        app.set_mode(Mode::Inspect);
        let before = app.active_tab();
        app.on_key(Key::Expand); // `l` → next tab in Inspect
        assert_ne!(app.active_tab(), before);
        // In Navigate mode the same key expands the tree, never touches the tab.
        app.set_mode(Mode::Navigate);
        let tab = app.active_tab();
        app.on_key(Key::Expand);
        assert_eq!(app.active_tab(), tab);
    }

    #[test]
    fn sync_inspect_computes_manifest_only_facets_without_a_file() {
        let mut app = sample_app(); // no path (in-memory)
        app.set_mode(Mode::Inspect);
        assert!(app.facets().is_none());
        app.sync_inspect();
        let f = app.facets().expect("facets computed from the manifest");
        assert!(f.integrity.sealed);
        assert!(f.trust.signature.is_none()); // no file → no signature member read
    }

    #[test]
    fn navigator_rows_start_fully_expanded_root_first() {
        let app = sample_app();
        let rows = app.rows();
        assert_eq!(rows[0].depth, 0);
        assert_eq!(rows[0].node.label, "study-014");
        // meta group + its field, blocks group + the events block + its column are all visible.
        let labels: Vec<&str> = rows.iter().map(|r| r.node.label.as_str()).collect();
        assert!(labels.contains(&"meta") && labels.contains(&"events") && labels.contains(&"ms"));
    }

    #[test]
    fn collapsing_a_group_hides_its_children() {
        let mut app = sample_app();
        // Select the "blocks" group row, collapse it, and confirm the block + column vanish.
        let blocks_idx = app
            .rows()
            .iter()
            .position(|r| r.node.label == "blocks")
            .unwrap();
        app.selected = blocks_idx;
        let before = app.rows().len();
        app.on_key(Key::Collapse);
        let after = app.rows();
        assert!(after.len() < before);
        assert!(!after.iter().any(|r| r.node.label == "events"));
        // Re-expanding restores them.
        app.on_key(Key::Expand);
        assert!(app.rows().iter().any(|r| r.node.label == "events"));
    }

    #[test]
    fn selection_moves_and_saturates() {
        let mut app = sample_app();
        app.on_key(Key::Up); // already at top → stays
        assert_eq!(app.selected_index(), 0);
        let n = app.rows().len();
        for _ in 0..(n + 5) {
            app.on_key(Key::Down);
        }
        assert_eq!(app.selected_index(), n - 1); // saturates at the last row
    }

    #[test]
    fn digit_keys_switch_modes_and_tab_cycles() {
        let mut app = sample_app();
        assert_eq!(app.mode, Mode::Navigate);
        app.on_key(Key::Mode(3));
        assert_eq!(app.mode, Mode::Data);
        app.on_key(Key::Mode(9)); // out of range → no change
        assert_eq!(app.mode, Mode::Data);
        app.on_key(Key::NextMode);
        assert_eq!(app.mode, Mode::Verify);
    }

    #[test]
    fn header_status_is_structural_and_honest() {
        let app = sample_app();
        let h = app.header_status();
        assert!(h.sealed);
        assert!(!h.signed); // no aux signature member
        assert_eq!(h.blocks, 1);
    }

    #[test]
    fn quit_key_sets_the_flag() {
        let mut app = sample_app();
        assert!(!app.should_quit);
        app.on_key(Key::Quit);
        assert!(app.should_quit);
    }
}
