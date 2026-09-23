//! Ingest-time **provenance stamp** — a non-sealed `aux/provenance.json` member (ADR-0042).
//!
//! Records **when + by what tool + on what host** an ingest produced a `.tsra`, so a downstream
//! reader can answer "who made this, and when did they make it?" *inside the one shareable file*.
//! Because the stamp lives in `aux/` — outside the seal — a wall-clock `ingested_at` does NOT break
//! writer-determinism (the sealed region is byte-identical across re-ingest; only the aux member
//! differs). This is the "why a wall-clock stamp is safe here at all" trick ADR-0042 turns on.
//!
//! ## Test/determinism seam
//! For the tests that DO compare whole-archive bytes (or that need a stable stamp for a doc
//! walkthrough), setting the env var [`ENV_SKIP_PROVENANCE`] (`TESSERA_SKIP_PROVENANCE=1`) turns
//! [`stamp_ingest_provenance`] into a no-op. Alternatively, callers can build [`ProvenanceOptions`]
//! with a fixed [`ProvenanceOptions::ingested_at`] to pin the value.

use std::path::Path;

use serde::{Deserialize, Serialize};

use tessera_core::manifest::TESSERA_VERSION;
use tessera_core::{Error, Result};

use crate::container::{add_aux_members, AuxMember, Reader, AUX_PROVENANCE_ENTRY};

/// Env var name for the test/CI seam: when set (to anything non-empty),
/// [`stamp_ingest_provenance`] is a no-op. Used by the byte-identical-archive tests and by the
/// deterministic docs-as-tests fixtures that would otherwise embed a live wall-clock.
pub const ENV_SKIP_PROVENANCE: &str = "TESSERA_SKIP_PROVENANCE";

/// The shape of the JSON written to `aux/provenance.json` (ADR-0042 §4). Every field is optional at
/// read time — a v1.0 producer might omit `host`, a test might pin `ingested_at`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IngestProvenance {
    /// RFC-3339 UTC timestamp of the ingest (wall-clock at seal). `None` = unstamped (should not
    /// happen in normal ingest; kept optional so a hand-crafted `aux/provenance.json` in an old
    /// archive still parses).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ingested_at: Option<String>,
    /// Tool that produced the archive (`tessera/<version>` by default).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub producer: Option<String>,
    /// Host the ingest ran on (from `HOSTNAME` / `HOST`; absent when the env var isn't set — no
    /// external crate is pulled in for this).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
}

/// Options for [`stamp_ingest_provenance`] — the caller can pin any field for tests / doc examples;
/// unset fields fall back to the live defaults (wall-clock time, `tessera/<TESSERA_VERSION>`,
/// `HOSTNAME` / `HOST`).
#[derive(Debug, Clone, Default)]
pub struct ProvenanceOptions {
    /// Fixed RFC-3339 UTC stamp. `None` = read the current wall clock at call time.
    pub ingested_at: Option<String>,
    /// Fixed producer string. `None` = `tessera/<TESSERA_VERSION>`.
    pub producer: Option<String>,
    /// Fixed host string. `None` = env `HOSTNAME` (fallback `HOST`) if set, else omitted.
    pub host: Option<String>,
}

/// Return the current wall-clock time as an RFC-3339 UTC string. Fails only if the formatter itself
/// errors (should be infallible for RFC-3339 UTC); returns `None` in that case so the stamp is just
/// missing rather than blowing up the ingest.
fn now_rfc3339() -> Option<String> {
    use time::format_description::well_known::Rfc3339;
    time::OffsetDateTime::now_utc().format(&Rfc3339).ok()
}

/// Resolve the host string from the environment (`HOSTNAME` first, `HOST` as a fallback), or
/// `None` when neither is set. Kept env-only per ADR-0042 §5 — no external `hostname` crate.
fn env_host() -> Option<String> {
    std::env::var("HOSTNAME")
        .ok()
        .or_else(|| std::env::var("HOST").ok())
        .filter(|s| !s.is_empty())
}

/// Stamp `aux/provenance.json` on an already-sealed `.tsra` (ADR-0042). No-op when
/// [`ENV_SKIP_PROVENANCE`] is set to any non-empty value (test seam). The sealed region — magic +
/// manifest + every `blocks/<name>` entry — is preserved byte-identical (proven by
/// [`add_aux_members`]'s invariant check).
///
/// Called once per member at the tail of an ingest — after the sealed `.tsra` is on disk at its
/// final path, before the engine records the manifest for its caller. Two producers can call this
/// twice on the same file (the second overwrites the first `aux/provenance.json`), so a re-ingest
/// naturally refreshes the stamp.
pub fn stamp_ingest_provenance(path: &Path, opts: &ProvenanceOptions) -> Result<()> {
    if std::env::var(ENV_SKIP_PROVENANCE)
        .map(|v| !v.is_empty())
        .unwrap_or(false)
    {
        return Ok(());
    }
    let record = IngestProvenance {
        ingested_at: opts.ingested_at.clone().or_else(now_rfc3339),
        producer: opts
            .producer
            .clone()
            .or_else(|| Some(format!("tessera/{TESSERA_VERSION}"))),
        host: opts.host.clone().or_else(env_host),
    };
    let json = serde_json::to_vec_pretty(&record).map_err(|e| Error::Container(e.to_string()))?;
    // AUX_PROVENANCE_ENTRY starts with `aux/`; add_aux_members expects the sub-path only.
    let entry_name = AUX_PROVENANCE_ENTRY
        .strip_prefix("aux/")
        .unwrap_or(AUX_PROVENANCE_ENTRY);
    add_aux_members(path, &[AuxMember::new(entry_name.to_string(), json)])?;
    Ok(())
}

