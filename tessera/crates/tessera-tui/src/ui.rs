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
use ratatui::widgets::{
    Block, Borders, Cell, List, ListItem, ListState, Paragraph, Row, Table, Wrap,
};
use ratatui::Frame;

use tessera_explore::diff::DiffStatus;
use tessera_explore::hierarchy::{human_bytes, Node, NodeHandle, NodeKind};
use tessera_explore::inspect::{FairCheck, InspectFacets};
use tessera_explore::verify::{ArtifactVerdict, SchemaVerdict, SignatureInfo};

use crate::app::{App, HeaderStatus, SchemaState};
use crate::config::{InspectTab, Mode};
use crate::data::DataView;

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
    // Data mode renders its own widgets (a scrolling table / histogram), not a paragraph.
    if app.mode == Mode::Data {
        let label = app.selected_node().map(|n| n.label.as_str());
        render_data(
            frame,
            area,
            app.data(),
            app.data_offset(),
            label,
            app.show_image(),
        );
        return;
    }
    let (title, lines) = match app.mode {
        Mode::Navigate => navigate_content(app),
        Mode::Inspect => inspect_content(app),
        Mode::Verify => verify_content(app),
        Mode::Compare => compare_content(app),
        Mode::Data => unreachable!("Data handled above"),
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

/// Inspect → the tab bar + the active tab's body (Phase 1c). Every body is a projection of the cached
/// [`InspectFacets`] (manifest-only, no block decode); tabs switch with `h`/`l`.
fn inspect_content(app: &App) -> (String, Vec<Line<'static>>) {
    let active = app.active_tab();
    let mut lines = vec![tab_bar(app), Line::raw("")];
    match app.facets() {
        Some(f) => lines.extend(tab_body(active, f)),
        None => lines.push(Line::styled(
            "Metadata facets not yet computed.",
            Style::new().dim(),
        )),
    }
    (format!(" INSPECT › {} ", active.label()), lines)
}

/// The body lines for one Inspect tab, projected from the [`InspectFacets`].
fn tab_body(tab: InspectTab, f: &InspectFacets) -> Vec<Line<'static>> {
    match tab {
        InspectTab::Integrity => integrity_tab(f),
        InspectTab::Provenance => provenance_tab(f),
        InspectTab::Trust => trust_tab(f),
        InspectTab::Schema => schema_tab(f),
        InspectTab::Referencing => referencing_tab(f),
        InspectTab::Governance => governance_tab(f),
        InspectTab::Fair => fair_tab(f),
    }
}

/// Integrity tab — identity, seal, product/schema, per-block digests.
fn integrity_tab(f: &InspectFacets) -> Vec<Line<'static>> {
    let g = &f.integrity;
    let mut lines = vec![
        kv("id", g.id.clone()),
        kv(
            "manifest_hash",
            format!(
                "{} (version)",
                g.manifest_hash.clone().unwrap_or_else(|| "—".into())
            ),
        ),
        kv("sealed", if g.sealed { "yes" } else { "no" }),
        kv(
            "product",
            format!("{}   schema {}", g.product, schema_verdict_word(g.schema)),
        ),
        Line::raw(""),
        Line::styled(
            format!("blocks ({})", g.blocks.len()),
            Style::new().add_modifier(Modifier::BOLD),
        ),
    ];
    for b in &g.blocks {
        lines.push(Line::from(vec![
            Span::raw(format!("  {} ", b.name)),
            Span::styled(format!("{}  ", b.kind), Style::new().dim()),
            Span::styled(b.digest.clone(), Style::new().dim()),
        ]));
    }
    lines
}

/// Provenance tab — the `sources` DAG edges + a lineage-coverage summary.
fn provenance_tab(f: &InspectFacets) -> Vec<Line<'static>> {
    let p = &f.provenance;
    let mut lines = vec![kv(
        "lineage",
        format!(
            "{} edge(s), {} pin an upstream content_hash",
            p.summary.edges, p.summary.with_content_hash
        ),
    )];
    if p.edges.is_empty() {
        lines.push(Line::styled(
            "no provenance edges recorded (a root/original product)",
            Style::new().dim(),
        ));
        return lines;
    }
    lines.push(Line::raw(""));
    for e in &p.edges {
        let chain = match &e.content_hash {
            Some(h) => format!("  ⛓ {h}"),
            None => "  (no content_hash — chain not closed)".into(),
        };
        lines.push(Line::from(vec![
            Span::styled(
                format!("{} ", e.role),
                Style::new().add_modifier(Modifier::BOLD),
            ),
            Span::raw(e.reference.clone()),
            Span::styled(chain, Style::new().dim()),
        ]));
    }
    lines
}

