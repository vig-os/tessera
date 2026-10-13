//! Operator-supplied **column semantics** for a generic ingest (ADR-0056 §7) — the `--column-meta`
//! door, applied **inside the seal**.
//!
//! # The semantic-nakedness problem this solves
//!
//! A Parquet file has column names and dtypes and nothing else: no descriptions, no units, no
//! vocabularies, no PHI tiers — while `FieldSpec` demands exactly those, and that demand *is* the
//! FAIR-Reusable pitch. A generically-ingested table that leaves them empty is a Parquet file with a
//! seal on it.
//!
//! The mechanism already existed and just needed generalising: `Column` carries `short_name`,
//! `description`, `unit` and `scale`, and the GE-HDF5 path already populates them from an embedded
//! TOML dictionary. ADR-0056 §7 requires **both** doors ship together:
//!
//! - **at ingest** — this module, landing inside the seal;
//! - **after the fact** — the existing content-addressed metadata edit (`tessera commit --set …`),
//!   one new object, lineage preserved.
//!
//! Shipping only the second is the failure mode that turns generic ingest into a wrapper, which is
//! why the first is not deferred.
//!
//! # File shape
//!
//! One TOML table per column, keyed by the column's storage name (after any struct flattening, so a
//! nested source column is addressed by its dotted name):
//!
//! ```toml
//! [energy]
//! short_name  = "Photon energy"
//! description = "Calibrated per-photon energy, detector-corrected"
//! unit        = "keV"
//! scale       = 0.1          # physical = raw × scale
//!
//! ["patient.mrn"]
//! description = "Site-issued pseudonym, not the raw MRN"
//! sensitivity = "identifying"
//! ```
//!
//! An entry naming a column the table does not have is a **hard error**, not a no-op. A typo in a
//! `--column-meta` file is the case where silence is worst: the operator believes they classified
//! `patient_id` and the artifact seals it as `unknown`, which is precisely the false-confidence
//! outcome §7's `Unknown` tier exists to prevent.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use tessera_core::block::table::Column;
use tessera_core::schema::Sensitivity;
use tessera_core::{Error, Result};

fn he(e: impl std::fmt::Display) -> Error {
    Error::Invalid(format!("ingest: --column-meta: {e}"))
}

/// One column's operator-declared semantics. Every field optional — an operator annotating only the
/// units should not have to restate a description they do not have.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ColumnAnnotation {
    /// Human-facing short label, distinct from the rename-safe storage `name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub short_name: Option<String>,
    /// Human + AI-readable description of what the column means (FAIR I1/I2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// UCUM physical unit of the values, after `scale`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Fixed-point scale: physical value = raw × scale.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<f64>,
    /// PHI tier (ADR-0040 §1 / ADR-0056 §6.3). Omitting it leaves the ingest-stamped
    /// [`Sensitivity::Unknown`] in place — an operator who annotates units but not tiers has not
    /// thereby asserted the column is public.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sensitivity: Option<Sensitivity>,
}

/// A parsed `--column-meta` file: column name → annotation.
///
/// `BTreeMap` so the apply order is deterministic; the result is sealed, so nothing here may depend
/// on hash iteration order.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ColumnMeta(pub BTreeMap<String, ColumnAnnotation>);

impl ColumnMeta {
    /// No annotations — what a generic ingest uses when the operator supplied no file.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Read + parse a `--column-meta` TOML file.
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| he(format!("read {}: {e}", path.display())))?;
        Self::parse(&text)
    }

    /// Parse from a TOML string — the filesystem-free seam the tests drive.
    pub fn parse(text: &str) -> Result<Self> {
        toml::from_str(text).map_err(|e| he(format!("parse: {e}")))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Apply the annotations to a table's columns, in place.
    ///
    /// Errors if an entry names a column that is not present, listing what *is* — see the module docs
    /// for why that is not a warning.
    pub fn apply(&self, columns: &mut [Column]) -> Result<()> {
        for (name, ann) in &self.0 {
            let Some(col) = columns.iter_mut().find(|c| &c.name == name) else {
                let available: Vec<&str> = columns.iter().map(|c| c.name.as_str()).collect();
                return Err(he(format!(
                    "no column named '{name}' in this table (a typo here would silently leave the \
                     column unannotated, so it is an error)\n  columns: {}",
                    available.join(" · ")
                )));
            };
            if let Some(v) = &ann.short_name {
                col.short_name = Some(v.clone());
            }
            if let Some(v) = &ann.description {
                col.description = Some(v.clone());
            }
            if let Some(v) = &ann.unit {
                col.unit = Some(v.clone());
            }
            if let Some(v) = ann.scale {
                col.scale = Some(v);
            }
            if let Some(v) = ann.sensitivity {
                col.sensitivity = v;
            }
        }
        Ok(())
    }

    /// Column names this file classifies at a real tier — i.e. anything it took off
    /// [`Sensitivity::Unknown`]. Used to decide whether the §7 suspect-name advisory still applies.
    pub fn classified(&self) -> Vec<&str> {
        self.0
            .iter()
            .filter(|(_, a)| a.sensitivity.is_some_and(|s| s != Sensitivity::Unknown))
            .map(|(n, _)| n.as_str())
            .collect()
    }
}

