//! Write-engine runtime configuration — the **single source of truth** for the write engine's
//! runtime knobs (RAM ceiling + encode parallelism). The row-group size is a **format invariant**
//! ([`crate::table::ROWS_PER_GROUP`]), NOT a runtime knob; this config is the *only* place threads
//! and the in-flight ring depth are derived.
//!
//! Model:
//! - `ram_budget` is a **RAM ceiling** for in-flight encode work — it bounds the [`StreamWriter`]
//!   ring depth regardless of how fast the producer pushes. Producer outruns encoders → backpressure
//!   blocks `push` once `ring_depth` blocks are queued, so peak memory ≈ `ring_depth × unit_bytes`.
//! - `workers` is the **encode-thread pool** (std threads — never tokio, per ADR-0034). On the
//!   chunked array/block path ([`StreamWriter`]), more workers = more parallel encode = real
//!   core-scaling. On the listmode [`crate::TableStreamWriter`] path it does **not**: that path
//!   encodes the one canonical Vortex block over a sequential stream, single-thread (multi-block
//!   core-scaling for the table path is deferred — ADR-0026).
//! - The row-group size ([`crate::table::ROWS_PER_GROUP`] = 65 536) is a *fixed format invariant*
//!   and intentionally NOT exposed here.
//!
//! Determinism: this config changes RUNTIME behavior (RAM, threads) and never the output bytes —
//! `streamed == batch` and the conformance corpus gates stay green.

use tessera_core::{Error, Result};

use crate::write::WriteSession;

/// One MiB in bytes — used by the suffix parser.
const MIB: u64 = 1024 * 1024;
/// One GiB in bytes — used by the suffix parser.
const GIB: u64 = 1024 * 1024 * 1024;

/// The default RAM ceiling for in-flight encode work — 1 GiB. A sane laptop/server default that
/// keeps the streaming pipeline well-fed without crowding the system; tune via
/// [`WriteConfig::ram_budget`] for boxes with less headroom.
pub const DEFAULT_RAM_BUDGET: u64 = GIB;

/// Runtime config for the streaming write engine. SSoT for `workers` (encode threads) and
/// `ram_budget` (RAM ceiling that bounds the in-flight ring). See module docs for the model.
#[derive(Debug, Clone, Copy)]
pub struct WriteConfig {
    /// Encode-thread pool size. Always ≥ 1 (clamped on construction/mutation).
    workers: usize,
    /// RAM ceiling (bytes) that bounds the in-flight block ring. Always ≥ 1 byte.
    ram_budget: u64,
}

impl WriteConfig {
    /// Machine-derived defaults: `workers = std::thread::available_parallelism()` (fallback 4) and
    /// `ram_budget` = [`DEFAULT_RAM_BUDGET`] (1 GiB).
    pub fn for_system() -> Self {
        let workers = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        WriteConfig {
            workers,
            ram_budget: DEFAULT_RAM_BUDGET,
        }
    }

    /// Set the encode-thread pool size (clamped to ≥ 1).
    pub fn workers(mut self, n: usize) -> Self {
        self.workers = n.max(1);
        self
    }

    /// Set the RAM ceiling in bytes (clamped to ≥ 1, so `ring_depth` is always ≥ 1 on the floor
    /// case of `unit_bytes == ram_budget`).
    pub fn ram_budget(mut self, bytes: u64) -> Self {
        self.ram_budget = bytes.max(1);
        self
    }

    /// The current encode-thread pool size.
    pub fn worker_count(&self) -> usize {
        self.workers
    }

    /// The current RAM ceiling in bytes.
    pub fn ram_budget_bytes(&self) -> u64 {
        self.ram_budget
    }

    /// The in-flight ring depth (in blocks) the [`StreamWriter`] should use for a producer whose
    /// per-block encode-buffer size is `unit_bytes`: `max(1, ram_budget / max(1, unit_bytes))`. The
    /// cast to `usize` is fallible on 32-bit hosts when `ram_budget / unit_bytes` exceeds
    /// `usize::MAX` — we saturate to `usize::MAX` rather than truncate.
    pub fn ring_depth(&self, unit_bytes: u64) -> usize {
        let per = unit_bytes.max(1);
        let depth = (self.ram_budget / per).max(1);
        usize::try_from(depth).unwrap_or(usize::MAX)
    }
}

impl Default for WriteConfig {
    fn default() -> Self {
        Self::for_system()
    }
}