/// Trust tab — the embedded signature envelope (attribution, never a trust proof).
fn trust_tab(f: &InspectFacets) -> Vec<Line<'static>> {
    match &f.trust.signature {
        None => vec![Line::styled(
            "no signature embedded — this product is unsigned.",
            Style::new().dim(),
        )],
        Some(sig) => signature_lines(sig),
    }
}

/// Shared signature rendering (Trust tab + Verify pane): declared identity + the honest trust caveat.
fn signature_lines(sig: &SignatureInfo) -> Vec<Line<'static>> {
    let mut lines = vec![
        kv("alg", sig.alg.clone()),
        kv("key_id", short(&sig.key_id)),
        kv(
            "signer",
            format!(
                "{}  (attribution)",
                sig.signer.clone().unwrap_or_else(|| "—".into())
            ),
        ),
    ];
    if let Some(ts) = &sig.signed_at {
        lines.push(kv("signed_at", ts.clone()));
    }
    if let Some(fmt) = &sig.key_format {
        lines.push(kv("key_format", fmt.clone()));
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "trust: NOT checked here — run `tsra verify-sig` against a trust store to prove the signer \
         is trusted.",
        Style::new().yellow(),
    ));
    lines
}

/// Schema tab — conformance verdict + the declared field roster (present fields marked).
fn schema_tab(f: &InspectFacets) -> Vec<Line<'static>> {
    let s = &f.schema;
    let mut lines = vec![kv(
        "conformance",
        match s.verdict {
            SchemaVerdict::Conformant => "✓ conformant",
            SchemaVerdict::NonConformant => "✗ non-conformant",
            SchemaVerdict::OpenWorld => "open-world (no schema)",
        },
    )];
    if let Some(h) = &s.heading {
        lines.push(kv("schema", h.clone()));
    }
    if s.fields.is_empty() {
        lines.push(Line::styled(
            "open-world product — no declared field roster.",
            Style::new().dim(),
        ));
        return lines;
    }
    lines.push(Line::raw(""));
    for fr in &s.fields {
        let mark = if fr.present { "✓" } else { "·" };
        let style = if fr.present {
            Style::new()
        } else {
            Style::new().dim()
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{mark} "), style),
            Span::raw(format!("{:<24}", fr.id)),
            Span::styled(
                format!("{} · {}", fr.tier, fr.sensitivity),
                Style::new().dim(),
            ),
        ]));
    }
    lines
}

/// Referencing tab — per-array spatial frames (convention · unit · space · derived spacing).
fn referencing_tab(f: &InspectFacets) -> Vec<Line<'static>> {
    let r = &f.referencing;
    if r.frames.is_empty() {
        return vec![Line::styled(
            "no world frame — this product is index-space only (no voxel→world affine).",
            Style::new().dim(),
        )];
    }
    let mut lines = Vec::new();
    for fr in &r.frames {
        lines.push(Line::styled(
            fr.block.clone(),
            Style::new().add_modifier(Modifier::BOLD),
        ));
        lines.push(kv(
            "convention",
            format!("{} ({})", fr.convention, fr.space),
        ));
        lines.push(kv(
            "spacing",
            format!(
                "{:.3} × {:.3} × {:.3} {}",
                fr.spacing[0], fr.spacing[1], fr.spacing[2], fr.unit
            ),
        ));
        lines.push(Line::raw(""));
    }
    lines
}

/// Governance tab — the PHI / sensitivity posture (schema-declared) + the seal's tamper-evidence.
fn governance_tab(f: &InspectFacets) -> Vec<Line<'static>> {
    let g = &f.governance;
    let c = &g.sensitivity_counts;
    let mut lines = vec![
        kv(
            "phi",
            if g.has_phi {
                "⚠ schema declares directly-identifying (PHI) fields".to_string()
            } else {
                "no directly-identifying fields declared".to_string()
            },
        ),
        kv(
            "sensitivity",
            format!(
                "{} public · {} coded · {} sensitive · {} identifying",
                c.public, c.coded, c.sensitive, c.identifying
            ),
        ),
        kv(
            "immutability",
            if g.sealed {
                "sealed — content-addressed, tamper-evident"
            } else {
                "unsealed — not tamper-evident"
            },
        ),
    ];
    if !g.identifying_in_clear.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            format!(
                "⚠ identifying fields present in the clear: {}",
                g.identifying_in_clear.join(", ")
            ),
            Style::new().yellow(),
        ));
    }
    lines
}