/// Read the embedded `aux/provenance.json` from a `.tsra`, or `Ok(None)` if unstamped. Convenience
/// wrapper for `tree` / `inspect` / bindings that surface the ingest wall-clock without opening
/// the raw aux member themselves.
pub fn read_provenance(path: &Path) -> Result<Option<IngestProvenance>> {
    let mut r = Reader::open(path)?;
    let entry_name = AUX_PROVENANCE_ENTRY
        .strip_prefix("aux/")
        .unwrap_or(AUX_PROVENANCE_ENTRY);
    if !r.aux_names().iter().any(|n| n == entry_name) {
        return Ok(None);
    }
    let bytes = r.read_aux(entry_name)?;
    let rec: IngestProvenance =
        serde_json::from_slice(&bytes).map_err(|e| Error::Container(e.to_string()))?;
    Ok(Some(rec))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::array::{self, ArrayData};
    use crate::container::pack;
    use tessera_core::block::array::ArraySpec;
    use tessera_core::ProductBuilder;

    fn sealed(dir: &Path, name: &str) -> std::path::PathBuf {
        let spec = ArraySpec::new(vec![4, 4, 4], "int16");
        let data = ArrayData::I16((0..64).map(|k| k as i16).collect());
        let (bref, payload) = array::array_block("volume", &spec, &data).unwrap();
        let mut b = ProductBuilder::new("recon", "PROV", "d", "2024-01-01T00:00:00Z");
        b.add_block_ref(bref);
        let sealed = b.seal().unwrap();
        let out = dir.join(name);
        pack(&sealed, &[payload], &out).unwrap();
        out
    }

    #[test]
    fn stamp_writes_provenance_and_preserves_the_seal() {
        // The archival guarantee: stamping a wall-clock provenance record MUST leave
        // content_hash / manifest_hash byte-identical (aux is outside the seal, ADR-0042).
        let dir = tempfile::tempdir().unwrap();
        let path = sealed(dir.path(), "stamp.tsra");
        let (pre_ch, pre_mh) = {
            let r = Reader::open(&path).unwrap();
            (
                r.manifest().content_hash.clone(),
                r.manifest().manifest_hash.clone(),
            )
        };
        // Pin `ingested_at` so the record's contents are testable; live wall-clock is exercised in
        // the "live" test below.
        let opts = ProvenanceOptions {
            ingested_at: Some("2026-06-25T12:34:56Z".into()),
            producer: Some("tessera/test".into()),
            host: Some("unit-test-host".into()),
        };
        stamp_ingest_provenance(&path, &opts).unwrap();

        // Read back the stamped record.
        let got = read_provenance(&path).unwrap().unwrap();
        assert_eq!(got.ingested_at.as_deref(), Some("2026-06-25T12:34:56Z"));
        assert_eq!(got.producer.as_deref(), Some("tessera/test"));
        assert_eq!(got.host.as_deref(), Some("unit-test-host"));

        // The sealed region is byte-identical.
        let r = Reader::open(&path).unwrap();
        assert_eq!(r.manifest().content_hash, pre_ch);
        assert_eq!(r.manifest().manifest_hash, pre_mh);
    }

    #[test]
    fn env_skip_provenance_makes_stamp_a_noop() {
        // The determinism seam: with TESSERA_SKIP_PROVENANCE=1 set, stamp becomes a no-op so a
        // whole-archive byte comparison never sees a wall-clock drift. Non-empty value activates
        // the guard; unset (or empty) leaves the default live-stamp behavior intact.
        let dir = tempfile::tempdir().unwrap();
        let path = sealed(dir.path(), "skip.tsra");
        // Reads the env var at call time — we set it just around the stamp call.
        // SAFETY: unit tests are single-threaded within one test process for env access; this
        // test doesn't spawn threads and restores the previous value at the end.
        let prev = std::env::var(ENV_SKIP_PROVENANCE).ok();
        std::env::set_var(ENV_SKIP_PROVENANCE, "1");
        stamp_ingest_provenance(&path, &ProvenanceOptions::default()).unwrap();
        // Restore or clear so the surrounding process sees its prior value.
        match prev {
            Some(v) => std::env::set_var(ENV_SKIP_PROVENANCE, v),
            None => std::env::remove_var(ENV_SKIP_PROVENANCE),
        }
        // No aux/provenance.json was written.
        assert!(read_provenance(&path).unwrap().is_none());
    }

    #[test]
    fn wall_clock_default_produces_a_stamp() {
        // The default (live) code path: no fixed `ingested_at`, no env skip → the stamp lands with
        // an RFC-3339-shaped `ingested_at`.
        let dir = tempfile::tempdir().unwrap();
        let path = sealed(dir.path(), "live.tsra");
        // Ensure the skip env isn't set from an earlier test run.
        std::env::remove_var(ENV_SKIP_PROVENANCE);
        stamp_ingest_provenance(&path, &ProvenanceOptions::default()).unwrap();
        let got = read_provenance(&path).unwrap().unwrap();
        let ts = got.ingested_at.expect("ingested_at is set on live stamp");
        // shape check: RFC-3339 UTC always contains a `T` between date and time and ends in `Z` or an offset.
        assert!(ts.contains('T'), "not an RFC-3339 stamp: {ts}");
        // The default producer is "tessera/<version>".
        assert_eq!(
            got.producer,
            Some(format!("tessera/{TESSERA_VERSION}")),
            "default producer should be tessera/<version>"
        );
    }
}
