//! Product-schema registry + the fd5 field model (issues #199, #200).
//!
//! The engine is **schema-driven and domain-agnostic**: product schemas are *embedded data*
//! (these structs), not engine code. A [`ProductSchema`] declares the blocks and metadata
//! fields a product of that kind must carry; [`SchemaRegistry::validate`] checks a manifest
//! against its declared schema. Unknown products are permitted (open-world) so custom domains
//! work without forking the engine; a product that *claims* a known schema must satisfy it.
//!
//! ## The fd5 field model (carried once, in the schema — never per value)
//! Each [`FieldSpec`] pairs a short **stable id** (the storage key + rename-safe evolution
//! anchor, Iceberg-style) with a human/AI-readable **description**, a **dtype**, an optional
//! **unit** (UCUM), an optional controlled **vocabulary**, a **default**, and whether it is
//! **required**. Coded values use the fd5 [`Coded`] (`_vocabulary`/`_code`) convention.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::block::array::ArraySpec;
use crate::block::table::TableSpec;
use crate::block::BlockKind;
use crate::manifest::Manifest;

pub mod contract;
pub use contract::{
    ArrayContract, ColumnReference, ContractViolation, ReferenceScope, ReferenceTarget,
    TableContract, TableOrder,
};

/// Schema-driven **sensitivity tier** for a metadata field (ADR-0040 §1 — the spike that
/// teaches the engine to *reason* about PHI without yet redacting/encrypting). Four tiers on an
/// increasing-identifiability ladder, plus one off-ladder tier for *unclassified*:
/// - [`Public`](Sensitivity::Public) — safe in clear (e.g. modality vocab, calibration
///   coefficients, scan geometry); ships unchanged.
/// - [`Coded`](Sensitivity::Coded) — a controlled-vocab code, intrinsically non-identifying but
///   carrying clinical meaning (e.g. DICOM `Modality` = `CT`).
/// - [`Sensitive`](Sensitivity::Sensitive) — clinical content that is not directly identifying
///   on its own but is access-controlled (e.g. impression text); the future field-redactor's
///   default-keep-in-clear tier.
/// - [`Identifying`](Sensitivity::Identifying) — **direct PHI** (the seed list is DICOM
///   PS3.15's confidentiality profile — Patient Name, Patient ID, MRN, Birth Date, …,
///   plus UIDs that bind to the patient/study). The redactor / field-encryption phases
///   (deferred) operate on the fields the schema marks at this tier.
/// - [`Unknown`](Sensitivity::Unknown) — **not on the ladder: nobody has classified this yet.**
///   Added for generic ingest (ADR-0056 §7). A Parquet/CSV a scientist hands us carries column
///   names and dtypes but no tiers, and the vendor paths' two defences are both absent: GE
///   listmode never sees PHI (scan events), and DICOM classifies at the door (PS3.15). Stamping
///   [`Public`](Sensitivity::Public) there would seal the assertion "safe in clear" over a
///   column that might be a patient identifier — and a seal is forever. `Unknown` says the true
///   thing instead, and gives `verify --require-classified` something to gate on.
///
/// Pure data (`Copy` + serde) — the wasm-targeted `tessera-core` stays host-free; no PHI logic
/// lives in this enum, only the classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sensitivity {
    /// Safe in clear — no clinical or identifying content.
    #[default]
    Public,
    /// A controlled-vocabulary code (e.g. DICOM `Modality`). Not identifying.
    Coded,
    /// Clinical content, not directly identifying on its own (access-controlled but not a name).
    Sensitive,
    /// Direct PHI — DICOM PS3.15 confidentiality-profile fields (Patient Name/ID/MRN, DOB,
    /// linking UIDs). The redact / field-encryption phases (deferred) operate on this tier.
    Identifying,
    /// **Unclassified** — no classification pass has run on this field/column (ADR-0056 §7).
    /// Off the ladder rather than at the bottom of it: it is not a claim of safety, it is the
    /// absence of a claim. What generic ingest stamps on every column no `--column-meta`
    /// classified. Deliberately NOT the serde default (that stays
    /// [`Public`](Sensitivity::Public)), so every schema written before this variant existed
    /// deserialises byte-identically — a producer opts into honesty explicitly.
    Unknown,
}

/// A field's self-description — carried once, in the schema, so values stay lean and a reader
/// (or an AI) always has the field's meaning, unit, and dtype without external context (FAIR I1/I2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldSpec {
    /// Short, stable storage id (the key). Renaming the human label never changes this — it is
    /// the rename-safe evolution anchor.
    pub id: String,
    /// Human + AI-readable description.
    pub description: String,
    /// Value dtype: a [`crate::dtype::DType`] name, or `"string"` / `"coded"` / `"json"` for metadata.
    pub dtype: String,
    /// UCUM physical unit, if any (e.g. "HU", "Bq/mL", "ps", "keV", "mm").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Controlled vocabulary the value is drawn from (e.g. "DICOM", "RadLex", "SNOMED", "UCUM").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vocabulary: Option<String>,
    /// Default value used when the field is absent (satisfies a `required` field).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
    /// Whether a valid product of the owning schema **must** carry this field — absence is a hard
    /// **block** at ingest ([`SchemaRegistry::validate`]).
    #[serde(default)]
    pub required: bool,
    /// Field severity below `required`: a **recommended** field's absence is a non-fatal **warn**
    /// ([`SchemaRegistry::missing_recommended`]) — FAIR-completeness nudge, never a block. Mutually
    /// meaningful with `required` (a `required` field is implicitly more than recommended; this flags
    /// the warn tier). Composes through `imaging_base` like every other field.
    #[serde(default)]
    pub recommended: bool,
    /// Schema-driven PHI classification (ADR-0040 §1). The engine uses this tier to *reason* about
    /// identifiability — e.g. the ingest warn surfaces an `identifying` field present in
    /// metadata in the clear, the hook the future field-encryption / redact phases replace.
    /// `#[serde(default)]` keeps existing on-disk schemas back-compat (absent ⇒ `Public`).
    #[serde(default)]
    pub sensitivity: Sensitivity,
    /// Whether this field is **inheritable identity** (ADR-0058 §5): copied from a `derived_from`
    /// parent into a child product's metadata at seal (unless the child overrides). This is the
    /// *inheritance* axis — orthogonal to [`sensitivity`](Self::sensitivity) (the PHI axis), and an
    /// inherited field carries its tier with it. `skip_serializing_if` keeps the default (`false`)
    /// out of the embedded schema, so existing sealed products' bytes — and seals — are unchanged.
    #[serde(default, skip_serializing_if = "is_false")]
    pub inherit: bool,
}

/// `skip_serializing_if` predicate for a `bool` that defaults to `false`: omit it when false so
/// adding the field leaves existing sealed manifests' embedded-schema bytes (and seals) untouched.
fn is_false(b: &bool) -> bool {
    !*b
}

impl FieldSpec {
    /// The JSON keys this type models (ADR-0060 §2.2 newer-field walk); kept equal to a fully
    /// populated instance's keys by a test.
    pub(crate) const FIELDS: &'static [&'static str] = &[
        "id",
        "description",
        "dtype",
        "unit",
        "vocabulary",
        "default",
        "required",
        "recommended",
        "sensitivity",
        "inherit",
    ];

    /// A required, undimensioned metadata field (absent ⇒ ingest **blocks**). Defaults to
    /// [`Sensitivity::Public`]; chain [`Self::with_sensitivity`] to classify PHI.
    pub fn required(id: &str, description: &str, dtype: &str) -> Self {
        FieldSpec {
            id: id.into(),
            description: description.into(),
            dtype: dtype.into(),
            unit: None,
            vocabulary: None,
            default: None,
            required: true,
            recommended: false,
            sensitivity: Sensitivity::Public,
            inherit: false,
        }
    }

    /// An optional metadata field (absent ⇒ silent). Defaults to [`Sensitivity::Public`].
    pub fn optional(id: &str, description: &str, dtype: &str) -> Self {
        FieldSpec {
            required: false,
            ..FieldSpec::required(id, description, dtype)
        }
    }

    /// A **recommended** metadata field: absent ⇒ a non-fatal warn (FAIR nudge), never a block. The
    /// middle tier between `required` (block) and `optional` (silent).
    pub fn recommended(id: &str, description: &str, dtype: &str) -> Self {
        FieldSpec {
            required: false,
            recommended: true,
            ..FieldSpec::required(id, description, dtype)
        }
    }

    /// Builder: attach a UCUM unit.
    pub fn unit(mut self, unit: &str) -> Self {
        self.unit = Some(unit.into());
        self
    }

    /// Builder: attach a controlled vocabulary.
    pub fn vocabulary(mut self, vocab: &str) -> Self {
        self.vocabulary = Some(vocab.into());
        self
    }

    /// Builder: classify the field's PHI **sensitivity tier** (ADR-0040 §1). Defaults to
    /// [`Sensitivity::Public`]; mark direct PHI as [`Sensitivity::Identifying`] so the engine's
    /// schema-driven PHI reasoning (today: ingest warn; future: redact / field encryption) picks
    /// it up automatically.
    pub fn with_sensitivity(mut self, s: Sensitivity) -> Self {
        self.sensitivity = s;
        self
    }

    /// Builder: mark this field **inheritable identity** (ADR-0058 §5) — it flows from a
    /// `derived_from` parent into a child product at seal unless the child overrides it.
    pub fn inheritable(mut self) -> Self {
        self.inherit = true;
        self
    }
}

