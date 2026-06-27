//! **WORM** (Write-Once-Read-Many) retention for sealed products (#209). A sealed Tessera product is
//! already immutable by identity (any edit changes its `content_hash`/`id`); WORM adds **retention
//! enforcement** — a product under retention must not be overwritten or deleted until its `retain_until`
//! instant, the regulatory guarantee for clinical/archival data (e.g. an N-year hold).
//!
//! This module is the **format-side contract**: a [`RetentionPolicy`] (retain-until + mode) plus a
//! [`guard_mutation`] check the storage layer calls before any overwrite/delete. The *enforcement
//! substrate* is the backend: object-lock on S3/MinIO (the canonical WORM store) honours the same policy
//! at the bucket; a local filesystem store uses this guard directly. The policy is **store-don't-compute
//! metadata** — recorded beside the product, not hashed into identity (retention is a storage attribute,
//! not part of *what the product is*).
//!
//! Two modes mirror S3 Object-Lock: **compliance** (no one — not even the root account — may shorten or
//! bypass the hold) and **governance** (a specially-authorized caller may bypass). Both refuse mutation
//! while retained; only governance honours an explicit authorized `bypass`.

use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use tessera_core::{Error, Result};

/// The retention enforcement mode (mirrors S3 Object-Lock).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RetentionMode {
    /// No one may overwrite/delete or shorten the hold until `retain_until` — not even an authorized
    /// bypass. The strict regulatory mode.
    Compliance,
    /// Refuses mutation while retained, but a specially-authorized caller may bypass (an audited escape
    /// hatch for legal-hold corrections).
    Governance,
}

/// A WORM retention policy on a product: hold it immutable until `retain_until` under `mode`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionPolicy {
    /// The instant the hold expires — **RFC 3339** (e.g. `"2030-01-01T00:00:00Z"`).
    pub retain_until: String,
    pub mode: RetentionMode,
}

impl RetentionPolicy {
    /// Construct a compliance-mode hold until `retain_until` (RFC 3339).
    pub fn compliance(retain_until: impl Into<String>) -> Self {
        RetentionPolicy {
            retain_until: retain_until.into(),
            mode: RetentionMode::Compliance,
        }
    }

    /// Construct a governance-mode hold until `retain_until` (RFC 3339).
    pub fn governance(retain_until: impl Into<String>) -> Self {
        RetentionPolicy {
            retain_until: retain_until.into(),
            mode: RetentionMode::Governance,
        }
    }

    /// Whether the product is still under retention at `now` (RFC 3339): `now < retain_until`. Errors on
    /// an unparseable timestamp (a malformed policy must fail closed, not silently allow mutation).
    pub fn is_retained(&self, now: &str) -> Result<bool> {
        let until = OffsetDateTime::parse(&self.retain_until, &Rfc3339)
            .map_err(|e| Error::Invalid(format!("WORM retain_until not RFC 3339: {e}")))?;
        let now = OffsetDateTime::parse(now, &Rfc3339)
            .map_err(|e| Error::Invalid(format!("WORM 'now' not RFC 3339: {e}")))?;
        Ok(now < until)
    }
}

/// Guard a **mutation** (overwrite or delete) of a retained product. Returns `Ok(())` if the mutation is
/// permitted; an `Invalid` error (refuses) while the product is under retention. `governance_bypass` is
/// honoured **only** in [`RetentionMode::Governance`] — a compliance hold can never be bypassed. The
/// storage layer calls this before writing over / deleting a product's bytes.
pub fn guard_mutation(policy: &RetentionPolicy, now: &str, governance_bypass: bool) -> Result<()> {
    if policy.is_retained(now)? {
        let bypassed = governance_bypass && policy.mode == RetentionMode::Governance;
        if !bypassed {
            tracing::warn!(
                target: "tessera::worm",
                mode = ?policy.mode,
                retain_until = %policy.retain_until,
                now = %now,
                "refused mutation of a product under WORM retention"
            );
            return Err(Error::Invalid(format!(
                "WORM: product is under {:?} retention until {} (now {now})",
                policy.mode, policy.retain_until
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const BEFORE: &str = "2025-01-01T00:00:00Z";
    const AFTER: &str = "2031-01-01T00:00:00Z";
    const UNTIL: &str = "2030-01-01T00:00:00Z";

    #[test]
    fn compliance_hold_refuses_all_mutation_until_expiry() {
        let p = RetentionPolicy::compliance(UNTIL);
        // under retention → refused, even with a bypass attempt.
        assert!(guard_mutation(&p, BEFORE, false).is_err());
        assert!(
            guard_mutation(&p, BEFORE, true).is_err(),
            "compliance can NEVER be bypassed"
        );
        // after expiry → permitted.
        assert!(guard_mutation(&p, AFTER, false).is_ok());
    }

    #[test]
    fn governance_hold_refuses_unless_authorized_bypass() {
        let p = RetentionPolicy::governance(UNTIL);
        assert!(guard_mutation(&p, BEFORE, false).is_err()); // no bypass → refused
        assert!(guard_mutation(&p, BEFORE, true).is_ok()); // authorized bypass → allowed
        assert!(guard_mutation(&p, AFTER, false).is_ok()); // expired → allowed
    }

    #[test]
    fn is_retained_compares_instants_and_fails_closed_on_bad_timestamp() {
        let p = RetentionPolicy::compliance(UNTIL);
        assert!(p.is_retained(BEFORE).unwrap());
        assert!(!p.is_retained(AFTER).unwrap());
        // a malformed policy/now must error (fail closed), not silently permit.
        assert!(RetentionPolicy::compliance("not-a-date")
            .is_retained(BEFORE)
            .is_err());
        assert!(p.is_retained("not-a-date").is_err());
    }

    #[test]
    fn policy_serializes_with_lowercase_mode() {
        let j = serde_json::to_value(RetentionPolicy::compliance(UNTIL)).unwrap();
        assert_eq!(j["mode"], "compliance");
        assert_eq!(j["retain_until"], UNTIL);
        let back: RetentionPolicy = serde_json::from_value(j).unwrap();
        assert_eq!(back, RetentionPolicy::compliance(UNTIL));
    }
}