/// FAIR tab — the findable/accessible/interoperable/reusable checklist, each with a short reason.
fn fair_tab(f: &InspectFacets) -> Vec<Line<'static>> {
    let x = &f.fair;
    let mut lines = vec![Line::styled(
        format!("{}/4 FAIR dimensions met", x.met()),
        Style::new().add_modifier(Modifier::BOLD),
    )];
    lines.push(Line::raw(""));
    for (name, c) in [
        ("Findable", &x.findable),
        ("Accessible", &x.accessible),
        ("Interoperable", &x.interoperable),
        ("Reusable", &x.reusable),
    ] {
        lines.push(fair_line(name, c));
    }
    lines
}

/// One FAIR checklist row: a coloured ✓/✗ + the dimension name + its reason.
fn fair_line(name: &str, c: &FairCheck) -> Line<'static> {
    let (mark, style) = if c.met {
        ("✓", Style::new().green())
    } else {
        ("✗", Style::new().red())
    };
    Line::from(vec![
        Span::styled(format!("{mark} "), style),
        Span::styled(
            format!("{name:<15}"),
            Style::new().add_modifier(Modifier::BOLD),
        ),
        Span::styled(c.reason.clone(), Style::new().dim()),
    ])
}

/// Data → the loaded [`DataView`] for the selected block: a scrolling table page, an array
/// stats card with a histogram or MIP image, a blob summary, or a guiding message. Renders its own
/// widgets. `show_image` selects the array sub-view (histogram vs image).
fn render_data(
    frame: &mut Frame,
    area: Rect,
    view: &DataView,
    offset: usize,
    label: Option<&str>,
    show_image: bool,
) {
    let title = match (view, label) {
        (DataView::Table { .. } | DataView::Array { .. } | DataView::Blob { .. }, Some(l)) => {
            format!(" DATA › {l} ")
        }
        _ => " DATA ".into(),
    };
    let block = Block::new().borders(Borders::ALL).title(Span::styled(
        title,
        Style::new().add_modifier(Modifier::BOLD),
    ));
    match view {
        DataView::Table {
            columns,
            rows,
            total,
        } => render_table(frame, area, block, columns, rows, *total, offset),
        DataView::Array { .. } | DataView::Blob { .. } | DataView::Unavailable(_) => {
            let lines = match view {
                // Image sizing uses the inner pane (minus borders + the stats header rows).
                DataView::Array { .. } => {
                    array_lines(view, show_image, area.width.saturating_sub(2))
                }
                DataView::Blob {
                    media_type,
                    size,
                    filename,
                } => vec![
                    kv("kind", "blob (opaque, preserved verbatim)"),
                    kv("media_type", media_type.clone()),
                    kv("size", human_bytes(*size)),
                    kv("filename", filename.clone().unwrap_or_else(|| "—".into())),
                ],
                DataView::Unavailable(msg) => vec![Line::styled(msg.clone(), Style::new().dim())],
                DataView::Table { .. } => unreachable!(),
            };
            frame.render_widget(
                Paragraph::new(lines)
                    .block(block)
                    .wrap(Wrap { trim: false }),
                area,
            );
        }
    }
}

/// Render a table page as a scrolling ratatui [`Table`]: header + the rows from `offset`, with a
/// footer note of the visible window against the true total.
fn render_table(
    frame: &mut Frame,
    area: Rect,
    block: Block,
    columns: &[String],
    rows: &[Vec<String>],
    total: u64,
    offset: usize,
) {
    // Column widths = max(header, any cell) capped at 24 for readability.
    let widths: Vec<Constraint> = (0..columns.len())
        .map(|c| {
            let w = columns[c].len().max(
                rows.iter()
                    .map(|r| r.get(c).map(|s| s.len()).unwrap_or(0))
                    .max()
                    .unwrap_or(0),
            );
            Constraint::Length(w.clamp(3, 24) as u16)
        })
        .collect();
    let header = Row::new(
        columns
            .iter()
            .map(|c| Cell::from(c.clone()).style(Style::new().add_modifier(Modifier::BOLD))),
    );
    // Visible body: from `offset`, as many rows as the pane can hold (minus border + header + note).
    let body_height = area.height.saturating_sub(4) as usize;
    let start = offset.min(rows.len());
    let end = (start + body_height).min(rows.len());
    let last = rows.len().min(total as usize);
    let note = format!(
        " rows {}–{} of {total}{} ",
        start,
        end,
        if (total as usize) > rows.len() {
            format!("  (first {last} loaded)")
        } else {
            String::new()
        }
    );
    let trows = rows[start..end]
        .iter()
        .map(|r| Row::new(r.iter().map(|c| Cell::from(c.clone()))));
    let table = Table::new(trows, widths)
        .header(header)
        .block(block.title_bottom(Span::styled(note, Style::new().dim())));
    frame.render_widget(table, area);
}