/// A coded value drawn from a controlled vocabulary (fd5 `_vocabulary`/`_code`). e.g. a CT
/// modality `{ _vocabulary: "DICOM", _code: "CT", label: "Computed Tomography" }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Coded {
    #[serde(rename = "_vocabulary")]
    pub vocabulary: String,
    #[serde(rename = "_code")]
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl Coded {
    /// The JSON keys this type models (ADR-0060 §2.2 newer-field walk); kept equal to a fully
    /// populated instance's keys by a test.
    pub(crate) const FIELDS: &'static [&'static str] = &["_vocabulary", "_code", "label"];

    pub fn new(vocabulary: &str, code: &str) -> Self {
        Coded {
            vocabulary: vocabulary.into(),
            code: code.into(),
            label: None,
        }
    }
}

/// A block a schema expects: a logical `role`, the required shape (`kind`, `None` = either), and
/// how many are required (`0` = optional).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlockRequirement {
    pub role: String,
    /// Required block shape; `None` accepts either array or table.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<BlockKind>,
    /// Minimum number of matching blocks (0 = optional).
    pub min_count: u32,
    pub description: String,
}

impl BlockRequirement {
    /// The JSON keys this type models (ADR-0060 §2.2 newer-field walk); kept equal to a fully
    /// populated instance's keys by a test.
    pub(crate) const FIELDS: &'static [&'static str] =
        &["role", "kind", "min_count", "description"];

    fn matches(&self, b: &crate::block::BlockRef) -> bool {
        self.kind.is_none_or(|k| k == b.kind)
    }
}

/// One product schema — the contract for a `product` kind. Versioned for additive evolution
/// (adding optional fields/blocks bumps the minor; ids never change).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProductSchema {
    pub product: String,
    pub version: String,
    pub description: String,
    #[serde(default)]
    pub fields: Vec<FieldSpec>,
    #[serde(default)]
    pub blocks: Vec<BlockRequirement>,
    /// Whether a product of this schema **must** carry a generation record (ADR-0058 §3): the
    /// schema-declared "this product needs a recipe" rule. `true` on schemas for computed products
    /// (a DAQ/SIM/recon output); absence at validate is a hard block. Default `false` (permissive,
    /// back-compat) with `skip_serializing_if` so existing embedded schemas' bytes are unchanged —
    /// the policy lives in the schema, not the engine (a domain opts in per product kind).
    #[serde(default, skip_serializing_if = "is_false")]
    pub requires_generation: bool,
    /// The **column contract** per table role (ADR-0065 §8, #809): ordered columns, sequence key,
    /// row order, keys and references. A caller checks a block against it with
    /// [`Self::check_table`]; nothing in validate or seal calls that yet. Skipped when empty, so
    /// every schema sealed before it existed keeps its bytes. A binary older than this field drops
    /// it on parse without an error (#830 makes that a refusal).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tables: BTreeMap<String, TableContract>,
    /// The contract per array role (dtype, rank, axes, unit; ADR-0065 §8), checked by a caller with
    /// [`Self::check_array`]. Skipped when empty, as [`Self::tables`].
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub arrays: BTreeMap<String, ArrayContract>,
}

impl ProductSchema {
    /// The JSON keys this type models (ADR-0060 §2.2 newer-field walk); kept equal to a fully
    /// populated instance's keys by a test.
    pub(crate) const FIELDS: &'static [&'static str] = &[
        "product",
        "version",
        "description",
        "fields",
        "blocks",
        "requires_generation",
        "tables",
        "arrays",
    ];

    /// Validate a manifest against this schema: every required block role must have enough
    /// matching blocks, and every required field must be present in `metadata` (or carry a
    /// default). Returns a typed [`crate::Error::Invalid`] naming the first violation.
    pub fn validate(&self, m: &Manifest) -> crate::Result<()> {
        for req in &self.blocks {
            if req.min_count == 0 {
                continue;
            }
            let count = m.blocks.iter().filter(|b| req.matches(b)).count();
            if count < req.min_count as usize {
                return Err(crate::Error::Invalid(format!(
                    "schema '{}' requires {}× block role '{}' ({}), found {count}",
                    self.product,
                    req.min_count,
                    req.role,
                    req.kind
                        .map(|k| format!("{k:?}"))
                        .unwrap_or_else(|| "any".into()),
                )));
            }
        }
        for f in &self.fields {
            if f.required && f.default.is_none() && !m.metadata.contains_key(&f.id) {
                return Err(crate::Error::Invalid(format!(
                    "schema '{}' requires metadata field '{}' ({})",
                    self.product, f.id, f.description
                )));
            }
        }
        // ADR-0058 §3: a schema-declared "needs a recipe" rule — reuse the same block-on-missing
        // mechanism as required fields, applied to the generation slot. Policy is schema data.
        if self.requires_generation && m.generation.as_ref().is_none_or(|g| g.is_empty()) {
            return Err(crate::Error::Invalid(format!(
                "schema '{}' requires a generation record (producer config/config_ref) — ADR-0058 §3",
                self.product
            )));
        }
        Ok(())
    }

    /// The fields this schema marks **inheritable identity** (ADR-0058 §5) — the allowlist
    /// [`crate::provenance::inherit_identity`] copies from a `derived_from` parent. Pure schema data;
    /// the engine holds no field list. Order matches schema declaration.
    pub fn inheritable_fields(&self) -> Vec<&FieldSpec> {
        self.fields.iter().filter(|f| f.inherit).collect()
    }

    /// The schema's **recommended** fields (the warn tier) that this manifest does **not** carry and
    /// that have no default — the FAIR-completeness nudge the engine surfaces as a non-fatal warning.
    /// Never blocks (that's [`Self::validate`]'s job for `required`). Domain-agnostic: the policy is
    /// pure schema data, so it composes through `imaging_base` for every product alike.
    pub fn missing_recommended<'a>(&'a self, m: &Manifest) -> Vec<&'a FieldSpec> {
        self.fields
            .iter()
            .filter(|f| f.recommended && f.default.is_none() && !m.metadata.contains_key(&f.id))
            .collect()
    }

    /// All fields of this schema classified at the given sensitivity `tier` (ADR-0040 §1). The
    /// query the redact / field-encryption phases (deferred) consume to know **which fields** to
    /// touch — pure data, no PHI logic inside the core. Order matches schema declaration.
    pub fn fields_by_sensitivity(&self, tier: Sensitivity) -> Vec<&FieldSpec> {
        self.fields
            .iter()
            .filter(|f| f.sensitivity == tier)
            .collect()
    }

    /// Check a table block's spec against the contract declared for table `role`.
    ///
    /// Passes when every column of `spec` is declared, appears once, and the columns follow the
    /// contract's order, where a column marked
    /// [`optional`](crate::block::table::Column#structfield.optional) may be absent; each present
    /// column's `dtype` and `nullable` must equal the declared ones. No other column attribute
    /// (`unit`, `scale`, `sensitivity`, …) and none of `sequence_key`, `keys` or `references` is
    /// compared, and rows are not looked at: [`contract`] lists what is carried but not checked.
    ///
    /// # Errors
    /// [`crate::Error::Contract`] carrying the first [`ContractViolation`] found, in three passes:
    /// `UnknownRole` when no table contract is declared for `role`; then, over the block's columns
    /// in order, `UnknownColumn` or `DuplicateColumn`; then, walking the contract's columns in
    /// order, the first of `ColumnOrder`, `MissingColumn`, `ColumnDtype` or `ColumnNullability`
    /// (so a dtype mismatch on an early column is reported before a missing later one).
    pub fn check_table(&self, role: &str, spec: &TableSpec) -> crate::Result<()> {
        let contract = self
            .tables
            .get(role)
            .ok_or_else(|| ContractViolation::UnknownRole {
                kind: BlockKind::Table,
                role: role.to_owned(),
            });
        contract
            .and_then(|c| c.check(role, spec))
            .map_err(|violation| self.contract_error(violation))
    }

    /// Check an array block's spec against the contract declared for array `role`: the dtype must
    /// equal the declared one verbatim, the rank must match, and so must the axis names when the
    /// contract declares any. The unit is not compared.
    ///
    /// # Errors
    /// [`crate::Error::Contract`] carrying `UnknownRole` when no array contract is declared for
    /// `role`, else the first of `ArrayDtype`, `ArrayRank` or `ArrayAxes`.
    pub fn check_array(&self, role: &str, spec: &ArraySpec) -> crate::Result<()> {
        let contract = self
            .arrays
            .get(role)
            .ok_or_else(|| ContractViolation::UnknownRole {
                kind: BlockKind::Array,
                role: role.to_owned(),
            });
        contract
            .and_then(|c| c.check(role, spec))
            .map_err(|violation| self.contract_error(violation))
    }

    fn contract_error(&self, violation: ContractViolation) -> crate::Error {
        crate::Error::Contract {
            product: self.product.clone(),
            violation,
        }
    }

    /// Serialize to the JSON value **embedded in a sealed manifest** (`Manifest.schema`), so the
    /// `.tsra` carries its own contract (FAIR-Reusable) and the seal commits to it.
    pub fn to_value(&self) -> crate::Result<serde_json::Value> {
        serde_json::to_value(self)
            .map_err(|e| crate::Error::Invalid(format!("schema does not serialize: {e}")))
    }

    /// Parse an embedded schema JSON (from `Manifest.schema`) back into a typed `ProductSchema`,
    /// **refusing a rule this build doesn't model** (ADR-0060 §2.2, #873).
    ///
    /// The schema is walked against the declared field names of every type it is made of: the top
    /// level, each `fields[i]` and `blocks[i]`, each `tables.<role>` with its `columns[i]` (and their
    /// `referencing`, `transform` included), `references[i]` and `references[i].to`, and each
    /// `arrays.<role>`. A key none of them declares is a rule from a newer format revision. The typed
    /// schema would drop it, so validating against it, inheriting through it or re-emitting it would
    /// silently ignore a rule the file declares: it is refused instead. Only keys count, never values:
    /// a known key holding its default (`inherit: false`, `requires_generation: false`, `unit: null`)
    /// is fine. `fields[i].default` is an opaque value and is never walked.
    ///
    /// # Errors
    /// - [`crate::Error::Invalid`] if `v` is not a `ProductSchema`: a known key of the wrong type, or
    ///   the schema or any record in it in array form instead of an object, or a name (`kind`,
    ///   `order`, `sensitivity`) spelled as an object instead of a string (SPEC §4, one spelling per
    ///   value: a positional array would hide its keys from the walk above; the message names its
    ///   path).
    /// - [`crate::Error::NewerFormatFields`] naming every unknown key by its dotted path within the
    ///   schema (`fields[0].pattern`, `tables.events.row_count_min`).
    pub fn from_value(v: &serde_json::Value) -> crate::Result<Self> {
        let (schema, newer) = Self::view_of(v)?;
        crate::manifest::refuse_newer(&newer)?;
        Ok(schema)
    }

    /// The typed **view** of an embedded schema plus the dotted paths of every key it doesn't model
    /// (the walk [`from_value`](Self::from_value) refuses on), for callers that only **display** the
    /// schema: they show what this build understands and warn about the rest. Anything that acts on
    /// the rules (validate, inherit, re-emit) uses [`from_value`](Self::from_value) instead; the view
    /// silently lacks the newer rules.
    ///
    /// # Errors
    /// [`crate::Error::Invalid`] if `v` is not a `ProductSchema`, as in [`from_value`](Self::from_value)
    /// (a known key of the wrong type, or a record in array form).
    pub fn view_of(v: &serde_json::Value) -> crate::Result<(Self, Vec<String>)> {
        let schema = crate::canonical::from_declared_form(v).map_err(|e| {
            crate::Error::Invalid(format!("embedded schema is not a ProductSchema: {e}"))
        })?;
        Ok((schema, newer_fields_of(v)))
    }
}

