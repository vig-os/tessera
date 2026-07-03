//! # tessera-tui — the terminal `.tsra` explorer
//!
//! A [ratatui] shell that renders the [`tessera_explore`] view-model (the SSOT): the navigator tree,
//! the inspector tabs, the block data pane, and the verify verdict. It is a **thin renderer** — every
//! pane binds to a view-model function the CLI / `serve` / MCP surfaces also render, so the TUI adds no
//! new way to read a `.tsra`, only an interactive way to look at one
//! (`docs/spikes/tsra-explorer-wireframes.md`, #286).
//!
//! Built bottom-up, most-testable-first. This module currently ships the [`config`] layer — the
//! config-driven layout ("a layout is data, not code"): the mode/tab/policy model and the shipped
//! persona presets, all unit-tested without a terminal. The interactive shell (app state, render, event
//! loop) lands on top of it.
//!
//! [ratatui]: https://ratatui.rs

pub mod config;
