//! The renderer — a pure function of [`App`] state to a ratatui frame. No input, no I/O; the event
//! loop calls [`render`] each tick. Kept separate from [`crate::app`] so the whole visual surface is
//! snapshot-testable against a `TestBackend` buffer (see the tests below).
//!
//! Layout (the shared shell chrome from `docs/spikes/tsra-explorer-wireframes.md`): a top **verdict
//! strip**, a body split into the **navigator** (the [`NodeTree`](tessera_explore::hierarchy)) and a
//! **content pane** that follows the active [`Mode`], and a **footer** mode-switcher. The chrome is
//! identical in every mode; only the content pane changes.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

use tessera_explore::hierarchy::{Node, NodeHandle, NodeKind};

use crate::app::{App, HeaderStatus, SchemaState};
use crate::config::Mode;

/// Render the whole shell for the current [`App`] state into `frame`.
pub fn render(frame: &mut Frame, app: &App) {
    let areas = Layout::vertical([
        Constraint::Length(3), // header verdict strip
        Constraint::Min(0),    // body
        Constraint::Length(1), // footer mode-switcher
    ])
    .split(frame.area());
    render_header(frame, areas[0], app);
    let body = Layout::horizontal([Constraint::Length(38), Constraint::Min(0)]).split(areas[1]);
    render_navigator(frame, body[0], app);
    render_content(frame, body[1], app);
    render_footer(frame, areas[2], app);
}

/// The top bar: title + the persistent verdict strip (seal / signature-present / schema / PHI).
fn render_header(frame: &mut Frame, area: Rect, app: &App) {
    let s = app.header_status();
    let mut spans = vec![
        Span::styled(
            format!(" tessera-tui · {} ", app.title),
            Style::new().add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
    ];
    spans.extend(verdict_spans(&s));
    let block = Block::new().borders(Borders::ALL);
    frame.render_widget(Paragraph::new(Line::from(spans)).block(block), area);
}

/// The verdict glyphs. Honest by construction — `sig present` (a signature member exists), never
/// `sig verified`; that proof is the Verify mode's `ArtifactVerdict` (Phase 1b-c).
fn verdict_spans(s: &HeaderStatus) -> Vec<Span<'static>> {
    let sep = || Span::raw(" · ");
    let ok = Style::new().green();
    let bad = Style::new().red();
    let warn = Style::new().yellow();
    let mut out = Vec::new();
    out.push(if s.sealed {
        Span::styled("seal ✓", ok)
    } else {
        Span::styled("seal ✗", bad)
    });
    out.push(sep());
    out.push(if s.signed {
        Span::styled("sig present", ok)
    } else {
        Span::styled("sig none", Style::new().dim())
    });
    out.push(sep());
    out.push(match s.schema {
        SchemaState::Conformant => Span::styled("schema ✓", ok),
        SchemaState::NonConformant => Span::styled("schema ✗", bad),
        SchemaState::OpenWorld => Span::styled("schema open", Style::new().dim()),
    });
    if s.has_phi {
        out.push(sep());
        out.push(Span::styled("phi ⚠", warn));
    }
    out
}

/// The left pane: the navigator tree as an indented, glyphed list with the selection highlighted.
fn render_navigator(frame: &mut Frame, area: Rect, app: &App) {
    let rows = app.rows();
    let items: Vec<ListItem> = rows
        .iter()
        .map(|r| {
            let indent = "  ".repeat(r.depth);
            let glyph = row_glyph(r.node, r.expandable, r.collapsed);
            let head = format!("{indent}{glyph} {}", r.node.label);
            // A dim, right-of-label detail annotation (count / headline / value).
            let line = Line::from(vec![
                Span::raw(head),
                Span::raw("  "),
                Span::styled(r.node.detail.clone(), Style::new().dim()),
            ]);
            ListItem::new(line)
        })
        .collect();
    let block = Block::new().borders(Borders::ALL).title(Span::styled(
        " NAVIGATOR ",
        Style::new().add_modifier(Modifier::BOLD),
    ));
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::new().add_modifier(Modifier::REVERSED))
        .highlight_symbol("");
    let mut state = ListState::default();
    state.select(Some(app.selected_index()));
    frame.render_stateful_widget(list, area, &mut state);
}

