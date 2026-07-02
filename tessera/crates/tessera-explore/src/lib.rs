//! # tessera-explore — the shared, presentation-agnostic view-model
//!
//! Phase 1a of the `.tsra` explorer (see `docs/spikes/tsra-explorer.md` and the UX spike
//! `docs/spikes/tsra-explorer-ux.md`, #286). This crate lifts the **derived-view / compute** layer out
//! of the CLI (`tessera-cli/src/nav.rs`, which today writes formatted text to a `Write` sink) into a
//! shared library that returns **structured data**. Every human-facing consumer — the CLI text
//! formatter, the ratatui TUI, and a future `tsra serve` (HTTP / Arrow / PNG) — is then a thin
//! *renderer* over this one view-model (the SSOT), rather than a re-implementation of "how to read a
//! `.tsra`".
//!
//! Design invariants (the anti-footgun contract from the UX spike):
//! - **Structured returns, never a `Write`/text sink** — callers render; the view-model computes.
//! - **Presentation-agnostic** — no pixel/PNG/HTTP/terminal concerns leak in here.
//! - Built on [`tessera_io`]'s generic `Reader<R: Read + Seek>` (local + `cloud`) — never a
//!   `&Path`-only surface.
//! - Aggregations normalise to an Arrow result contract (added as the table/aggregation views land).
//! - Preserve `tessera-io`'s bounded-memory streaming (ADR-0026).
//!
//! The migration proceeds function-by-function from `nav.rs`; the CLI is re-expressed as a thin text
//! formatter over each extracted view, with the `trycmd` snapshots held unchanged as the
//! behaviour-preserving guarantee.

pub mod array;