/// Split a column name into lowercase word tokens, honouring both separators and camelCase.
///
/// Needed because the real-world spellings of the same field are `PatientID`, `patient_id`,
/// `pat-id`, `SOPInstanceUID` and `mrn_hash`, and a bare substring search over the squashed name is
/// wrong in the direction that matters: `uid` is a substring of `fluid_volume` and `uidx_offset`, and
/// an advisory that fires on ordinary science columns is an advisory people learn to ignore.
///
/// The camel rule is the standard one — break before an uppercase letter that follows a
/// lowercase/digit, and before the last uppercase of an acronym run that is followed by a lowercase —
/// so `SOPInstanceUID` yields `[sop, instance, uid]` and `uidx` stays one token.
fn name_tokens(name: &str) -> Vec<String> {
    let chars: Vec<char> = name.chars().collect();
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if !c.is_ascii_alphanumeric() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            continue;
        }
        let prev = i.checked_sub(1).and_then(|j| chars.get(j)).copied();
        let next = chars.get(i + 1).copied();
        let starts_word = c.is_ascii_uppercase()
            && (prev.is_some_and(|p| p.is_ascii_lowercase() || p.is_ascii_digit())
                || (prev.is_some_and(|p| p.is_ascii_uppercase())
                    && next.is_some_and(|n| n.is_ascii_lowercase())));
        if starts_word && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        cur.push(c.to_ascii_lowercase());
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Does this column name look like a direct identifier?
///
/// ADR-0056 §7's advisory list: MRN / patient id / name / DOB / accession / UID. Two matchers, because
/// the needles are not all equally safe:
///
/// - **Exact token** for the short, ambiguous ones (`mrn`, `dob`, `ssn`, `uid`) — `uid` as a substring
///   also matches `fluid` and `uidx`, so it must be a whole word.
/// - **Substring over the concatenated tokens** for the long, unambiguous ones (`patientid`,
///   `dateofbirth`, `medicalrecord`, …) — these survive any separator convention and cannot collide
///   with a real measurement name.
///
/// This **only ever prints** (once, to stderr, and never when stdout is not a TTY — ADR-0056 §9):
/// someone running `find … -exec tessera ingest …` over 5000 files must not scroll 30k lines of
/// advice, and an advisory never gates an ingest. It is deliberately a *name* heuristic and not a
/// value scan: reading values to guess identifiability would be both slow and a semantic decision,
/// which §1 forbids making silently.
pub fn looks_identifying(name: &str) -> bool {
    /// Short + ambiguous ⇒ must be a whole token.
    const TOKENS: &[&str] = &["mrn", "dob", "ssn", "uid", "accession", "pid"];
    /// Long + unambiguous ⇒ safe as a substring of the joined tokens.
    const PHRASES: &[&str] = &[
        "patientid",
        "patientname",
        "patname",
        "birthdate",
        "dateofbirth",
        "medicalrecord",
        "nhsnumber",
        "socialsecurity",
    ];
    let tokens = name_tokens(name);
    if tokens.iter().any(|t| TOKENS.contains(&t.as_str())) {
        return true;
    }
    let joined: String = tokens.concat();
    PHRASES.iter().any(|p| joined.contains(p))
}