/// The glyph in front of a navigator row: expand/collapse chevrons for groups, a kind glyph for
/// blocks, a dot for inline leaves.
fn row_glyph(node: &Node, expandable: bool, collapsed: bool) -> &'static str {
    if expandable {
        return if collapsed { "▸" } else { "▾" };
    }
    match node.kind {
        NodeKind::Block(kind) => match kind {
            tessera_core::block::BlockKind::Array => "▦",
            tessera_core::block::BlockKind::Table => "▤",
            tessera_core::block::BlockKind::Blob => "◼",
            tessera_core::block::BlockKind::ChunkIndex => "◇",
        },
        _ => "·",
    }
}

/// The right pane: content for the active mode. Chrome is constant; only this pane changes.
fn render_content(frame: &mut Frame, area: Rect, app: &App) {
    let (title, lines) = match app.mode {
        Mode::Navigate => navigate_content(app),
        Mode::Inspect => inspect_content(app),
        Mode::Data => data_content(app),
        Mode::Verify => verify_content(app),
        Mode::Compare => compare_content(app),
    };
    let block = Block::new().borders(Borders::ALL).title(Span::styled(
        title,
        Style::new().add_modifier(Modifier::BOLD),
    ));
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
}

/// Navigate → a detail card for the selected node.
fn navigate_content(app: &App) -> (String, Vec<Line<'static>>) {
    let Some(node) = app.selected_node() else {
        return (" NAVIGATE ".into(), vec![Line::raw("(empty)")]);
    };
    let mut lines = vec![
        kv("kind", kind_label(&node.kind)),
        kv(
            "detail",
            if node.detail.is_empty() {
                "—".to_string()
            } else {
                node.detail.clone()
            },
        ),
        kv("handle", handle_label(node.handle.as_ref())),
    ];
    if let Some(hint) = node_hint(&node.kind) {
        lines.push(Line::raw(""));
        lines.push(Line::styled(hint, Style::new().dim()));
    }
    (format!(" NAVIGATE › {} ", node.label), lines)
}

/// Inspect → the Integrity tab (Phase 1b-a); the tab bar shows the full set with pinned tabs marked.
/// The remaining tabs' bodies land in Phase 1b-c alongside the `ArtifactVerdict`.
fn inspect_content(app: &App) -> (String, Vec<Line<'static>>) {
    let m = &app.manifest;
    let s = app.header_status();
    let block_names: Vec<&str> = m.blocks.iter().map(|b| b.name.as_str()).collect();
    let mut lines = vec![tab_bar(app), Line::raw("")];
    lines.extend([
        kv("id", m.id.clone()),
        kv(
            "manifest_hash",
            format!(
                "{} (version)",
                m.manifest_hash.clone().unwrap_or_else(|| "—".into())
            ),
        ),
        kv("sealed", if s.sealed { "yes" } else { "no" }),
        kv(
            "blocks",
            format!("{}   {}", s.blocks, block_names.join(" · ")),
        ),
        kv(
            "product",
            format!("{}   schema {}", m.product, schema_word(s.schema)),
        ),
    ]);
    (" INSPECT ".into(), lines)
}

/// Data → the selected block's shape + spec (paged read / stats+histogram land in Phase 1b-b).
fn data_content(app: &App) -> (String, Vec<Line<'static>>) {
    match app.selected_node() {
        Some(node) if matches!(node.kind, NodeKind::Block(_)) => {
            let mut lines = vec![
                kv("block", node.label.clone()),
                kv("shape", node.detail.clone()),
            ];
            for child in &node.children {
                lines.push(kv(&child.label, child.detail.clone()));
            }
            (format!(" DATA › {} ", node.label), lines)
        }
        _ => (
            " DATA ".into(),
            vec![Line::styled(
                "Select a block in the navigator (1 Navigate) to view its data.",
                Style::new().dim(),
            )],
        ),
    }
}

/// Verify → the structural checklist (manifest-only). The deep, block-by-block `ArtifactVerdict`
/// (streaming every block digest, signature + trust-store) is Phase 1b-c.
fn verify_content(app: &App) -> (String, Vec<Line<'static>>) {
    let s = app.header_status();
    let mark = |ok: bool| if ok { "✓" } else { "✗" };
    let lines = vec![
        kv("seal", format!("{}  manifest_hash present", mark(s.sealed))),
        kv("signature", format!("{}  member in container", mark(s.signed))),
        kv("schema", schema_word(s.schema).to_string()),
        kv("phi", if s.has_phi { "declares identifying fields" } else { "none declared" }),
        Line::raw(""),
        Line::styled(
            "structural check (manifest only) — deep verify (every block digest + signature + trust) \
             is Phase 1b-c.",
            Style::new().dim(),
        ),
    ];
    (" VERIFY ".into(), lines)
}