/// The array Data card: a stats header (shape/dtype/codec · raw & physical range) followed by either a
/// histogram (`show_image == false`) or a MIP/plane image downsampled to `inner_w` (`show_image ==
/// true`). A footer hint advertises the `m` toggle.
fn array_lines(view: &DataView, show_image: bool, inner_w: u16) -> Vec<Line<'static>> {
    let DataView::Array {
        dtype,
        codec,
        shape,
        unit,
        stats,
        rescale,
        hist,
        image,
    } = view
    else {
        return Vec::new();
    };
    let shape_str = format!(
        "[{}]",
        shape
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    );
    let mut lines = vec![
        kv("dtype", format!("{dtype}   codec {codec}")),
        kv("shape", format!("{shape_str}   voxels {}", stats.count)),
        kv(
            "raw",
            format!(
                "min {}  max {}  mean {:.3}  std {:.3}",
                stats.min, stats.max, stats.mean, stats.std
            ),
        ),
    ];
    if let Some((slope, intercept)) = rescale {
        let u = unit.clone().unwrap_or_default();
        lines.push(kv(
            "physical",
            format!(
                "min {}  max {}  ({slope}·raw + {intercept}) {u}",
                slope * stats.min + intercept,
                slope * stats.max + intercept
            ),
        ));
    }
    lines.push(Line::raw(""));
    // The `m` toggle is only meaningful when an image exists (2-D/3-D arrays).
    let toggle_hint = if image.is_some() {
        "   (m: histogram / image)"
    } else {
        ""
    };
    if show_image {
        match image {
            Some(plane) => {
                lines.push(Line::styled(
                    format!(
                        "{} image  {}×{}{toggle_hint}",
                        plane.kind, plane.width, plane.height
                    ),
                    Style::new().add_modifier(Modifier::BOLD),
                ));
                lines.extend(image_lines(plane, inner_w));
            }
            None => lines.push(Line::styled(
                "no image for this array (needs a 2-D plane or 3-D volume)",
                Style::new().dim(),
            )),
        }
    } else {
        lines.push(Line::styled(
            format!("histogram (raw value distribution){toggle_hint}"),
            Style::new().add_modifier(Modifier::BOLD),
        ));
        let peak = hist.iter().map(|b| b.count).max().unwrap_or(0).max(1);
        for b in hist {
            let width = ((b.count as f64 / peak as f64) * 30.0).round() as usize;
            let bar: String = "█".repeat(width);
            lines.push(Line::from(vec![
                Span::styled(format!("{:>10.3}  ", b.lo), Style::new().dim()),
                Span::styled(bar, Style::new().cyan()),
                Span::styled(format!("  {}", b.count), Style::new().dim()),
            ]));
        }
    }
    lines
}

/// The glyph ramp for intensity rendering (darkest → brightest). Terminal-agnostic — no sixel/kitty
/// dependency, so it renders (and snapshot-captures) everywhere; a high-fidelity image protocol is a
/// later additive refinement.
const RAMP: &[u8] = b" .:-=+*#%@";