/// `object[key]`, or `null` when `object` has no such key (or is not an object).
fn field<'a>(object: &'a serde_json::Value, key: &str) -> &'a serde_json::Value {
    object.get(key).unwrap_or(&serde_json::Value::Null)
}

/// The dotted paths of every key in a product-schema object that this build's types don't declare
/// (see [`ProductSchema::from_value`]).
fn newer_fields_of(v: &serde_json::Value) -> Vec<String> {
    use crate::block::table::Column;
    use crate::manifest::{unknown_keys, unknown_keys_each};
    use crate::referencing::{Referenced, Transform};
    let mut out = Vec::new();
    unknown_keys(v, ProductSchema::FIELDS, "", &mut out);
    unknown_keys_each(field(v, "fields"), FieldSpec::FIELDS, "fields", &mut out);
    unknown_keys_each(
        field(v, "blocks"),
        BlockRequirement::FIELDS,
        "blocks",
        &mut out,
    );
    if let Some(tables) = field(v, "tables").as_object() {
        for (role, t) in tables {
            let at = format!("tables.{role}");
            unknown_keys(t, TableContract::FIELDS, &format!("{at}."), &mut out);
            let columns = format!("{at}.columns");
            unknown_keys_each(field(t, "columns"), Column::FIELDS, &columns, &mut out);
            for (i, c) in field(t, "columns")
                .as_array()
                .into_iter()
                .flatten()
                .enumerate()
            {
                let r = field(c, "referencing");
                let at = format!("{columns}[{i}].referencing.");
                unknown_keys(r, Referenced::FIELDS, &at, &mut out);
                let tr = field(r, "transform");
                let kind = field(tr, "kind").as_str().unwrap_or_default();
                unknown_keys(
                    tr,
                    Transform::fields(kind),
                    &format!("{at}transform."),
                    &mut out,
                );
            }
            let refs = format!("{at}.references");
            unknown_keys_each(
                field(t, "references"),
                ColumnReference::FIELDS,
                &refs,
                &mut out,
            );
            for (i, r) in field(t, "references")
                .as_array()
                .into_iter()
                .flatten()
                .enumerate()
            {
                let to = format!("{refs}[{i}].to.");
                unknown_keys(field(r, "to"), ReferenceTarget::FIELDS, &to, &mut out);
            }
        }
    }
    if let Some(arrays) = field(v, "arrays").as_object() {
        for (role, a) in arrays {
            unknown_keys(
                a,
                ArrayContract::FIELDS,
                &format!("arrays.{role}."),
                &mut out,
            );
        }
    }
    out
}

/// Validate a manifest against its **embedded** schema (self-describing) when present, else the
/// built-in registry (legacy / open-world files). The embedded copy is authoritative — a sealed
/// `.tsra` carries its own contract, so a reader validates what the file *declares*, not whatever
/// schema version happens to be compiled into this binary.
///
/// # Errors
/// [`crate::Error::Invalid`] naming the first violation, or an embedded schema that doesn't parse;
/// [`crate::Error::NewerFormatFields`] when the embedded schema carries a rule this build doesn't
/// model, named by its path in the manifest (`schema.fields[0].pattern`): a gate that can't
/// evaluate a rule never reports "valid" ([`ProductSchema::from_value`]).
pub fn validate_manifest(m: &Manifest) -> crate::Result<()> {
    let Some(v) = &m.schema else {
        return SchemaRegistry::builtin().validate(m);
    };
    let schema = ProductSchema::from_value(v).map_err(|e| match e {
        crate::Error::NewerFormatFields { fields } => crate::Error::NewerFormatFields {
            fields: fields.into_iter().map(|f| format!("schema.{f}")).collect(),
        },
        other => other,
    })?;
    schema.validate(m)
}

/// The registry of built-in product schemas. Domain-agnostic: lookups for unknown products
/// return `None` and validation is then permissive (custom/extension products are allowed).
#[derive(Debug, Clone)]
pub struct SchemaRegistry {
    schemas: BTreeMap<String, ProductSchema>,
}

impl SchemaRegistry {
    /// The built-in Tessera product schemas (medical imaging + simulation lineage).
    pub fn builtin() -> Self {
        let mut schemas = BTreeMap::new();
        for s in builtin_schemas() {
            schemas.insert(s.product.clone(), s);
        }
        SchemaRegistry { schemas }
    }

    pub fn get(&self, product: &str) -> Option<&ProductSchema> {
        self.schemas.get(product)
    }

    pub fn products(&self) -> impl Iterator<Item = &str> {
        self.schemas.keys().map(String::as_str)
    }

    /// Validate a manifest against its declared product schema. Unknown product → `Ok` (a
    /// custom/extension product is permitted). A known product must satisfy its schema.
    pub fn validate(&self, m: &Manifest) -> crate::Result<()> {
        match self.get(&m.product) {
            Some(schema) => schema.validate(m),
            None => Ok(()),
        }
    }

    /// The recommended-but-absent fields for a manifest's declared schema (warn tier; see
    /// [`ProductSchema::missing_recommended`]). Empty for an unknown product (open-world).
    pub fn missing_recommended(&self, m: &Manifest) -> Vec<&FieldSpec> {
        self.get(&m.product)
            .map(|s| s.missing_recommended(m))
            .unwrap_or_default()
    }

    /// The fields of `m`'s declared product schema at the given sensitivity `tier` (ADR-0040 §1)
    /// — the schema-driven query the redact / field-encryption phases (deferred) use to know
    /// which fields to touch. Empty for an unknown product (open-world).
    pub fn fields_by_sensitivity(&self, m: &Manifest, tier: Sensitivity) -> Vec<&FieldSpec> {
        self.get(&m.product)
            .map(|s| s.fields_by_sensitivity(tier))
            .unwrap_or_default()
    }
}