/// Compare → the empty state until a second version/product is loaded (Phase 1b-c).
fn compare_content(_app: &App) -> (String, Vec<Line<'static>>) {
    (
        " COMPARE ".into(),
        vec![Line::styled(
            "No comparison target loaded. A/B against a prior version or sibling arrives in Phase 1b-c.",
            Style::new().dim(),
        )],
    )
}

/// The inspector tab bar (`[Integrity] Provenance …`), with pinned tabs starred and the active tab
/// (Integrity in 1b-a) bracketed.
fn tab_bar(app: &App) -> Line<'static> {
    use crate::config::InspectTab;
    let mut spans = Vec::new();
    for (i, tab) in InspectTab::ORDER.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(" "));
        }
        let pinned = app.layout.pinned.contains(tab);
        let label = if pinned {
            format!("{}*", tab.label())
        } else {
            tab.label().to_string()
        };
        // Integrity is the active tab in this build.
        if *tab == InspectTab::Integrity {
            spans.push(Span::styled(
                format!("[{label}]"),
                Style::new().add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled(label, Style::new().dim()));
        }
    }
    Line::from(spans)
}

/// The footer mode-switcher (`1 Navigate · 2 Inspect · …`) with the active mode highlighted, plus the
/// global verbs.
fn render_footer(frame: &mut Frame, area: Rect, app: &App) {
    let mut spans = Vec::new();
    for (i, mode) in Mode::ORDER.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(" · "));
        }
        let label = format!("{} {}", mode.digit(), mode.label());
        if *mode == app.mode {
            spans.push(Span::styled(
                label,
                Style::new().add_modifier(Modifier::REVERSED | Modifier::BOLD),
            ));
        } else {
            spans.push(Span::raw(label));
        }
    }
    spans.push(Span::styled("    ? help · q quit", Style::new().dim()));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// A `key    value` line (dim key, plain value) for the content cards.
fn kv(key: &str, value: impl Into<String>) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{key:<14}"), Style::new().dim()),
        Span::raw(value.into()),
    ])
}

/// A human label for a [`NodeKind`].
fn kind_label(kind: &NodeKind) -> String {
    match kind {
        NodeKind::Product => "product".into(),
        NodeKind::MetaGroup => "metadata group".into(),
        NodeKind::MetaField => "metadata field".into(),
        NodeKind::SchemaGroup => "schema".into(),
        NodeKind::SchemaField => "schema field".into(),
        NodeKind::Referencing => "referencing".into(),
        NodeKind::BlockGroup => "blocks group".into(),
        NodeKind::Block(k) => format!("{k:?} block").to_lowercase(),
        NodeKind::Column => "table column".into(),
        NodeKind::BlockDetail => "block detail".into(),
        NodeKind::SourceGroup => "provenance group".into(),
        NodeKind::Source => "provenance edge".into(),
        NodeKind::ExtraGroup => "extension group".into(),
        NodeKind::ExtraField => "extension field".into(),
        NodeKind::AuxGroup => "aux group".into(),
        NodeKind::AuxMember => "aux member".into(),
    }
}

/// A human label for a node's drill [`NodeHandle`] (`—` when the node is not drillable).
fn handle_label(handle: Option<&NodeHandle>) -> String {
    match handle {
        None => "—".into(),
        Some(NodeHandle::Block(b)) => format!("block:{b}"),
        Some(NodeHandle::Column { block, column }) => format!("column:{block}.{column}"),
        Some(NodeHandle::Extra(k)) => format!("extra/{k}"),
        Some(NodeHandle::Aux(n)) => format!("aux/{n}"),
        Some(NodeHandle::Source(i)) => format!("source[{i}]"),
    }
}

/// A one-line hint for what a node opens (shown in the Navigate card).
fn node_hint(kind: &NodeKind) -> Option<&'static str> {
    match kind {
        NodeKind::Block(_) => Some("press 3 (Data) to view this block's data"),
        NodeKind::MetaField | NodeKind::SchemaGroup | NodeKind::SchemaField => {
            Some("press 2 (Inspect) for the metadata tabs")
        }
        NodeKind::SourceGroup | NodeKind::Source => Some("press 2 (Inspect) › Provenance"),
        _ => None,
    }
}

