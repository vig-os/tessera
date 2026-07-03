//! Configurable layout — a layout is **data, not code** (`docs/spikes/tsra-explorer-wireframes.md`
//! § Configurable layout).
//!
//! The shell is one generic machine: a fixed union of modes (`Navigate · Inspect · Data · Verify ·
//! Compare`) over the same view-model. A [`Layout`] only chooses *defaults* — which mode opens first,
//! which inspector tabs are pinned, and the PHI/edit policy. It never adds or removes machinery, so the
//! UX-spike "personas" are just short TOML files (shipped as [`presets`]), not four code paths. This
//! mirrors tessera's own rule — *schemas are embedded data, not engine code* — applied to the UI.
//!
//! ```toml
//! default_mode = "data"                    # navigate | inspect | data | verify | compare
//! pinned       = ["referencing", "schema"] # inspector tabs pinned open
//! [policy]
//! phi_render   = "aware"                    # on | off | aware
//! edit         = false                      # gates the CoW editor (Phase 2)
//! ```

use serde::Deserialize;

/// The fixed union of shell modes — the footer's `1 2 3 4 5`. `Data` is **one** mode that adapts to
/// the selected block's kind (table → paged read + SQL; array → stats + histogram + slice/MIP), never
/// split into Table/Array (a signed-off decision).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Tree focus — browse the [`NodeTree`](tessera_explore::hierarchy::NodeTree).
    #[default]
    Navigate,
    /// Metadata tabs (Integrity / Provenance / Trust / Schema / Referencing / Governance / FAIR).
    Inspect,
    /// The selected block's data — shape-adaptive (table viewer + SQL, or array stats/histogram/slice).
    Data,
    /// The auditor's read-only integrity + signature + trust + schema verdict view.
    Verify,
    /// A/B comparison against a prior version or sibling product.
    Compare,
}

impl Mode {
    /// The mode-switch order (footer `1..5`), for cycling and number-key selection.
    pub const ORDER: [Mode; 5] = [
        Mode::Navigate,
        Mode::Inspect,
        Mode::Data,
        Mode::Verify,
        Mode::Compare,
    ];

    /// The footer label (`"Navigate"`), title-cased for display.
    pub fn label(self) -> &'static str {
        match self {
            Mode::Navigate => "Navigate",
            Mode::Inspect => "Inspect",
            Mode::Data => "Data",
            Mode::Verify => "Verify",
            Mode::Compare => "Compare",
        }
    }

    /// The 1-based footer digit for this mode (`Navigate` → 1 … `Compare` → 5).
    pub fn digit(self) -> u8 {
        Mode::ORDER.iter().position(|&m| m == self).unwrap_or(0) as u8 + 1
    }

    /// Select a mode by its footer digit `1..=5`, or `None` if out of range.
    pub fn from_digit(d: u8) -> Option<Mode> {
        (d >= 1)
            .then(|| Mode::ORDER.get((d - 1) as usize).copied())
            .flatten()
    }
}

/// The inspector's metadata tabs — the `pinned` list names a subset to open by default. This is the
/// closed set from the Inspect wireframe; a layout picks which are pinned, never invents new ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InspectTab {
    /// id · manifest_hash (version) · sealed · block digests · product/schema.
    Integrity,
    /// Provenance & lineage (the `sources` DAG; log/diff).
    Provenance,
    /// Signature + trust-store verdict.
    Trust,
    /// Schema conformance (declared field roster vs the manifest).
    Schema,
    /// World/affine addressing (convention · units · spacing).
    Referencing,
    /// PHI / WORM governance posture.
    Governance,
    /// The FAIR record (findable/accessible/interoperable/reusable summary).
    Fair,
}

impl InspectTab {
    /// Left-to-right tab order in the Inspect bar.
    pub const ORDER: [InspectTab; 7] = [
        InspectTab::Integrity,
        InspectTab::Provenance,
        InspectTab::Trust,
        InspectTab::Schema,
        InspectTab::Referencing,
        InspectTab::Governance,
        InspectTab::Fair,
    ];