impl Default for SchemaRegistry {
    fn default() -> Self {
        Self::builtin()
    }
}

/// Convenience: one required block of a given role/kind.
fn one(role: &str, kind: Option<BlockKind>, description: &str) -> BlockRequirement {
    BlockRequirement {
        role: role.into(),
        kind,
        min_count: 1,
        description: description.into(),
    }
}

fn schema(product: &str, version: &str, description: &str) -> ProductSchema {
    ProductSchema {
        product: product.into(),
        version: version.into(),
        description: description.into(),
        fields: Vec::new(),
        blocks: Vec::new(),
        requires_generation: false,
        tables: BTreeMap::new(),
        arrays: BTreeMap::new(),
    }
}

/// A composable **requirement-set** (ADR-0029 §5 trait/mixin): the shared imaging metadata every
/// modality-bearing product carries — defined **once** here and composed into `recon` / `dynamic_pet` /
/// `diffusion_mri` / `multicontrast_mri` rather than repeated per schema (DRY). `with` appends the
/// schema's own extra fields.
fn imaging_base(with: Vec<FieldSpec>) -> Vec<FieldSpec> {
    // `modality` is intrinsic acquisition identity — it flows raw→derived (ADR-0058 §5).
    let mut fields = vec![FieldSpec::required("modality", "Imaging modality", "coded")
        .vocabulary("DICOM")
        .with_sensitivity(Sensitivity::Coded)
        .inheritable()];
    fields.extend(with);
    fields
}