/// Downsample an [`ImagePlane`] to fit `inner_w` columns (preserving aspect, halving rows for the ~2:1
/// character cell) and map each cell to a [`RAMP`] glyph by windowed intensity. Nearest-neighbour.
fn image_lines(plane: &crate::data::ImagePlane, inner_w: u16) -> Vec<Line<'static>> {
    let max_cols = (inner_w as usize).clamp(8, 80);
    // Target grid: cap columns at the source width; rows follow the aspect, halved for cell height.
    let cols = plane.width.min(max_cols).max(1);
    let scale = cols as f64 / plane.width as f64;
    let rows = ((plane.height as f64 * scale * 0.5).round() as usize)
        .clamp(1, 40)
        .min(plane.height);
    let span = (plane.max - plane.min).max(f64::MIN_POSITIVE);
    (0..rows)
        .map(|ry| {
            let sy = (ry as f64 / rows as f64 * plane.height as f64) as usize;
            let row: String = (0..cols)
                .map(|rx| {
                    let sx = (rx as f64 / cols as f64 * plane.width as f64) as usize;
                    let v = plane.values[sy * plane.width + sx];
                    let t = ((v - plane.min) / span).clamp(0.0, 1.0);
                    let idx = (t * (RAMP.len() - 1) as f64).round() as usize;
                    RAMP[idx] as char
                })
                .collect();
            Line::styled(row, Style::new().cyan())
        })
        .collect()
}

/// Verify → the deep [`ArtifactVerdict`] (seal + every block digest + schema + signature + lineage).
/// Until it is computed (or when there is no file), a manifest-only structural glance is shown.
fn verify_content(app: &App) -> (String, Vec<Line<'static>>) {
    match app.verdict() {
        Some(v) => (" VERIFY ".into(), verdict_lines(v)),
        None => verify_structural_fallback(app),
    }
}

/// Render an [`ArtifactVerdict`] as the Verify pane's lines: an overall banner, then each dimension.
fn verdict_lines(v: &ArtifactVerdict) -> Vec<Line<'static>> {
    let mark = |ok: bool| if ok { "✓" } else { "✗" };
    // Overall banner — byte integrity (sealed + all blocks verified), coloured.
    let (banner, style) = if v.integrity_ok() {
        (
            "✓ INTACT — sealed, every block digest verified".to_string(),
            Style::new().green().add_modifier(Modifier::BOLD),
        )
    } else {
        (
            "✗ FAILED — integrity check did not pass".to_string(),
            Style::new().red().add_modifier(Modifier::BOLD),
        )
    };
    let mut lines = vec![Line::styled(banner, style), Line::raw("")];
    lines.push(kv(
        "seal",
        format!("{}  manifest_hash present", mark(v.sealed)),
    ));
    // Integrity: N/N blocks, or the first failing block.
    let integ = match &v.integrity.failure {
        None => format!(
            "{}  {}/{} blocks re-hashed & verified",
            mark(v.integrity.all_ok()),
            v.integrity.blocks_ok,
            v.integrity.blocks_total
        ),
        Some(f) => format!(
            "✗  {} of {} ok — {f}",
            v.integrity.blocks_ok, v.integrity.blocks_total
        ),
    };
    lines.push(kv("integrity", integ));
    lines.push(kv(
        "schema",
        match v.schema {
            SchemaVerdict::Conformant => "✓ conformant",
            SchemaVerdict::NonConformant => "✗ non-conformant",
            SchemaVerdict::OpenWorld => "open-world (no schema)",
        },
    ));
    // Signature: attribution only — never claim trust here.
    match &v.signature {
        Some(sig) => {
            let signer = sig.signer.clone().unwrap_or_else(|| "—".into());
            lines.push(kv(
                "signature",
                format!("{} · key {}", sig.alg, short(&sig.key_id)),
            ));
            lines.push(kv("signer", format!("{signer}  (attribution)")));
            if let Some(ts) = &sig.signed_at {
                lines.push(kv("signed_at", ts.clone()));
            }
            lines.push(Line::styled(
                "trust: NOT checked here — run `tsra verify-sig` against a trust store to prove the \
                 signer is trusted.",
                Style::new().yellow(),
            ));
        }
        None => lines.push(kv("signature", "none embedded")),
    }
    lines.push(kv(
        "lineage",
        format!(
            "{} edge(s), {} pin an upstream content_hash",
            v.lineage.edges, v.lineage.with_content_hash
        ),
    ));
    lines
}

/// The pre-compute / no-file fallback for Verify — the manifest-only structural glance.
fn verify_structural_fallback(app: &App) -> (String, Vec<Line<'static>>) {
    let s = app.header_status();
    let mark = |ok: bool| if ok { "✓" } else { "✗" };
    let lines = vec![
        kv("seal", format!("{}  manifest_hash present", mark(s.sealed))),
        kv(
            "signature",
            format!("{}  member in container", mark(s.signed)),
        ),
        kv("schema", schema_word(s.schema).to_string()),
        Line::raw(""),
        Line::styled(
            "structural glance (manifest only) — the deep verdict computes on entering Verify.",
            Style::new().dim(),
        ),
    ];
    (" VERIFY ".into(), lines)
}