/// ADR-0056 §7's suspect-column advisory: print **once**, to stderr, with the fix.
///
/// Never gates the ingest, and deliberately quiet when there is nothing an operator could act on.
/// §9's loudness rule is why this is one aggregated line per product rather than one per column:
/// someone running `find … -exec tessera ingest …` across 5000 files must not scroll 30k lines of
/// advice. It goes through the `tracing` facade, so a non-TTY consumer can filter it out entirely.
///
/// Takes column **names** rather than a decoded table so the bounded-memory streaming path can call
/// it too (#458): streaming never holds a whole `CanonicalTable`, and a PHI advisory that fired on
/// only one of two paths to the same product would be worse than none — an operator would learn to
/// trust its silence.
pub fn warn_unclassified_identifying(names: &[&str], column_meta: &ColumnMeta, product: &str) {
    let classified = column_meta.classified();
    let suspect: Vec<&str> = names
        .iter()
        .copied()
        .filter(|n| looks_identifying(n))
        .filter(|n| !classified.contains(n))
        .collect();
    if suspect.is_empty() {
        return;
    }
    tracing::warn!(
        target: "tessera::ingest::phi",
        member = %product,
        columns = %suspect.join(", "),
        "column(s) '{}' match an identifying-name pattern (MRN / patient id / name / DOB / \
         accession / UID) and no --column-meta gave them a tier; stamped: unknown. Classify before \
         sharing: add a [<column>] sensitivity = \"identifying\" entry to a --column-meta file, or \
         edit after the fact with `tessera commit --set`.",
        suspect.join("', '")
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cols() -> Vec<Column> {
        vec![
            Column::new("energy", "f8").with_sensitivity(Sensitivity::Unknown),
            Column::new("patient.mrn", "str").with_sensitivity(Sensitivity::Unknown),
            Column::new("count", "u4").with_sensitivity(Sensitivity::Unknown),
        ]
    }

    #[test]
    fn annotations_land_on_the_named_columns_only() {
        let meta = ColumnMeta::parse(
            r#"
            [energy]
            short_name = "Photon energy"
            description = "Calibrated per-photon energy"
            unit = "keV"
            scale = 0.1

            ["patient.mrn"]
            description = "Site-issued pseudonym"
            sensitivity = "identifying"
            "#,
        )
        .unwrap();
        let mut c = cols();
        meta.apply(&mut c).unwrap();

        assert_eq!(c[0].short_name.as_deref(), Some("Photon energy"));
        assert_eq!(c[0].unit.as_deref(), Some("keV"));
        assert_eq!(c[0].scale, Some(0.1));
        // Annotating units did NOT silently downgrade the tier to Public.
        assert_eq!(c[0].sensitivity, Sensitivity::Unknown);

        assert_eq!(c[1].sensitivity, Sensitivity::Identifying);
        assert_eq!(c[1].description.as_deref(), Some("Site-issued pseudonym"));

        // An unmentioned column is untouched.
        assert_eq!(c[2].sensitivity, Sensitivity::Unknown);
        assert!(c[2].description.is_none());
    }

    #[test]
    fn an_entry_for_a_missing_column_is_an_error_that_lists_the_real_ones() {
        let meta = ColumnMeta::parse("[patient_id]\nsensitivity = \"identifying\"\n").unwrap();
        let err = meta.apply(&mut cols()).unwrap_err().to_string();
        assert!(err.contains("no column named 'patient_id'"), "got {err}");
        assert!(
            err.contains("energy") && err.contains("patient.mrn"),
            "got {err}"
        );
    }

    #[test]
    fn an_unknown_annotation_key_is_rejected_rather_than_ignored() {
        // `deny_unknown_fields` is what turns a mistyped key into an error instead of a silent no-op.
        // The failure it prevents: an operator writes `tier = "identifying"`, believes the column is
        // classified, and the artifact seals it as `unknown`. Serde must name the key it rejected.
        let err = ColumnMeta::parse("[energy]\ntier = \"identifying\"\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains("tier"), "the error names the bad key: {err}");
        assert!(err.contains("unknown field"), "got {err}");
    }

    #[test]
    fn classified_lists_only_real_tiers() {
        let meta = ColumnMeta::parse(
            r#"
            [a]
            sensitivity = "identifying"
            [b]
            unit = "mm"
            [c]
            sensitivity = "unknown"
            "#,
        )
        .unwrap();
        assert_eq!(meta.classified(), vec!["a"]);
    }

    #[test]
    fn camel_and_separator_spellings_tokenise_the_same_way() {
        assert_eq!(name_tokens("patient_id"), ["patient", "id"]);
        assert_eq!(name_tokens("PatientID"), ["patient", "id"]);
        assert_eq!(name_tokens("patient-id"), ["patient", "id"]);
        assert_eq!(name_tokens("SOPInstanceUID"), ["sop", "instance", "uid"]);
        assert_eq!(name_tokens("energy_keV"), ["energy", "ke", "v"]);
        // An acronym run with no trailing lowercase stays whole, which is what keeps `uidx` out of
        // the `uid` bucket.
        assert_eq!(name_tokens("uidx_offset"), ["uidx", "offset"]);
    }

    #[test]
    fn the_identifying_name_heuristic_covers_the_real_spellings() {
        for hit in [
            "MRN",
            "mrn_hash",
            "PatientID",
            "patient_id",
            "patient-id",
            "PatientName",
            "pat_name",
            "dob",
            "DateOfBirth",
            "birth_date",
            "AccessionNumber",
            "StudyInstanceUID",
            "SOPInstanceUID",
            "ssn",
        ] {
            assert!(looks_identifying(hit), "'{hit}' should be flagged");
        }
        // Deliberately NOT flagged — §9 lists dtype width, nulls and file size as non-signals, and a
        // heuristic that fires on ordinary science columns trains people to ignore it. The three
        // `uid`-substring names are the ones a naive squashed-substring match gets wrong.
        for miss in [
            "energy",
            "count",
            "x",
            "timestamp",
            "duration_ms",
            "uidx_offset_kev",
            "fluid_volume",
            "liquid_fraction",
            "accessions_total_typo_guard",
        ] {
            assert!(
                !looks_identifying(miss),
                "'{miss}' must not be flagged — a noisy advisory is an ignored advisory"
            );
        }
    }
}