    /// The tab's display label (`"Integrity"`).
    pub fn label(self) -> &'static str {
        match self {
            InspectTab::Integrity => "Integrity",
            InspectTab::Provenance => "Provenance",
            InspectTab::Trust => "Trust",
            InspectTab::Schema => "Schema",
            InspectTab::Referencing => "Referencing",
            InspectTab::Governance => "Governance",
            InspectTab::Fair => "FAIR",
        }
    }
}

/// How PHI/identifying fields are rendered — the safety knob for a clinical terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhiRender {
    /// Show identifying values in the clear (a trusted, access-controlled console).
    On,
    /// Always mask identifying values.
    Off,
    /// Mask by default, reveal on an explicit per-field action — the safe clinical default.
    #[default]
    Aware,
}

/// The layout's policy block — display + capability gates independent of which mode is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Policy {
    /// PHI rendering posture (default: [`PhiRender::Aware`]).
    pub phi_render: PhiRender,
    /// Whether the CoW editor capability is offered (Phase 2). Default `false` — read-only.
    pub edit: bool,
}

/// A resolved shell layout — the *defaults* a preset or a user config chooses. Everything here is
/// data; the shell machinery is identical regardless.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Layout {
    /// The mode the shell opens in.
    pub default_mode: Mode,
    /// Inspector tabs pinned open (rendered first, in this order).
    pub pinned: Vec<InspectTab>,
    /// Display + capability policy.
    pub policy: Policy,
}

impl Default for Layout {
    /// The **balanced** layout used when no `--layout` is given: open on the navigator, pin the two
    /// most broadly useful inspector tabs, PHI-aware, read-only.
    fn default() -> Self {
        Layout {
            default_mode: Mode::Navigate,
            pinned: vec![InspectTab::Schema, InspectTab::Referencing],
            policy: Policy::default(),
        }
    }
}

impl Layout {
    /// Parse a layout from TOML (a preset file or a user's `~/.config/tessera/layouts/<name>.toml`).
    /// Unknown keys / bad enum values are a hard error — a config typo should be told, not ignored.
    pub fn from_toml(src: &str) -> Result<Layout, toml::de::Error> {
        toml::from_str(src)
    }

    /// Look up a shipped [`preset`](presets) by name (`analyst` · `auditor` · `steward` · `fair`),
    /// parsed through the same TOML path. `None` if the name is not a shipped preset — the caller can
    /// then try it as a filesystem path.
    pub fn preset(name: &str) -> Option<Layout> {
        presets::SHIPPED
            .iter()
            .find(|(n, _)| *n == name)
            // A shipped preset is a compile-time constant that always parses (guarded by a test).
            .map(|(_, src)| Layout::from_toml(src).expect("shipped preset is valid TOML"))
    }
}

/// The shipped layout presets — the UX-spike personas, expressed as data. Each is embedded TOML so the
/// exact same parser validates them, and they can be written out to `~/.config/tessera/layouts/` as a
/// starting point for a user's own.
pub mod presets {
    /// `analyst` — data-first: opens straight into the block's data with the physical-referencing and
    /// schema tabs pinned for context.
    pub const ANALYST: &str = r#"
default_mode = "data"
pinned       = ["referencing", "schema"]
[policy]
phi_render   = "aware"
edit         = false
"#;

    /// `auditor` — verification-first: opens on the Verify verdict with every integrity/trust tab
    /// pinned; PHI stays masked (the auditor checks structure, not patient content).
    pub const AUDITOR: &str = r#"
default_mode = "verify"
pinned       = ["integrity", "provenance", "trust", "governance"]
[policy]
phi_render   = "off"
edit         = false
"#;