/// Shorten a long hex identifier for display (`1a2b3c4d…`).
fn short(id: &str) -> String {
    if id.len() > 12 {
        format!("{}…", &id[..12])
    } else {
        id.to_string()
    }
}

/// Compare → a field-level manifest diff (A = the opened file, B = the `--compare` target), or an
/// empty state prompting how to load a comparison.
fn compare_content(app: &App) -> (String, Vec<Line<'static>>) {
    let Some(cv) = app.compare() else {
        return (
            " COMPARE ".into(),
            vec![
                Line::styled(
                    "No comparison target loaded.",
                    Style::new().add_modifier(Modifier::BOLD),
                ),
                Line::raw(""),
                Line::styled(
                    "Open with `tsra tui <file> --compare <other.tsra>` to diff two products \
                     (or a version against its prior).",
                    Style::new().dim(),
                ),
            ],
        );
    };
    let changed = cv.diff.changed();
    let mut lines = vec![
        Line::from(vec![
            Span::styled("A  ", Style::new().dim()),
            Span::raw(cv.a_title.clone()),
        ]),
        Line::from(vec![
            Span::styled("B  ", Style::new().dim()),
            Span::raw(cv.b_title.clone()),
        ]),
        Line::styled(
            if changed == 0 {
                "identical manifests — no fields differ".to_string()
            } else {
                format!("{changed} of {} fields differ", cv.diff.rows.len())
            },
            Style::new().add_modifier(Modifier::BOLD),
        ),
        Line::raw(""),
    ];
    // The differing rows, coloured by status; unchanged rows are omitted to keep the diff legible.
    for row in cv.diff.rows.iter().filter(|r| r.status != DiffStatus::Same) {
        let (glyph, style) = match row.status {
            DiffStatus::Changed => ("~", Style::new().yellow()),
            DiffStatus::Added => ("+", Style::new().green()),
            DiffStatus::Removed => ("-", Style::new().red()),
            DiffStatus::Same => (" ", Style::new()),
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{glyph} "), style),
            Span::styled(
                format!("{:<20}", row.field),
                Style::new().add_modifier(Modifier::BOLD),
            ),
            Span::styled(row.a.clone(), Style::new().dim()),
            Span::raw("  →  "),
            Span::raw(row.b.clone()),
        ]));
    }
    (
        format!(" COMPARE › {} vs {} ", cv.a_title, cv.b_title),
        lines,
    )
}