/// Parse a byte-count with optional `K`/`M`/`G`/`T` (KiB/MiB/GiB/TiB) suffix — what the CLI
/// `--ram-budget` flag accepts. Case-insensitive; an optional trailing `B`/`iB` is tolerated. The
/// numeric part may be a decimal (e.g. `1.5G`); the result is rounded down to whole bytes.
///
/// Examples (each returns the same value): `512M`, `512MB`, `512MiB`, `0.5G`, `0.5GB`.
pub fn parse_byte_size(s: &str) -> Result<u64> {
    let raw = s.trim();
    if raw.is_empty() {
        return Err(Error::Invalid("empty byte-size".into()));
    }
    // Trim trailing "iB" / "B" so "512MiB", "512MB", "512M" all parse the same.
    let mut t = raw.to_string();
    let lower = t.to_lowercase();
    if let Some(stripped) = lower.strip_suffix("ib") {
        t.truncate(stripped.len());
    } else if let Some(stripped) = lower.strip_suffix('b') {
        // drop a trailing byte-unit b so a plain 512-byte value loses its b and a mega value keeps
        // only its M; a bare number ending in b is invalid anyway and gets rejected downstream.
        t.truncate(stripped.len());
    }
    let bytes = t.as_bytes();
    let (num_str, mult): (&str, u64) = match bytes.last().copied() {
        Some(c) if c.is_ascii_alphabetic() => {
            let suffix = c.to_ascii_lowercase();
            let mult = match suffix {
                b'k' => 1024,
                b'm' => MIB,
                b'g' => GIB,
                b't' => GIB * 1024,
                _ => {
                    return Err(Error::Invalid(format!(
                        "byte-size '{raw}': unknown suffix '{}'",
                        c as char
                    )))
                }
            };
            (&t[..t.len() - 1], mult)
        }
        _ => (t.as_str(), 1),
    };
    let num_str = num_str.trim();
    if num_str.is_empty() {
        return Err(Error::Invalid(format!("byte-size '{raw}': missing number")));
    }
    let n: f64 = num_str
        .parse()
        .map_err(|e| Error::Invalid(format!("byte-size '{raw}': {e}")))?;
    if !n.is_finite() || n < 0.0 {
        return Err(Error::Invalid(format!(
            "byte-size '{raw}': must be a finite non-negative number"
        )));
    }
    let v = n * (mult as f64);
    if v > (u64::MAX as f64) {
        return Err(Error::Invalid(format!("byte-size '{raw}': overflows u64")));
    }
    Ok(v as u64)
}

impl crate::stream::StreamWriter {
    /// Build a [`StreamWriter`](crate::stream::StreamWriter) from a [`WriteConfig`] and a per-block
    /// `unit_bytes` estimate. Workers come from `config.worker_count()`; the in-flight ring depth
    /// is `config.ring_depth(unit_bytes)` — so the RAM ceiling holds regardless of how fast the
    /// producer pushes.
    pub fn with_config(
        session: WriteSession,
        config: &WriteConfig,
        unit_bytes: u64,
    ) -> crate::stream::StreamWriter {
        let workers = config.worker_count();
        let capacity = config.ring_depth(unit_bytes);
        crate::stream::StreamWriter::new(session, workers, capacity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn for_system_yields_positive_defaults() {
        let cfg = WriteConfig::for_system();
        assert!(cfg.worker_count() >= 1);
        assert_eq!(cfg.ram_budget_bytes(), DEFAULT_RAM_BUDGET);
    }

    #[test]
    fn workers_and_ram_budget_clamp_to_floor() {
        // workers(0) clamps to 1 — a zero-thread pool would deadlock; the engine guarantees ≥ 1.
        let cfg = WriteConfig::for_system().workers(0).ram_budget(0);
        assert_eq!(cfg.worker_count(), 1);
        // ram_budget(0) clamps to 1 — keeps `ring_depth(unit) ≥ 1` even at the absurd floor.
        assert_eq!(cfg.ram_budget_bytes(), 1);
        assert_eq!(cfg.ring_depth(1024), 1);
    }

    #[test]
    fn ring_depth_divides_ram_budget_by_unit() {
        // 1 GiB / 512 KiB = 2048 in-flight blocks
        let cfg = WriteConfig::for_system().ram_budget(GIB);
        assert_eq!(cfg.ring_depth(512 * 1024), 2048);
        // never below 1 even if unit > budget (producer of one huge block must still fit)
        let cfg = WriteConfig::for_system().ram_budget(1024);
        assert_eq!(cfg.ring_depth(8 * 1024 * 1024), 1);
        // unit_bytes == 0 is treated as 1 (defensive; should never happen in practice)
        assert!(cfg.ring_depth(0) >= 1);
    }

    #[test]
    fn parse_byte_size_handles_suffixes() {
        assert_eq!(parse_byte_size("1024").unwrap(), 1024);
        assert_eq!(parse_byte_size("1K").unwrap(), 1024);
        assert_eq!(parse_byte_size("512M").unwrap(), 512 * MIB);
        assert_eq!(parse_byte_size("512MB").unwrap(), 512 * MIB);
        assert_eq!(parse_byte_size("512MiB").unwrap(), 512 * MIB);
        assert_eq!(parse_byte_size("1G").unwrap(), GIB);
        assert_eq!(parse_byte_size("8G").unwrap(), 8 * GIB);
        assert_eq!(parse_byte_size("0.5G").unwrap(), GIB / 2);
    }

    #[test]
    fn parse_byte_size_rejects_bad_input() {
        assert!(parse_byte_size("").is_err());
        assert!(parse_byte_size("abc").is_err());
        assert!(parse_byte_size("12X").is_err());
        assert!(parse_byte_size("-1G").is_err());
    }

    #[test]
    fn with_config_picks_up_workers_and_ring_depth() {
        // 8 MiB ram_budget / 1 MiB unit = 8 in-flight blocks; 3 workers requested.
        let cfg = WriteConfig::for_system().workers(3).ram_budget(8 * MIB);
        assert_eq!(cfg.worker_count(), 3);
        assert_eq!(cfg.ring_depth(MIB), 8);

        // Functional smoke: with_config builds a viable writer over a real WriteSession + seals.
        let dir = tempfile::tempdir().unwrap();
        let stage = dir.path().join("stage");
        let ws = WriteSession::create(&stage, "recon", "p", "d", "2024-01-01T00:00:00Z").unwrap();
        let sw = crate::stream::StreamWriter::with_config(ws, &cfg, MIB);
        assert_eq!(sw.worker_count(), 3, "with_config must wire workers");
        let out = dir.path().join("s.tsra");
        // No blocks pushed → seals an empty product (proves the pipeline shuts down cleanly).
        sw.finish(&out).unwrap();
    }
}