/// The built-in product schemas. Each declares its required block(s) + key fields; all are
/// versioned `1.0` and evolve additively (new optional fields/blocks only). See ROADMAP P1 +
/// ADR-0029 §5 (the multi-dimensional `dynamic_pet`/`diffusion_mri`/`multicontrast_mri` set).
fn builtin_schemas() -> Vec<ProductSchema> {
    use BlockKind::{Array, Blob, Table};
    vec![
        ProductSchema {
            // A blob is opaque (no parsed metadata), so operator-supplied context is what makes a
            // preserved file Reusable — `study` is *recommended* (a warn nudge), never required (the
            // escape hatch must stay frictionless): you can always preserve junk now, label it later.
            fields: vec![FieldSpec::recommended(
                "study",
                "Study / exam this preserved file belongs to (FAIR grouping)",
                "string",
            )],
            blocks: vec![one(
                "data",
                Some(Blob),
                "The preserved source file, stored verbatim as opaque bytes (blake3-verified)",
            )],
            ..schema(
                "blob",
                "1.0",
                "Opaque preserved file — bytes stored bit-faithfully, not engine-parsed (the \"junk\" tier).",
            )
        },
        // ── The two **generic-ingest** schemas (ADR-0056 §7). Named by *primitive*, not by source
        //    format, because the primitive is what the product IS: the same `table` schema serves a
        //    Parquet, an Arrow IPC file and a CSV, and nothing downstream needs to care which.
        //
        //    Permissive by the same argument that makes `blob` permissive: every field is
        //    `recommended`, never `required`, so the door stays frictionless (normalise now, label
        //    later) and the warn tier does the FAIR nudging. A generic ingest that *blocked* on
        //    missing semantics would push users back onto `ingest blob`, which is the outcome
        //    ADR-0056 exists to prevent.
        //
        //    These two are also the **laundering rule's** allowlist (ADR-0056 §7): a generic backend
        //    may only produce a product in {table, array, blob}, enforced by the ingest engine, which
        //    knows which backend it dispatched to. Without that, `ingest table events.csv --schema
        //    listmode` would mint a `.tsra` byte-indistinguishable from a real vendor listmode
        //    ingest, carrying PS3.15 `Identifying` tiers no classification pass ever validated.
        ProductSchema {
            fields: vec![
                FieldSpec::recommended(
                    "study",
                    "Study / cohort / experiment this table belongs to (FAIR grouping)",
                    "string",
                ),
                FieldSpec::recommended(
                    "source_format",
                    "Source format normalised at ingest (\"parquet\" | \"arrow\" | \"csv\" | …)",
                    "string",
                ),
            ],
            blocks: vec![one(
                "data",
                Some(Table),
                "Normalised flat columnar table (Vortex-encoded at seal)",
            )],
            ..schema(
                "table",
                "1.0",
                "A generically-ingested flat table — structural preservation, semantics still to be attached.",
            )
        },
        ProductSchema {
            fields: vec![
                FieldSpec::recommended(
                    "study",
                    "Study / cohort / experiment this array belongs to (FAIR grouping)",
                    "string",
                ),
                FieldSpec::recommended(
                    "source_format",
                    "Source format normalised at ingest (\"npy\" | \"npz\" | \"nifti\" | …)",
                    "string",
                ),
            ],
            blocks: vec![one(
                "data",
                Some(Array),
                "Normalised dense N-D numeric grid (Zarr v3 + pcodec at seal)",
            )],
            ..schema(
                "array",
                "1.0",
                "A generically-ingested dense N-D array — structural preservation, semantics still to be attached.",
            )
        },
        ProductSchema {
            // `rescale_*` are pure scan geometry → `Public`. The remaining fields are seeded
            // from DICOM **PS3.15 Annex E** (Basic Application Confidentiality Profile, the
            // tag list any de-identifier must touch) and the DICOM ingest's curated-tag port:
            // the two `Identifying` handles (patient pseudonym + acquisition UID family)
            // plus the DICOM Study/Series UIDs (PS3.15 UID family, `Identifying`), the
            // `StudyDate` (`Sensitive`), and the vendor scan-geometry tags — Manufacturer,
            // ManufacturerModelName, KVP, SliceThickness, PixelSpacing — that stay `Public`
            // (device metadata, no PHI). All are **recommended-or-optional**, never required:
            // a series lacking any single DICOM tag must still ingest (warn tier, not block).
            fields: imaging_base(vec![
                FieldSpec::optional("rescale_slope", "Native→physical slope", "float64"),
                FieldSpec::optional("rescale_intercept", "Native→physical intercept", "float64"),
                FieldSpec::optional(
                    "patient_pseudonym",
                    "Pseudonymised patient handle (DICOM PS3.15 Patient Name / Patient ID family — direct PHI; supply a site-issued pseudonym, never the raw MRN)",
                    "string",
                )
                .with_sensitivity(Sensitivity::Identifying)
                .inheritable(),
                FieldSpec::optional(
                    "acquisition_uid",
                    "Acquisition UID (DICOM PS3.15 UID family — Study/Series/SOPInstance UID; linking PHI under the confidentiality profile)",
                    "string",
                )
                .with_sensitivity(Sensitivity::Identifying)
                .inheritable(),
                // ── DICOM curated-tag port (#253) ──────────────────────────────────────────
                // Ported straight from the DICOM header at ingest time. Sensitivity tiers
                // seeded from PS3.15 — the UIDs link back to the patient/study so they are
                // Identifying, StudyDate is access-controlled scan context so Sensitive (per
                // ADR-0040 §1), and the vendor + geometry tags describe the scanner not the
                // patient so Public.
                FieldSpec::recommended(
                    "study_instance_uid",
                    "DICOM StudyInstanceUID (0020,000D) — PS3.15 UID family, links back to the patient/study",
                    "string",
                )
                .with_sensitivity(Sensitivity::Identifying)
                .inheritable(),
                FieldSpec::recommended(
                    "series_instance_uid",
                    "DICOM SeriesInstanceUID (0020,000E) — PS3.15 UID family, links back to the series",
                    "string",
                )
                .with_sensitivity(Sensitivity::Identifying)
                .inheritable(),
                FieldSpec::recommended(
                    "study_date",
                    "DICOM StudyDate (0008,0020), YYYYMMDD — scan context, access-controlled",
                    "string",
                )
                .with_sensitivity(Sensitivity::Sensitive)
                .inheritable(),
                FieldSpec::recommended(
                    "manufacturer",
                    "DICOM Manufacturer (0008,0070) — device vendor",
                    "string",
                )
                .inheritable(),
                FieldSpec::recommended(
                    "model_name",
                    "DICOM ManufacturerModelName (0008,1090) — device model",
                    "string",
                )
                .inheritable(),
                FieldSpec::recommended("kvp", "DICOM KVP (0018,0060), CT peak kilovoltage", "float64")
                    .unit("kV"),
                FieldSpec::recommended(
                    "slice_thickness",
                    "DICOM SliceThickness (0018,0050), nominal slice thickness",
                    "float64",
                )
                .unit("mm"),
                FieldSpec::recommended(
                    "pixel_spacing",
                    "DICOM PixelSpacing (0028,0030), [row_spacing, col_spacing] in mm",
                    "json",
                )
                .unit("mm"),
            ]),
            blocks: vec![one(
                "volume",
                Some(Array),
                "Reconstructed image volume (z,y,x), native dtype",
            )],
            ..schema(
                "recon",
                "1.0",
                "A reconstructed image volume (CT/PET/μ-map).",
            )
        },
        ProductSchema {
            // `coincidence_mode` is per-level (singles vs prompt vs extended) — NOT inherited.
            // The acquisition-identity fields ARE intrinsic to the acquisition and flow raw→derived
            // (ADR-0058 §5): the raw `.dat` carries them, the derived singles/coin/events inherit
            // them so each product is self-describing. Vendor acquisition *config* (cal files, the
            // acq `.cfg`, load-point) is the raw's `generation.config` recipe, not inheritable identity.
            fields: vec![
                FieldSpec::required(
                    "coincidence_mode",
                    "Acquisition mode (singles / prompt-coincidence / extended-coincidence)",
                    "string",
                ),
                FieldSpec::optional(
                    "patient_id",
                    "Pseudonymised patient handle for this acquisition (direct PHI — supply a site pseudonym, never the raw MRN)",
                    "string",
                )
                .with_sensitivity(Sensitivity::Identifying)
                .inheritable(),
                FieldSpec::optional(
                    "exam",
                    "Exam / study number linking the acquisition to its study",
                    "string",
                )
                .with_sensitivity(Sensitivity::Identifying)
                .inheritable(),
                FieldSpec::optional(
                    "study_date",
                    "Acquisition date (YYYYMMDD or RFC-3339) — scan context, access-controlled",
                    "string",
                )
                .with_sensitivity(Sensitivity::Sensitive)
                .inheritable(),
                FieldSpec::optional(
                    "acquisition_start",
                    "Acquisition start instant",
                    "string",
                )
                .with_sensitivity(Sensitivity::Sensitive)
                .inheritable(),
                FieldSpec::optional("acquisition_stop", "Acquisition stop instant", "string")
                    .with_sensitivity(Sensitivity::Sensitive)
                    .inheritable(),
                FieldSpec::optional("acquisition_duration", "Acquisition duration", "float64")
                    .unit("s")
                    .inheritable(),
            ],
            blocks: vec![one(
                "events",
                Some(Table),
                "Per-event columnar table (timestamps, energies, positions)",
            )],
            ..schema(
                "listmode",
                "1.0",
                "Raw per-event list-mode acquisition data.",
            )
        },
        ProductSchema {
            blocks: vec![one(
                "sinogram",
                Some(Array),
                "Projection / sinogram array (angle, radial, plane)",
            )],
            ..schema("sinogram", "1.0", "Projection-space (sinogram) data.")
        },
        ProductSchema {
            fields: vec![FieldSpec::optional(
                "domain",
                "Histogram domain (energy / lifetime / time-of-flight)",
                "string",
            )],
            blocks: vec![one(
                "spectrum",
                Some(Array),
                "Histogram of an energy/lifetime/TOF quantity — a dense 1-D array by nature (ADR-0029 §6)",
            )],
            ..schema(
                "spectrum",
                "1.0",
                "An energy / positronium-lifetime / TOF histogram.",
            )
        },
        ProductSchema {
            blocks: vec![one(
                "roi",
                None, // representation by nature (ADR-0029 §4): raster label N-D **array** OR a
                "Region(s) of interest — a raster label array, or a parametric / contour / stats table",
            )],
            ..schema(
                "roi",
                "1.0",
                "Regions of interest over an image product (representation chosen by the ROI's nature).",
            )
        },
        ProductSchema {
            blocks: vec![one(
                "transform",
                None,
                "Spatial transform: affine (table) or deformation field (array)",
            )],
            ..schema(
                "transform",
                "1.0",
                "A spatial transform / registration result.",
            )
        },
        ProductSchema {
            blocks: vec![one(
                "calibration",
                None,
                "Calibration coefficients / lookup (normalization, attenuation, dead-time)",
            )],
            ..schema("calibration", "1.0", "Scanner calibration data.")
        },
        ProductSchema {
            fields: vec![
                FieldSpec::optional(
                    "simulator",
                    "Simulation toolkit (e.g. GATE/Geant4)",
                    "string",
                ),
                FieldSpec::optional("seed", "RNG seed for reproducibility", "int64"),
            ],
            blocks: vec![one(
                "output",
                None,
                "Simulation output (hits table or scored volume)",
            )],
            ..schema("sim", "1.0", "Monte-Carlo simulation output.")
        },
        ProductSchema {
            fields: vec![
                FieldSpec::optional("vendor", "Device vendor", "string"),
                FieldSpec::optional("model", "Device model", "string"),
            ],
            blocks: vec![one(
                "raw",
                None,
                "Raw vendor/device payload (normalized at ingest, preserved verbatim)",
            )],
            ..schema(
                "device_data",
                "1.0",
                "Raw device data from an existing system (GE/Siemens/…).",
            )
        },
        // ── ADR-0029 §5 multi-dimensional acquisition schemas (additive) ──
        ProductSchema {
            // ADR-0032 §6: the PET decay-correction reference — the named instant activity is
            // decay-corrected to (so a reader can recompute or un-correct). Optional metadata.
            fields: imaging_base(vec![FieldSpec::optional(
                "decay_correction_reference",
                "Named instant PET activity is decay-corrected to (e.g. injection / scan-start / acquisition-start)",
                "string",
            )]),
            blocks: vec![
                one(
                    "volume",
                    Some(Array),
                    "4-D dynamic volume (t,z,y,x), native dtype — one N-D array, the t_c lever sets per-frame↔TAC locality",
                ),
                one(
                    "frame_timing",
                    Some(Table),
                    "Per-frame start + duration (s, monotonic) and decay-correction reference",
                ),
            ],
            ..schema(
                "dynamic_pet",
                "1.0",
                "A dynamic (4-D) PET acquisition: one time-series volume + a frame-timing table.",
            )
        },
        ProductSchema {
            fields: imaging_base(vec![]),
            blocks: vec![
                one(
                    "volume",
                    Some(Array),
                    "4-D diffusion volume (dir,z,y,x), native dtype",
                ),
                one(
                    "gradients",
                    Some(Table),
                    "Per-direction b-value (s/mm²) + unit b-vector (bval/bvec)",
                ),
            ],
            ..schema(
                "diffusion_mri",
                "1.0",
                "A diffusion MRI acquisition: per-direction volumes + b-values/vectors.",
            )
        },
        ProductSchema {
            fields: imaging_base(vec![]),
            blocks: vec![one(
                "volume",
                Some(Array),
                "One image volume per contrast (≥1 — heterogeneous matrix/voxel, e.g. T1/T2/FLAIR)",
            )],
            ..schema(
                "multicontrast_mri",
                "1.0",
                "A multi-contrast MRI study: N separately-acquired contrast volumes (model B).",
            )
        },
        // ADR-0030 §5 — the non-linear (deformable) registration carrier.
        ProductSchema {
            blocks: vec![one(
                "field",
                Some(Array),
                "A dense displacement/deformation vector field, e.g. `[3, z, y, x]` (one array block)",
            )],
            ..schema(
                "deformation_field",
                "1.0",
                "A non-linear (deformable) spatial transform — a per-voxel displacement field (ADR-0030 §5).",
            )
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::array::{ArrayBlock, ArraySpec};
    use crate::ProductBuilder;

    /// A fixed acquisition timestamp — identity is content-derived, so every test that seals needs
    /// one that never comes from the clock.
    const TS: &str = "2024-01-01T00:00:00Z";

    /// Every declared-field list of the schema walk equals the keys a fully populated instance
    /// writes. The struct literals name every field (no `..`), so adding a field to any of these
    /// types fails to compile here until the walk learns it (ADR-0060 §2.2, #873).
    #[test]
    fn schema_declared_fields_match_fully_populated_instances() {
        use crate::block::table::Column;
        use crate::referencing::{Referenced, Transform};
        let keys = |v: &serde_json::Value| {
            let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
            k.sort();
            k
        };
        let sorted = |f: &[&str]| {
            let mut k: Vec<String> = f.iter().map(|s| s.to_string()).collect();
            k.sort();
            k
        };
        let s = |x: &str| Some(x.to_string());
        let column = Column {
            name: "t".into(),
            dtype: "u8".into(),
            codec: s("pco"),
            short_name: s("t"),
            description: s("time"),
            unit: s("ps"),
            scale: Some(2.0),
            nullable: true,
            referencing: Some(Referenced {
                transform: Transform::Identity,
                unit: s("s"),
                vocabulary: s("UCUM"),
                frame: s("epoch"),
            }),
            sensitivity: Sensitivity::Coded,
            optional: true,
        };
        let reference = ColumnReference {
            columns: vec!["t".into()],
            to: ReferenceTarget {
                scope: ReferenceScope::Block,
                block: "b".into(),
                columns: vec!["t".into()],
            },
            note: s("n"),
        };
        let table = TableContract {
            columns: vec![column],
            sequence_key: vec!["t".into()],
            order: TableOrder::Producer,
            keys: vec![vec!["t".into()]],
            references: vec![reference],
        };
        let array = ArrayContract {
            dtype: "float32".into(),
            rank: 2,
            axes: vec!["y".into(), "x".into()],
            unit: s("1"),
        };
        let schema = ProductSchema {
            product: "p".into(),
            version: "1".into(),
            description: "d".into(),
            fields: vec![FieldSpec {
                id: "f".into(),
                description: "d".into(),
                dtype: "string".into(),
                unit: s("mm"),
                vocabulary: s("DICOM"),
                default: Some(serde_json::json!("x")),
                required: true,
                recommended: true,
                sensitivity: Sensitivity::Identifying,
                inherit: true,
            }],
            blocks: vec![BlockRequirement {
                role: "r".into(),
                kind: Some(BlockKind::Table),
                min_count: 1,
                description: "d".into(),
            }],
            requires_generation: true,
            tables: BTreeMap::from([("events".to_string(), table)]),
            arrays: BTreeMap::from([("map".to_string(), array)]),
        };
        let v = schema.to_value().unwrap();
        let t = &v["tables"]["events"];
        let c = &t["columns"][0];
        assert_eq!(keys(&v), sorted(ProductSchema::FIELDS));
        assert_eq!(keys(&v["fields"][0]), sorted(FieldSpec::FIELDS));
        assert_eq!(keys(&v["blocks"][0]), sorted(BlockRequirement::FIELDS));
        assert_eq!(keys(t), sorted(TableContract::FIELDS));
        assert_eq!(keys(c), sorted(Column::FIELDS));
        assert_eq!(keys(&c["referencing"]), sorted(Referenced::FIELDS));
        assert_eq!(keys(&t["references"][0]), sorted(ColumnReference::FIELDS));
        assert_eq!(
            keys(&t["references"][0]["to"]),
            sorted(ReferenceTarget::FIELDS)
        );
        assert_eq!(keys(&v["arrays"]["map"]), sorted(ArrayContract::FIELDS));
        let coded = serde_json::to_value(Coded {
            vocabulary: "v".into(),
            code: "c".into(),
            label: s("l"),
        })
        .unwrap();
        assert_eq!(keys(&coded), sorted(Coded::FIELDS));
        // Each transform variant (an exhaustive match, so a new variant must be listed).
        for tr in [
            Transform::Identity,
            Transform::Affine1d {
                slope: 1.0,
                intercept: 0.0,
            },
            Transform::Log {
                ln_lo: 0.0,
                ln_step: 1.0,
            },
            Transform::AffineNd {
                dims: 1,
                matrix: vec![1.0, 0.0],
            },
            Transform::Lookup { values: vec![1.0] },
        ] {
            match tr {
                Transform::Identity
                | Transform::Affine1d { .. }
                | Transform::Log { .. }
                | Transform::AffineNd { .. }
                | Transform::Lookup { .. } => {}
            }
            let tv = serde_json::to_value(&tr).unwrap();
            assert_eq!(
                keys(&tv),
                sorted(Transform::fields(tv["kind"].as_str().unwrap()))
            );
        }
        assert!(newer_fields_of(&v).is_empty());
        assert_eq!(ProductSchema::from_value(&v).unwrap(), schema);
    }

    #[test]
    fn registry_has_all_builtins() {
        let r = SchemaRegistry::builtin();
        for p in [
            "recon",
            "listmode",
            "sinogram",
            "spectrum",
            "roi",
            "transform",
            "calibration",
            "sim",
            "device_data",
            // ADR-0029 §5 multi-dimensional acquisitions
            "dynamic_pet",
            "diffusion_mri",
            "multicontrast_mri",
            "deformation_field", // ADR-0030 §5 deformable registration carrier
            "blob",              // ADR-0038 opaque preservation tier
            "table",             // ADR-0056 §7 generic ingest — the table primitive
            "array",             // ADR-0056 §7 generic ingest — the array primitive
        ] {
            assert!(r.get(p).is_some(), "missing built-in schema '{p}'");
        }
        assert_eq!(r.products().count(), 16);
    }

    #[test]
    fn roi_accepts_representation_by_nature_array_or_table() {
        // ADR-0029 §4: an ROI's representation is chosen by its nature — irregular → a raster label
        // **array**; primitive/contour/stats → a **table**. The `roi` schema accepts either.
        use crate::block::array::{ArrayBlock, ArraySpec};
        use crate::block::table::{Column, TableBlock, TableSpec};
        let r = SchemaRegistry::builtin();
        // irregular ROI → a raster uint16 label-map array
        let mask = ArrayBlock::new("mask", ArraySpec::new(vec![8, 8, 8], "uint16"));
        let mut b = ProductBuilder::new("roi", "roi-a", "tumour mask", "2024-01-01T00:00:00Z");
        b.add_block(&mask).unwrap();
        r.validate(&b.seal().unwrap()).unwrap();
        // primitive ROI → a parametric table (type + params)
        let params = TableBlock::new(
            "rois",
            TableSpec {
                columns: vec![Column {
                    name: "radius".into(),
                    dtype: "f4".into(),
                    codec: None,
                    ..Default::default()
                }],
                rows: 1,
                row_index: None,
                partition: None,
                edition: None,
                codec_profile: None,
            },
        );
        let mut b = ProductBuilder::new("roi", "roi-b", "sphere ROI", "2024-01-01T00:00:00Z");
        b.add_block(&params).unwrap();
        r.validate(&b.seal().unwrap()).unwrap();
    }

    #[test]
    fn registration_is_a_transform_product_with_new_frame_and_provenance() {
        // ADR-0030 §5: a registration is a `transform` product whose array carries the **new** (target)
        // world_frame, with a provenance edge recording source→target + the recipe. No new mechanism —
        // it composes the `transform` schema + `world_frame` + the `sources` DAG.
        use crate::block::array::{ArrayBlock, ArraySpec, WorldFrame};
        use crate::provenance::Source;
        let r = SchemaRegistry::builtin();
        let mut spec = ArraySpec::new(vec![8, 8, 8], "int16");
        spec.world_frame = Some(WorldFrame {
            affine: [
                1.0, 0.0, 0.0, 0.0, //
                0.0, 1.0, 0.0, 0.0, //
                0.0, 0.0, 1.0, 0.0,
            ],
            convention: "LPS".into(),
            unit: "mm".into(),
            space: "atlas:mni152".into(), // the TARGET frame the registration resampled into
        });
        let vol = ArrayBlock::new("registered", spec);
        let mut b = ProductBuilder::new(
            "transform",
            "reg-01",
            "rigid reg → MNI",
            "2024-01-01T00:00:00Z",
        );
        b.add_block(&vol).unwrap();
        b.add_source(Source::new(
            "registered_from",
            "blake3:source-manifest-hash",
        ));
        b.with_field(
            "recipe",
            serde_json::json!({"kind": "rigid", "to_space": "atlas:mni152"}),
        );
        let m = b.seal().unwrap();

        // valid `transform`; carries the target frame + the typed provenance edge; verifies.
        r.validate(&m).unwrap();
        assert_eq!(m.product, "transform");
        assert_eq!(m.sources.len(), 1);
        assert_eq!(m.sources[0].role, "registered_from");
        let stored: ArraySpec = serde_json::from_value(m.blocks[0].spec.clone()).unwrap();
        assert_eq!(stored.world_frame.unwrap().space, "atlas:mni152");
        assert!(m.verify().is_ok());
    }

    #[test]
    fn dynamic_pet_requires_volume_and_frame_timing() {
        use crate::block::array::{ArrayBlock, ArraySpec};
        use crate::block::table::{Column, TableBlock, TableSpec};
        let r = SchemaRegistry::builtin();
        let vol = ArrayBlock::new("vol", ArraySpec::new(vec![4, 8, 8, 8], "int16"));
        let timing = TableBlock::new(
            "frame_timing",
            TableSpec {
                columns: vec![Column {
                    name: "start_s".into(),
                    dtype: "f8".into(),
                    codec: None,
                    ..Default::default()
                }],
                rows: 4,
                row_index: None,
                partition: None,
                edition: None,
                codec_profile: None,
            },
        );
        // volume alone → rejected (frame_timing table required)
        let mut b = ProductBuilder::new("dynamic_pet", "DP", "d", "2024-01-01T00:00:00Z");
        b.add_block(&vol).unwrap();
        b.with_field(
            "modality",
            serde_json::json!({"_vocabulary": "DICOM", "_code": "PT"}),
        );
        let m = b.seal().unwrap();
        assert!(r.validate(&m).is_err());
        // volume + frame_timing → valid
        let mut b = ProductBuilder::new("dynamic_pet", "DP", "d", "2024-01-01T00:00:00Z");
        b.add_block(&vol).unwrap();
        b.add_block(&timing).unwrap();
        b.with_field(
            "modality",
            serde_json::json!({"_vocabulary": "DICOM", "_code": "PT"}),
        );
        let m = b.seal().unwrap();
        r.validate(&m).unwrap();
    }

    #[test]
    fn dynamic_pet_carries_optional_decay_correction_reference() {
        use crate::block::array::{ArrayBlock, ArraySpec};
        use crate::block::table::{Column, TableBlock, TableSpec};
        let r = SchemaRegistry::builtin();
        // the schema declares the ADR-0032 §6 decay-correction reference as an optional field.
        let s = r.get("dynamic_pet").unwrap();
        let f = s
            .fields
            .iter()
            .find(|f| f.id == "decay_correction_reference")
            .expect("dynamic_pet declares decay_correction_reference");
        assert!(
            !f.required,
            "decay-correction reference is optional (feature-by-presence)"
        );
        // a product that sets it validates (the named instant activity is corrected to).
        let vol = ArrayBlock::new("vol", ArraySpec::new(vec![4, 8, 8, 8], "int16"));
        let timing = TableBlock::new(
            "frame_timing",
            TableSpec {
                columns: vec![Column {
                    name: "start_s".into(),
                    dtype: "f8".into(),
                    codec: None,
                    ..Default::default()
                }],
                rows: 4,
                row_index: None,
                partition: None,
                edition: None,
                codec_profile: None,
            },
        );
        let mut b = ProductBuilder::new("dynamic_pet", "DP", "d", "2024-01-01T00:00:00Z");
        b.add_block(&vol).unwrap();
        b.add_block(&timing).unwrap();
        b.with_field(
            "modality",
            serde_json::json!({"_vocabulary": "DICOM", "_code": "PT"}),
        );
        b.with_field("decay_correction_reference", serde_json::json!("injection"));
        let m = b.seal().unwrap();
        r.validate(&m).unwrap();
        assert_eq!(
            m.metadata.get("decay_correction_reference"),
            Some(&serde_json::json!("injection"))
        );
    }

    #[test]
    fn imaging_base_trait_set_is_shared_across_modality_schemas() {
        // ADR-0029 §5: the `modality` requirement is defined ONCE in `imaging_base()` and composed
        // into every modality-bearing schema — DRY trait/mixin. Assert every such schema carries the
        // identical shared field, and that `recon` additionally composes its own rescale fields on top.
        let r = SchemaRegistry::builtin();
        let shared = imaging_base(vec![]);
        assert_eq!(
            shared.len(),
            1,
            "imaging_base is exactly the shared modality field"
        );
        let modality = &shared[0];
        for product in ["recon", "dynamic_pet", "diffusion_mri", "multicontrast_mri"] {
            let s = r.get(product).expect("schema present");
            let f = s
                .fields
                .iter()
                .find(|f| f.id == "modality")
                .unwrap_or_else(|| panic!("{product} carries the shared modality field"));
            assert_eq!(
                f, modality,
                "{product}'s modality is the shared trait-set field, byte-for-byte"
            );
        }
        // recon = imaging_base + its own extras (composition appends, does not replace).
        let recon = r.get("recon").unwrap();
        assert_eq!(
            recon.fields.first().map(|f| f.id.as_str()),
            Some("modality")
        );
        assert!(recon.fields.iter().any(|f| f.id == "rescale_slope"));
        assert!(recon.fields.iter().any(|f| f.id == "rescale_intercept"));
    }

    #[test]
    fn recommended_field_warns_but_never_blocks() {
        let r = SchemaRegistry::builtin();
        // a blob with its required `data` block but no `study` → schema-VALID (study is recommended,
        // not required), yet surfaced as a recommendation (the warn tier).
        let mut b = ProductBuilder::new("blob", "junk", "d", "2024-01-01T00:00:00Z");
        b.add_block_ref(crate::block::BlockRef {
            name: "data".parse().unwrap(),
            kind: crate::block::BlockKind::Blob,
            digest: Some("blake3:00".into()),
            spec: serde_json::json!({"filename": "x.l64", "size": 1}),
        });
        let m = b.seal().unwrap();
        assert!(r.validate(&m).is_ok(), "recommended-absent must NOT block");
        let missing = r.missing_recommended(&m);
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].id, "study");

        // supplying it (what `--meta study=…` does) clears the recommendation.
        let mut b2 = ProductBuilder::from_manifest(&m).unwrap();
        b2.with_field("study", serde_json::json!("DUPLET-07"));
        let m2 = b2.seal().unwrap();
        assert!(r.missing_recommended(&m2).is_empty());
    }

    #[test]
    fn recon_validates_with_volume_and_required_field() {
        let r = SchemaRegistry::builtin();
        let vol = ArrayBlock::new("volume", ArraySpec::new(vec![64, 64, 64], "int16"));
        let mut b = ProductBuilder::new("recon", "DP06", "d", "2024-01-01T00:00:00Z");
        b.add_block(&vol).unwrap();
        b.with_field(
            "modality",
            serde_json::json!({"_vocabulary": "DICOM", "_code": "CT"}),
        );
        let m = b.seal().unwrap();
        r.validate(&m).unwrap();
    }

    #[test]
    fn recon_without_volume_block_is_rejected() {
        let r = SchemaRegistry::builtin();
        let mut b = ProductBuilder::new("recon", "DP06", "d", "2024-01-01T00:00:00Z");
        b.with_field("modality", serde_json::json!("CT"));
        // a recon needs a table? no — it needs an *array* volume; add a table instead → fail
        let m = b.seal().unwrap();
        assert!(r.validate(&m).is_err(), "recon with no volume must fail");
    }

    #[test]
    fn recon_missing_required_field_is_rejected() {
        let r = SchemaRegistry::builtin();
        let vol = ArrayBlock::new("volume", ArraySpec::new(vec![64, 64, 64], "int16"));
        let mut b = ProductBuilder::new("recon", "DP06", "d", "2024-01-01T00:00:00Z");
        b.add_block(&vol).unwrap();
        let m = b.seal().unwrap(); // no `modality`
        match r.validate(&m) {
            Err(crate::Error::Invalid(msg)) => assert!(msg.contains("modality"), "{msg}"),
            other => panic!("expected missing-field error, got {other:?}"),
        }
    }

    #[test]
    fn unknown_product_is_permitted_open_world() {
        let r = SchemaRegistry::builtin();
        let m = ProductBuilder::new("my_custom_domain", "x", "d", "2024-01-01T00:00:00Z")
            .seal()
            .unwrap();
        r.validate(&m).unwrap();
    }

    // ─── ADR-0040 §1: sensitivity classification — plain-data tier + schema-driven query ───

    #[test]
    fn sensitivity_defaults_to_public_and_round_trips_through_serde() {
        // Default is `Public` (so an unannotated schema field — and any deserialized older
        // FieldSpec with the field absent — classifies as the safest tier, not PHI).
        assert_eq!(Sensitivity::default(), Sensitivity::Public);

        // snake_case wire form, all four variants round-trip.
        for (variant, wire) in [
            (Sensitivity::Public, "\"public\""),
            (Sensitivity::Coded, "\"coded\""),
            (Sensitivity::Sensitive, "\"sensitive\""),
            (Sensitivity::Identifying, "\"identifying\""),
        ] {
            let s = serde_json::to_string(&variant).unwrap();
            assert_eq!(s, wire, "{variant:?} serializes as {wire}");
            let v: Sensitivity = serde_json::from_str(&s).unwrap();
            assert_eq!(v, variant, "round-trip");
        }

        // A FieldSpec with NO `sensitivity` key deserializes (back-compat) → Public.
        let json = r#"{"id":"x","description":"y","dtype":"string"}"#;
        let f: FieldSpec = serde_json::from_str(json).unwrap();
        assert_eq!(f.sensitivity, Sensitivity::Public);

        // …and a FieldSpec round-trips a non-default tier intact through serde.
        let f =
            FieldSpec::optional("mrn", "PHI", "string").with_sensitivity(Sensitivity::Identifying);
        let s = serde_json::to_string(&f).unwrap();
        let back: FieldSpec = serde_json::from_str(&s).unwrap();
        assert_eq!(back.sensitivity, Sensitivity::Identifying);
    }

    #[test]
    fn recon_seeds_ps3_15_identifying_fields_and_classifies_them() {
        // The spike's PS3.15-seeded example fields + the DICOM curated-tag UIDs are all
        // present on `recon`, marked Identifying.
        let r = SchemaRegistry::builtin();
        let recon = r.get("recon").unwrap();
        for id in [
            "patient_pseudonym",
            "acquisition_uid",
            "study_instance_uid",
            "series_instance_uid",
        ] {
            let f = recon
                .fields
                .iter()
                .find(|f| f.id == id)
                .unwrap_or_else(|| panic!("recon must seed PS3.15 example field '{id}'"));
            assert!(
                !f.required,
                "PS3.15 example field '{id}' is optional (escape hatch must stay frictionless)"
            );
            assert_eq!(
                f.sensitivity,
                Sensitivity::Identifying,
                "PS3.15 example field '{id}' is direct PHI → Identifying"
            );
        }
    }

    #[test]
    fn fields_by_sensitivity_returns_the_right_tier() {
        let r = SchemaRegistry::builtin();
        // Build a real recon manifest so we can query through SchemaRegistry::fields_by_sensitivity.
        let vol = ArrayBlock::new("volume", ArraySpec::new(vec![8, 8, 8], "int16"));
        let mut b = ProductBuilder::new("recon", "DP", "d", "2024-01-01T00:00:00Z");
        b.add_block(&vol).unwrap();
        b.with_field(
            "modality",
            serde_json::json!({"_vocabulary": "DICOM", "_code": "CT"}),
        );
        let m = b.seal().unwrap();

        // Identifying: the PS3.15-seeded recon fields + the DICOM curated UIDs.
        let phi = r.fields_by_sensitivity(&m, Sensitivity::Identifying);
        let phi_ids: Vec<&str> = phi.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(
            phi_ids,
            [
                "patient_pseudonym",
                "acquisition_uid",
                "study_instance_uid",
                "series_instance_uid",
            ]
        );

        // Coded: modality (DICOM controlled vocab).
        let coded = r.fields_by_sensitivity(&m, Sensitivity::Coded);
        let coded_ids: Vec<&str> = coded.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(coded_ids, ["modality"]);

        // Public: rescale + curated DICOM scan-geometry / vendor tags (safe in clear).
        let public = r.fields_by_sensitivity(&m, Sensitivity::Public);
        let public_ids: Vec<&str> = public.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(
            public_ids,
            [
                "rescale_slope",
                "rescale_intercept",
                "manufacturer",
                "model_name",
                "kvp",
                "slice_thickness",
                "pixel_spacing",
            ]
        );

        // Sensitive: DICOM StudyDate (scan context, access-controlled).
        let sensitive = r.fields_by_sensitivity(&m, Sensitivity::Sensitive);
        let sensitive_ids: Vec<&str> = sensitive.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(sensitive_ids, ["study_date"]);

        // Open-world: an unknown product → empty (no schema to classify against).
        let m = ProductBuilder::new("my_custom", "x", "d", "2024-01-01T00:00:00Z")
            .seal()
            .unwrap();
        assert!(r
            .fields_by_sensitivity(&m, Sensitivity::Identifying)
            .is_empty());
    }

    /// The book's product-schema reference (#389) is GENERATED from this registry, so it cannot be
    /// allowed to drift: a schema gaining a required field must show up on the published page in the same
    /// commit. This asserts the committed copy still matches what the generator emits.
    ///
    /// Regenerate with
    /// `cd tessera && cargo run -q -p tessera-core --example schema_reference > tests/derived-docs/schema-reference.md`.
    ///
    /// The comparison lives in a *test* rather than in the `derived-docs` gate because that gate runs in a
    /// nix sandbox with no cargo on PATH — a cargo-backed marker command cannot work there.
    #[test]
    fn schema_reference_matches_the_committed_copy() {
        // The example is the single renderer; re-running it here would duplicate the format, so the test
        // shells out to it exactly as the regenerate command does.
        let out = std::process::Command::new(env!("CARGO"))
            .args([
                "run",
                "-q",
                "-p",
                "tessera-core",
                "--example",
                "schema_reference",
            ])
            .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
            .output()
            .expect("run the schema_reference example");
        assert!(
            out.status.success(),
            "generator failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let generated = String::from_utf8(out.stdout).expect("utf-8");
        let committed = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/derived-docs/schema-reference.md"
        ))
        .expect("the committed reference must exist");
        assert_eq!(
            generated.trim_end(),
            committed.trim_end(),
            "the committed product-schema reference is out of date — regenerate it (see this test's docs)"
        );
    }

    /// ADR-0058 §3: the "needs a recipe" rule is schema data — a schema with `requires_generation`
    /// blocks a product carrying no (or an empty) generation record, and accepts one with a config.
    /// A schema that does not opt in never blocks (the permissive back-compat default).
    #[test]
    fn requires_generation_is_a_schema_declared_block() {
        let ts = "2024-01-01T00:00:00Z";
        let mut m = Manifest::new("daq", "x", "d", ts);

        // Opt-in schema: no generation → block.
        let mut s = schema("daq", "1", "a DAQ product");
        s.requires_generation = true;
        assert!(s.validate(&m).is_err(), "missing recipe must block");

        // An empty generation record is still "no recipe" → block.
        m.generation = Some(crate::provenance::Generation::default());
        assert!(s.validate(&m).is_err(), "empty recipe must block");

        // A non-empty inline config satisfies it.
        m.generation =
            Some(crate::provenance::Generation::default().with("seed", serde_json::json!(42)));
        assert!(s.validate(&m).is_ok(), "a real recipe passes");

        // A schema that does not opt in never blocks on generation.
        let permissive = schema("daq", "1", "a DAQ product");
        let bare = Manifest::new("daq", "y", "d", ts);
        assert!(permissive.validate(&bare).is_ok(), "default is permissive");
    }

    /// ADR-0056 §7: the two generic-ingest schemas are **permissive** — a product carrying nothing
    /// but its one block validates. That is the load-bearing property: a generic ingest that blocked
    /// on absent semantics would push users back onto `ingest blob`, which is the outcome ADR-0056
    /// exists to prevent. The FAIR nudge is the warn tier instead.
    #[test]
    fn generic_ingest_schemas_are_permissive_and_nudge_rather_than_block() {
        use crate::block::table::{Column, TableBlock, TableSpec};
        let r = SchemaRegistry::builtin();

        let t = TableBlock::new(
            "data",
            TableSpec {
                columns: vec![Column::new("x", "f8")],
                rows: 3,
                row_index: None,
                partition: None,
                edition: None,
                codec_profile: None,
            },
        );
        let mut b = ProductBuilder::new("table", "t-01", "a parquet someone handed us", TS);
        b.add_block(&t).unwrap();
        let m = b.seal().unwrap();
        r.validate(&m).expect("a bare generic table validates");

        let a = ArrayBlock::new("data", ArraySpec::new(vec![4, 4], "float32"));
        let mut b = ProductBuilder::new("array", "a-01", "an npy someone handed us", TS);
        b.add_block(&a).unwrap();
        let ma = b.seal().unwrap();
        r.validate(&ma).expect("a bare generic array validates");

        // …but both nudge: `study` and `source_format` are recommended, so the warn tier fires.
        for m in [&m, &ma] {
            let missing: Vec<&str> = r
                .missing_recommended(m)
                .iter()
                .map(|f| f.id.as_str())
                .collect();
            assert_eq!(missing, vec!["study", "source_format"]);
        }

        // Neither requires a recipe — `ingest_decoder` is recorded because the engine derives it,
        // not because a schema compels it (ADR-0056 §6a: "require the key" is a separate, unbuilt
        // mechanism; until it exists a generic product must not be blocked for lacking one).
        assert!(!r.get("table").unwrap().requires_generation);
        assert!(!r.get("array").unwrap().requires_generation);
    }

    /// The block *kind* is what the generic schemas pin: a `table` product whose only block is an
    /// array is not a `table` product. This is what stops `--from`/verb confusion producing a
    /// mislabelled artifact.
    #[test]
    fn generic_schemas_pin_the_block_kind() {
        let r = SchemaRegistry::builtin();
        let a = ArrayBlock::new("data", ArraySpec::new(vec![2, 2], "int16"));
        let mut b = ProductBuilder::new("table", "wrong", "an array claiming to be a table", TS);
        b.add_block(&a).unwrap();
        let err = r.validate(&b.seal().unwrap()).unwrap_err().to_string();
        assert!(err.contains("table") && err.contains("Table"), "got {err}");
    }

    /// ADR-0056 §6.3 / §7: `Unknown` is reachable, serialises as `"unknown"`, and — the part that
    /// matters for the corpus — is **not** the serde default, so every schema written before it
    /// existed still deserialises to `Public`.
    #[test]
    fn unknown_sensitivity_is_opt_in_and_public_stays_the_default() {
        assert_eq!(Sensitivity::default(), Sensitivity::Public);
        assert_eq!(
            serde_json::to_string(&Sensitivity::Unknown).unwrap(),
            r#""unknown""#
        );
        // A FieldSpec written before the variant existed (no `sensitivity` key) is still Public.
        let f: FieldSpec =
            serde_json::from_str(r#"{"id":"x","description":"d","dtype":"string"}"#).unwrap();
        assert_eq!(f.sensitivity, Sensitivity::Public);
        // And the tier query finds it, so `verify --require-classified` has something to select on.
        let s = ProductSchema {
            fields: vec![FieldSpec::optional("c", "an unclassified column", "string")
                .with_sensitivity(Sensitivity::Unknown)],
            ..schema("g", "1", "generic")
        };
        assert_eq!(
            s.fields_by_sensitivity(Sensitivity::Unknown)
                .iter()
                .map(|f| f.id.as_str())
                .collect::<Vec<_>>(),
            vec!["c"]
        );
    }
}