/// One-word schema-state label for the content cards.
fn schema_word(s: SchemaState) -> &'static str {
    match s {
        SchemaState::Conformant => "✓ conformant",
        SchemaState::NonConformant => "✗ non-conformant",
        SchemaState::OpenWorld => "open-world",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::Terminal;
    use serde_json::json;
    use tessera_core::block::{BlockKind, BlockRef};
    use tessera_core::Manifest;

    use crate::config::Layout;

    fn sample_app() -> App {
        let mut m = Manifest::new("pet-ct", "study-014", "d", "2024-01-01T00:00:00Z");
        m.metadata.insert("modality".into(), json!("PT"));
        m.blocks.push(BlockRef {
            name: "pet_suv".into(),
            kind: BlockKind::Array,
            digest: Some("blake3:bb".into()),
            spec: json!({"dtype":"f32","codec":"pcodec","shape":[200,200,402],"chunks":[64,64,64]}),
        });
        m.manifest_hash = Some("blake3:7c9f".into());
        App::new("study.tsra", m, vec![], Layout::default())
    }

    /// Render `app` into a fixed 90×24 buffer and return it as newline-joined rows.
    fn draw(app: &App) -> String {
        let mut term = Terminal::new(TestBackend::new(90, 24)).unwrap();
        term.draw(|f| render(f, app)).unwrap();
        buffer_text(term.backend().buffer())
    }

    fn buffer_text(buf: &Buffer) -> String {
        let area = buf.area;
        let mut s = String::new();
        for y in 0..area.height {
            for x in 0..area.width {
                s.push_str(buf[(x, y)].symbol());
            }
            s.push('\n');
        }
        s
    }

    #[test]
    fn shell_shows_header_navigator_and_footer_chrome() {
        let out = draw(&sample_app());
        // Header title + verdict strip.
        assert!(out.contains("tessera-tui · study.tsra"), "{out}");
        assert!(out.contains("seal ✓"));
        assert!(out.contains("schema"));
        // Navigator pane + a couple of tree rows.
        assert!(out.contains("NAVIGATOR"));
        assert!(out.contains("study-014"));
        assert!(out.contains("pet_suv"));
        // Footer mode-switcher.
        assert!(out.contains("1 Navigate"));
        assert!(out.contains("5 Compare"));
        assert!(out.contains("q quit"));
    }

    #[test]
    fn navigate_pane_shows_the_selected_node_card() {
        let out = draw(&sample_app());
        // Root selected by default → NAVIGATE card for the product node.
        assert!(out.contains("NAVIGATE › study-014"), "{out}");
        assert!(out.contains("kind"));
        assert!(out.contains("product"));
    }

    #[test]
    fn switching_to_inspect_renders_the_integrity_tab() {
        let mut app = sample_app();
        app.on_key(crate::app::Key::Mode(2));
        let out = draw(&app);
        assert!(out.contains("INSPECT"), "{out}");
        assert!(out.contains("[Integrity]"));
        assert!(out.contains("manifest_hash"));
        // Pinned tabs (Schema, Referencing in the balanced default) are starred.
        assert!(
            out.contains("Schema*") && out.contains("Referencing*"),
            "{out}"
        );
    }

    #[test]
    fn data_pane_follows_the_selected_block() {
        let mut app = sample_app();
        // Move selection onto the pet_suv block, then switch to Data.
        let target = app
            .rows()
            .iter()
            .position(|r| r.node.label == "pet_suv")
            .unwrap();
        while app.selected_index() < target {
            app.on_key(crate::app::Key::Down);
        }
        app.on_key(crate::app::Key::Mode(3));
        let out = draw(&app);
        assert!(out.contains("DATA › pet_suv"), "{out}");
        assert!(out.contains("f32") && out.contains("pcodec"), "{out}");
    }

    #[test]
    fn verify_pane_is_structural_and_labelled_honestly() {
        let mut app = sample_app();
        app.on_key(crate::app::Key::Mode(4));
        let out = draw(&app);
        assert!(out.contains("VERIFY"), "{out}");
        assert!(out.contains("seal"));
        // The honesty caveat is on screen — this is not claimed as a deep verify.
        assert!(out.contains("structural check"), "{out}");
    }
}