/// The inspector tab bar (`[Integrity] Provenance …`), with pinned tabs starred and the **active** tab
/// (switched with `h`/`l`) bracketed + bold.
fn tab_bar(app: &App) -> Line<'static> {
    let active = app.active_tab();
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
        if *tab == active {
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

/// One-word label for a [`SchemaVerdict`] (the view-model's conformance enum), for the Inspect tabs.
fn schema_verdict_word(v: SchemaVerdict) -> &'static str {
    match v {
        SchemaVerdict::Conformant => "✓ conformant",
        SchemaVerdict::NonConformant => "✗ non-conformant",
        SchemaVerdict::OpenWorld => "open-world",
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

    use crate::app::Key;
    use crate::config::Layout;
    use crate::data::{DataView, HistBin};
    use tessera_explore::array::ArrayStats;

    /// Navigate the selection to the row whose node label matches `label`.
    fn select_label(app: &mut App, label: &str) {
        let target = app
            .rows()
            .iter()
            .position(|r| r.node.label == label)
            .unwrap();
        while app.selected_index() < target {
            app.on_key(Key::Down);
        }
    }

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
    fn inspect_opens_on_the_first_pinned_tab_with_starred_pins() {
        let mut app = sample_app();
        app.on_key(crate::app::Key::Mode(2));
        app.sync_inspect(); // manifest-only facets (no file) — the body needs them
        let out = draw(&app);
        assert!(out.contains("INSPECT"), "{out}");
        // Balanced default pins [Schema, Referencing] → Inspect opens on Schema (the first pinned).
        assert!(
            out.contains("[Schema*]"),
            "opens on first pinned tab: {out}"
        );
        assert!(out.contains("conformance"), "schema-tab body: {out}");
        // Both pinned tabs are starred.
        assert!(
            out.contains("Schema*") && out.contains("Referencing*"),
            "{out}"
        );
    }

    #[test]
    fn inspect_tabs_switch_with_left_right_and_render_their_bodies() {
        use crate::app::Key;
        let mut app = sample_app();
        app.on_key(Key::Mode(2));
        app.sync_inspect();
        // Integrity is the default active tab (no pinned tab precedes it in the balanced layout's
        // pinned list? balanced pins Schema first) — assert whatever the layout opened, then cycle.
        // Cycle to each tab with `l` (Expand) and confirm the bracketed tab + a body marker.
        let seen = |app: &App, needle: &str| draw(app).contains(needle);
        // Walk all seven tabs; each should render without panicking and show its heading.
        // Schema + Referencing are pinned in the balanced default, so their active bracket carries the
        // star (`[Schema*]`); the rest are unpinned.
        let markers = [
            ("[Integrity]", "manifest_hash"),
            ("[Provenance]", "lineage"),
            ("[Trust]", "unsigned"),
            ("[Schema*]", "conformance"),
            ("[Referencing*]", "index-space only"),
            ("[Governance]", "sensitivity"),
            ("[FAIR]", "FAIR dimensions met"),
        ];
        // Set the active tab explicitly to Integrity first for a deterministic walk.
        while app.active_tab() != InspectTab::Integrity {
            app.on_key(Key::Expand);
        }
        for (tab_marker, body_marker) in markers {
            assert!(seen(&app, tab_marker), "missing {tab_marker}");
            assert!(
                seen(&app, body_marker),
                "missing body {body_marker} for {tab_marker}"
            );
            app.on_key(Key::Expand); // `l` → next tab
        }
        // `h` (Collapse) cycles backwards.
        app.on_key(Key::Collapse);
        assert!(seen(&app, "[FAIR]"), "h should wrap back to FAIR");
    }

    #[test]
    fn inspect_governance_flags_phi_and_fair_scores() {
        use crate::app::Key;
        // pet-ct is a builtin schema that declares identifying fields → PHI + a FAIR score.
        let mut app = sample_app();
        app.on_key(Key::Mode(2));
        app.sync_inspect();
        while app.active_tab() != InspectTab::Governance {
            app.on_key(Key::Expand);
        }
        let out = draw(&app);
        assert!(out.contains("identifying"), "{out}");
        assert!(out.contains("tamper-evident"), "sealed posture: {out}");
    }

    /// An injected array view with a 4×4 gradient MIP image for the render tests.
    fn array_view() -> DataView {
        let values: Vec<f64> = (0..16).map(|i| i as f64).collect();
        DataView::Array {
            dtype: "f32".into(),
            codec: "pcodec".into(),
            shape: vec![2, 2, 2],
            unit: None,
            stats: ArrayStats {
                min: 0.0,
                max: 4.0,
                mean: 2.0,
                std: 1.0,
                count: 8,
            },
            rescale: None,
            hist: vec![
                HistBin {
                    lo: 0.0,
                    hi: 2.0,
                    count: 6,
                },
                HistBin {
                    lo: 2.0,
                    hi: 4.0,
                    count: 2,
                },
            ],
            image: Some(crate::data::ImagePlane {
                width: 4,
                height: 4,
                values,
                min: 0.0,
                max: 15.0,
                kind: "MIP z".into(),
            }),
        }
    }

    #[test]
    fn data_pane_array_shows_stats_and_histogram() {
        let mut app = sample_app();
        select_label(&mut app, "pet_suv");
        app.on_key(Key::Mode(3));
        // Inject the loaded view (an in-memory test app has no file to read from).
        app.set_data(array_view());
        let out = draw(&app);
        assert!(out.contains("DATA › pet_suv"), "{out}");
        assert!(out.contains("f32") && out.contains("pcodec"), "{out}");
        assert!(out.contains("histogram") && out.contains("█"), "{out}");
    }

    #[test]
    fn pressing_m_toggles_the_array_image() {
        let mut app = sample_app();
        select_label(&mut app, "pet_suv");
        app.on_key(Key::Mode(3));
        app.set_data(array_view());
        // Toggle to the image sub-view.
        app.on_key(Key::ToggleImage);
        let out = draw(&app);
        assert!(out.contains("MIP z image"), "{out}");
        // The high-intensity end of the gradient maps to bright ramp glyphs.
        assert!(out.contains('#'), "{out}");
        // Toggling back returns to the histogram.
        app.on_key(Key::ToggleImage);
        assert!(draw(&app).contains("histogram"));
    }

    #[test]
    fn data_pane_table_shows_header_rows_and_total() {
        let mut app = sample_app();
        app.on_key(Key::Mode(3));
        app.set_data(DataView::Table {
            columns: vec!["ms".into(), "en".into()],
            rows: (0..5)
                .map(|i| vec![i.to_string(), format!("{i}.5")])
                .collect(),
            total: 100,
        });
        let out = draw(&app);
        assert!(out.contains("ms") && out.contains("en"), "{out}");
        // The footer note reports the visible window against the true total + the loaded-page cap.
        assert!(out.contains("of 100"), "{out}");
        assert!(out.contains("first 5 loaded"), "{out}");
    }

    #[test]
    fn data_pane_without_a_block_guides_the_user() {
        let mut app = sample_app(); // root selected, no block
        app.on_key(Key::Mode(3));
        let out = draw(&app);
        assert!(out.contains("Select a block"), "{out}");
    }

    #[test]
    fn verify_pane_without_a_verdict_shows_the_structural_glance() {
        let mut app = sample_app(); // in-memory, no file → no computed verdict
        app.on_key(Key::Mode(4));
        let out = draw(&app);
        assert!(out.contains("VERIFY"), "{out}");
        assert!(out.contains("seal"));
        // Honest: this is the manifest-only glance, not claimed as a deep verify.
        assert!(out.contains("structural glance"), "{out}");
    }

    #[test]
    fn verify_pane_renders_a_deep_verdict_with_signature_attribution() {
        use tessera_explore::verify::{
            ArtifactVerdict, IntegrityCheck, LineageSummary, SchemaVerdict, SignatureInfo,
        };
        let mut app = sample_app();
        app.on_key(Key::Mode(4));
        app.set_verdict(ArtifactVerdict {
            sealed: true,
            integrity: IntegrityCheck {
                blocks_total: 3,
                blocks_ok: 3,
                failure: None,
            },
            schema: SchemaVerdict::Conformant,
            signature: Some(SignatureInfo {
                alg: "ed25519".into(),
                key_id: "9e2b1a2b3c4d5e6f7890".into(),
                signer: Some("https://orcid.org/0000-0002-1825-0097".into()),
                signed_at: Some("2026-07-03T10:00:00Z".into()),
                key_format: Some("raw-hex".into()),
                trust_checked: false,
            }),
            lineage: LineageSummary {
                edges: 1,
                with_content_hash: 1,
            },
        });
        let out = draw(&app);
        assert!(out.contains("INTACT"), "{out}");
        assert!(out.contains("3/3 blocks"), "{out}");
        assert!(
            out.contains("ed25519") && out.contains("0000-0002-1825-0097"),
            "{out}"
        );
        // Trust is explicitly not claimed here.
        assert!(out.contains("trust: NOT checked"), "{out}");
    }

    #[test]
    fn compare_pane_empty_state_prompts_for_a_target() {
        let mut app = sample_app();
        app.on_key(Key::Mode(5));
        let out = draw(&app);
        assert!(out.contains("COMPARE"), "{out}");
        assert!(out.contains("--compare"), "{out}");
    }

    #[test]
    fn compare_pane_shows_the_changed_rows() {
        use tessera_explore::diff::{DiffRow, DiffStatus, ManifestDiff};
        let mut app = sample_app();
        app.on_key(Key::Mode(5));
        app.set_compare(crate::app::CompareView {
            a_title: "v1.tsra".into(),
            b_title: "v2.tsra".into(),
            diff: ManifestDiff {
                rows: vec![
                    DiffRow {
                        field: "product".into(),
                        a: "recon".into(),
                        b: "recon".into(),
                        status: DiffStatus::Same,
                    },
                    DiffRow {
                        field: "meta:tracer".into(),
                        a: "FDG".into(),
                        b: "FMISO".into(),
                        status: DiffStatus::Changed,
                    },
                ],
            },
        });
        let out = draw(&app);
        assert!(out.contains("COMPARE › v1.tsra vs v2.tsra"), "{out}");
        assert!(out.contains("1 of 2 fields differ"), "{out}");
        // The changed row is shown (A → B); the unchanged `product` row is omitted.
        assert!(
            out.contains("meta:tracer") && out.contains("FDG") && out.contains("FMISO"),
            "{out}"
        );
        assert!(!out.contains("product"), "{out}");
    }
}