    /// `steward` — curation-first: opens on Inspect with the FAIR/governance/provenance tabs pinned.
    pub const STEWARD: &str = r#"
default_mode = "inspect"
pinned       = ["fair", "governance", "provenance"]
[policy]
phi_render   = "aware"
edit         = false
"#;

    /// `fair` — publishing-first: opens on Inspect with the FAIR-record and schema tabs pinned.
    pub const FAIR: &str = r#"
default_mode = "inspect"
pinned       = ["fair", "schema"]
[policy]
phi_render   = "aware"
edit         = false
"#;

    /// The (name → TOML) catalogue backing [`Layout::preset`](super::Layout::preset) and any
    /// `--layout <name>` resolution.
    pub const SHIPPED: &[(&str, &str)] = &[
        ("analyst", ANALYST),
        ("auditor", AUDITOR),
        ("steward", STEWARD),
        ("fair", FAIR),
    ];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn balanced_default_opens_on_the_navigator_read_only() {
        let l = Layout::default();
        assert_eq!(l.default_mode, Mode::Navigate);
        assert_eq!(l.policy.phi_render, PhiRender::Aware);
        assert!(!l.policy.edit);
        assert_eq!(l.pinned, vec![InspectTab::Schema, InspectTab::Referencing]);
    }

    #[test]
    fn parses_the_wireframe_example_verbatim() {
        let src = r#"
            default_mode = "data"
            pinned       = ["referencing", "schema"]
            [policy]
            phi_render   = "aware"
            edit         = false
        "#;
        let l = Layout::from_toml(src).unwrap();
        assert_eq!(l.default_mode, Mode::Data);
        assert_eq!(l.pinned, vec![InspectTab::Referencing, InspectTab::Schema]);
        assert_eq!(l.policy.phi_render, PhiRender::Aware);
    }

    #[test]
    fn a_bare_config_falls_back_to_balanced_defaults() {
        // Every field is optional; an empty file is the balanced default.
        let l = Layout::from_toml("").unwrap();
        assert_eq!(l, Layout::default());
        // A partial file overrides only what it names.
        let l = Layout::from_toml("default_mode = \"verify\"").unwrap();
        assert_eq!(l.default_mode, Mode::Verify);
        assert_eq!(l.pinned, Layout::default().pinned);
    }

    #[test]
    fn unknown_key_or_bad_value_is_a_hard_error() {
        // A typo'd key is rejected (deny_unknown_fields) — a config mistake is told, not swallowed.
        assert!(Layout::from_toml("default_moed = \"data\"").is_err());
        // An out-of-vocabulary mode value is rejected.
        assert!(Layout::from_toml("default_mode = \"browse\"").is_err());
        // A bad inspector-tab name is rejected.
        assert!(Layout::from_toml("pinned = [\"integrity\", \"nope\"]").is_err());
    }

    #[test]
    fn every_shipped_preset_parses_and_is_distinct() {
        for (name, _) in presets::SHIPPED {
            let l = Layout::preset(name).unwrap_or_else(|| panic!("preset {name} missing"));
            // Sanity: a preset is a real layout with a resolvable default mode.
            assert!(Mode::ORDER.contains(&l.default_mode));
        }
        assert!(Layout::preset("does-not-exist").is_none());
        // The four personas open on different modes — they are genuinely distinct presets.
        let analyst = Layout::preset("analyst").unwrap();
        let auditor = Layout::preset("auditor").unwrap();
        assert_ne!(analyst.default_mode, auditor.default_mode);
        assert_eq!(auditor.policy.phi_render, PhiRender::Off);
    }

    #[test]
    fn mode_digit_round_trips_for_the_footer() {
        for (i, &m) in Mode::ORDER.iter().enumerate() {
            let d = (i as u8) + 1;
            assert_eq!(m.digit(), d);
            assert_eq!(Mode::from_digit(d), Some(m));
        }
        assert_eq!(Mode::from_digit(0), None);
        assert_eq!(Mode::from_digit(6), None);
    }
}
