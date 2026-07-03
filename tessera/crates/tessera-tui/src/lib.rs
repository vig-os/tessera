//! # tessera-tui — the terminal `.tsra` explorer
//!
//! A [ratatui] shell that renders the [`tessera_explore`] view-model (the SSOT): the navigator tree,
//! the inspector tabs, the block data pane, and the verify verdict. It is a **thin renderer** — every
//! pane binds to a view-model function the CLI / `serve` / MCP surfaces also render, so the TUI adds no
//! new way to read a `.tsra`, only an interactive way to look at one
//! (`docs/spikes/tsra-explorer-wireframes.md`, #286).
//!
//! Structure — logic and presentation are terminal-free and fully unit/snapshot-tested; only [`run`]
//! touches a TTY:
//! - [`config`] — the config-driven layout ("a layout is data, not code"): modes, inspector tabs,
//!   policy, and the shipped persona presets.
//! - [`app`] — shell state + input handling (navigator selection/collapse, mode switching, the
//!   structural header status) as a pure model driven by [`app::Key`].
//! - [`ui`] — the render function ([`ui::render`]): header verdict strip · navigator · content pane ·
//!   footer, snapshot-tested against a `TestBackend`.
//! - [`run`] — the crossterm event loop + raw-mode/alternate-screen lifecycle (the only non-testable
//!   part; a panic-safe [terminal guard] restores the terminal).
//!
//! [ratatui]: https://ratatui.rs
//! [terminal guard]: run

pub mod app;
pub mod config;
pub mod data;
pub mod run;
pub mod ui;

pub use run::run;
