//! Table block — columnar storage. arrow/parquet backend (feature `table-arrow`).
//!
//! Listmode events, spectra, ROIs. Columnar (never row-major compound — see fd5 #193: a
//! single-column projection on compound costs a full-table read). Per-column codecs, and an
//! optional secondary index for fast random per-event `take` (Lance-style).

use serde::{Deserialize, Serialize};

use super::{Block, BlockKind};

/// First schema difference, in column order (count is checked before columns).
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum SchemaMismatchKind {
    /// Different numbers of columns.
    #[error("column count: expected {expected}, found {found}")]
    ColumnCount {
        /// Declared or first-leaf count.
        expected: usize,
        /// Incoming or later-leaf count.
        found: usize,
    },
    /// A column name differs at a zero-based position, including reordered columns.
    #[error("column {index} name: expected {expected:?}, found {found:?}")]
    ColumnName {
        /// Zero-based column position.
        index: usize,
        /// Declared or first-leaf name.
        expected: String,
        /// Incoming or later-leaf name.
        found: String,
    },
    /// A value-contract field differs on the named column.
    #[error("column '{column}' {field}: expected {expected}, found {found}")]
    Field {
        /// Declared or first-leaf column name.
        column: String,
        /// First differing field, in dtype, nullable, unit, scale, referencing order.
        field: ContractField,
        /// Declared or first-leaf value, with the variant corresponding to `field`.
        /// Boxed to keep the core error small on every `Result` path.
        expected: Box<ContractValue>,
        /// Incoming or later-leaf value, with the variant corresponding to `field`.
        found: Box<ContractValue>,
    },
}

/// A column value-contract field; names and positions belong to [`SchemaMismatchKind`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ContractField {
    /// Stored scalar dtype.
    Dtype,
    /// Whether values carry a validity mask.
    Nullable,
    /// Physical unit after scaling.
    Unit,
    /// Physical value multiplier.
    Scale,
    /// Complete referencing descriptor.
    Referencing,
}

/// Owned diagnostic value, retaining the field's Rust type and optionality.
/// Display uses `absent` for missing values, quoted strings, plain scalars and JSON descriptors.
/// Floating-point values use ordinary `PartialEq`, just like [`ValueContract`].
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ContractValue {
    /// Stored scalar dtype.
    Dtype(String),
    /// Whether values carry a validity mask.
    Nullable(bool),
    /// Physical unit, preserving absent versus explicitly empty.
    Unit(Option<String>),
    /// Physical value multiplier, preserving absent versus explicit.
    Scale(Option<f64>),
    /// Complete structured referencing descriptor.
    Referencing(Option<crate::referencing::Referenced>),
}

impl std::fmt::Display for ContractField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Dtype => "dtype",
            Self::Nullable => "nullable",
            Self::Unit => "unit",
            Self::Scale => "scale",
            Self::Referencing => "referencing",
        })
    }
}

impl std::fmt::Display for ContractValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Dtype(value) => write!(f, "{value:?}"),
            Self::Nullable(value) => write!(f, "{value}"),
            Self::Unit(Some(value)) => write!(f, "{value:?}"),
            Self::Scale(Some(value)) => write!(f, "{value}"),
            Self::Referencing(Some(value)) => {
                // JSON preserves every descriptor member with its public wire name.
                let json = serde_json::to_string(value).map_err(|_| std::fmt::Error)?;
                f.write_str(&json)
            }
            Self::Unit(None) | Self::Scale(None) | Self::Referencing(None) => f.write_str("absent"),
        }
    }
}

/// Borrowed value contract for a column, independent of encoding and descriptive annotations.
/// A decoded column has no metadata; use [`ValueContract::decoded`] for representation checks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ValueContract<'a> {
    /// Stored scalar dtype.
    pub dtype: &'a str,
    /// Whether values carry a validity mask.
    pub nullable: bool,
    /// Physical unit after scaling.
    pub unit: Option<&'a str>,
    /// Physical value multiplier; absence differs from an explicit multiplier.
    pub scale: Option<f64>,
    /// Complete transform, output unit, vocabulary and reference frame/epoch.
    pub referencing: Option<&'a crate::referencing::Referenced>,
}

impl<'a> ValueContract<'a> {
    /// First differing field in dtype, nullable, unit, scale, referencing order.
    /// Exhaustive destructuring makes new fields a compile error until comparison is updated.
    /// Uses exactly the equality semantics of [`ValueContract`], including floating-point values.
    pub fn first_difference(
        self,
        other: Self,
    ) -> Option<(ContractField, ContractValue, ContractValue)> {
        let Self {
            dtype,
            nullable,
            unit,
            scale,
            referencing,
        } = self;
        let Self {
            dtype: other_dtype,
            nullable: other_nullable,
            unit: other_unit,
            scale: other_scale,
            referencing: other_referencing,
        } = other;
        if dtype != other_dtype {
            return Some((
                ContractField::Dtype,
                ContractValue::Dtype(dtype.into()),
                ContractValue::Dtype(other_dtype.into()),
            ));
        }
        if nullable != other_nullable {
            return Some((
                ContractField::Nullable,
                ContractValue::Nullable(nullable),
                ContractValue::Nullable(other_nullable),
            ));
        }
        if unit != other_unit {
            return Some((
                ContractField::Unit,
                ContractValue::Unit(unit.map(str::to_owned)),
                ContractValue::Unit(other_unit.map(str::to_owned)),
            ));
        }
        if scale != other_scale {
            return Some((
                ContractField::Scale,
                ContractValue::Scale(scale),
                ContractValue::Scale(other_scale),
            ));
        }
        if referencing != other_referencing {
            return Some((
                ContractField::Referencing,
                ContractValue::Referencing(referencing.cloned()),
                ContractValue::Referencing(other_referencing.cloned()),
            ));
        }
        None
    }

    /// Representation contract for decoded values without column metadata.
    pub fn decoded(dtype: &'a str, nullable: bool) -> Self {
        Self {
            dtype,
            nullable,
            unit: None,
            scale: None,
            referencing: None,
        }
    }
}

impl<'a> From<&'a Column> for ValueContract<'a> {
    fn from(column: &'a Column) -> Self {
        Self {
            dtype: &column.dtype,
            nullable: column.nullable,
            unit: column.unit.as_deref(),
            scale: column.scale,
            referencing: column.referencing.as_ref(),
        }
    }
}

/// Whether two column value contracts can concatenate without conversion or invented validity.
/// Shared by seal/group validation and decoded column `extend`. Names and order are checked by
/// the table group; codecs and descriptive annotations are excluded. Metadata compares exactly.
pub fn same_value_contract(contract: ValueContract<'_>, other: ValueContract<'_>) -> bool {
    contract.first_difference(other).is_none()
}

/// Compare one column's name and value contract, reporting its first difference.
/// `expected` is the declaration or first leaf; `found` is the incoming data or later leaf.
/// Decoded values use [`ValueContract::decoded`] so absent metadata is not invented.
pub fn column_contract_difference(
    index: usize,
    expected_name: &str,
    expected: ValueContract<'_>,
    found_name: &str,
    found: ValueContract<'_>,
) -> Option<SchemaMismatchKind> {
    if expected_name != found_name {
        return Some(SchemaMismatchKind::ColumnName {
            index,
            expected: expected_name.into(),
            found: found_name.into(),
        });
    }
    expected
        .first_difference(found)
        .map(|(field, expected, found)| SchemaMismatchKind::Field {
            column: expected_name.into(),
            field,
            expected: Box::new(expected),
            found: Box::new(found),
        })
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Column {
    pub name: String,
    /// Arrow-ish dtype string, e.g. "i2", "u4", "f4".
    pub dtype: String,
    /// Per-column codec — columnar layout lets each column compress optimally.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codec: Option<String>,
    /// Human-facing short label (fd5 I1/I2), distinct from `name` (the rename-safe storage id).
    /// e.g. `name = "lt"`, `short_name = "lifetime"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub short_name: Option<String>,
    /// Human + AI-readable description of the column's meaning — so a reader (or an AI) has the
    /// column's semantics without external context (FAIR I1/I2). The vendor HDF5 carries none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// UCUM physical unit of the values (after `scale`, if any): "keV", "mm", "ns", "ms", "1".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Fixed-point scale (#310): physical value = `raw × scale`. `None` ⇒ values are already
    /// physical (no quantization). Carried so the read/compute path recovers physical units.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<f64>,
    /// Whether the column carries a validity mask, i.e. values may be NULL (#330).
    ///
    /// Missing is **not** the same as a float sentinel: `NaN` is a legitimate measured value in a
    /// float column, and an integer column has no sentinel at all, so "unknown" was previously
    /// unrepresentable. A nullable column stores a bit-packed validity mask alongside its values.
    ///
    /// `skip_serializing_if` keeps a non-nullable column's JSON **byte-identical** to a manifest
    /// written before this field existed — which is what lets the committed conformance corpus
    /// keep its `content_hash` without regeneration. Legacy manifests deserialize as `false`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub nullable: bool,
    /// **Epoch/frame anchor** for the column's values (ADR-0056 §6.1, ADR-0032 descriptor).
    ///
    /// `unit` + `scale` say *how big a tick is*; they cannot say *what a tick is counted from*. So
    /// an ingested Arrow `Timestamp(us, _)` would otherwise land as `i8 + unit="s" + scale=1e-6` —
    /// arithmetically indistinguishable from a **duration**, and a live violation of ADR-0046 §2's
    /// ticks-plus-epoch time model. A [`Referenced`](crate::referencing::Referenced) with
    /// `frame = "epoch:unix"` carries the missing half, and the same slot serves any column whose
    /// values are referenced rather than bare (a rescaled intensity, a log-spaced axis).
    ///
    /// Sealed, because it **changes what the values mean** (ADR-0056 §6): a reader who cannot see
    /// the epoch cannot reconstruct the source's semantics, and FAIR-Reusable collapses.
    /// `skip_serializing_if` keeps an unreferenced column's JSON byte-identical to a manifest
    /// written before this field existed, so the committed conformance corpus needs no regeneration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub referencing: Option<crate::referencing::Referenced>,
    /// Per-column PHI classification (ADR-0056 §6.3) — the column-level twin of
    /// [`FieldSpec::sensitivity`](crate::schema::FieldSpec::sensitivity).
    ///
    /// Schema `FieldSpec`s classify *metadata*; a table's PHI lives in its **columns**, and until
    /// now there was nowhere to say so. Generic ingest stamps
    /// [`Sensitivity::Unknown`](crate::schema::Sensitivity::Unknown) on every column an operator
    /// did not classify, which is what lets a future `verify --require-classified` be a one-line
    /// gate before share/push instead of a manual audit.
    ///
    /// Serialised only when it is not the default, so existing manifests are unchanged.
    #[serde(default, skip_serializing_if = "is_public")]
    pub sensitivity: crate::schema::Sensitivity,
    /// Whether a block may **omit** this column altogether (ADR-0065 §8, #809). Distinct from
    /// [`nullable`](Self::nullable), which is per-row NULL inside a present column.
    ///
    /// Meaningful only in a schema's [`TableContract`](crate::schema::TableContract): there it lets
    /// [`ProductSchema::check_table`](crate::schema::ProductSchema::check_table) pass a block
    /// without the column. A block's own [`TableSpec`] lists the columns it has, so the flag means
    /// nothing there and nothing reads it. Skipped when `false`, so every existing manifest keeps
    /// its bytes.
    #[serde(default, skip_serializing_if = "is_false")]
    pub optional: bool,
}

/// `skip_serializing_if` predicate — omit the default tier so adding [`Column::sensitivity`] leaves
/// every previously-sealed manifest's bytes (and therefore its `manifest_hash`) untouched.
fn is_public(s: &crate::schema::Sensitivity) -> bool {
    matches!(s, crate::schema::Sensitivity::Public)
}

/// `skip_serializing_if` predicate — `bool::not` is not usable here (it takes `self` by value,
/// serde passes `&bool`).
fn is_false(b: &bool) -> bool {
    !*b
}

impl Column {
    /// The JSON keys this type models (ADR-0060 §2.2 newer-field walk); kept equal to a fully
    /// populated instance's keys by a test.
    pub(crate) const FIELDS: &'static [&'static str] = &[
        "name",
        "dtype",
        "codec",
        "short_name",
        "description",
        "unit",
        "scale",
        "nullable",
        "referencing",
        "sensitivity",
        "optional",
    ];

    /// A bare column: storage `name` + `dtype`, no codec/annotation. Chain the `with_*` builders
    /// to attach the fd5 annotation triad (`short_name`/`description`/`unit`) and a `scale`.
    pub fn new(name: impl Into<String>, dtype: impl Into<String>) -> Self {
        Column {
            name: name.into(),
            dtype: dtype.into(),
            ..Default::default()
        }
    }
    /// Builder: per-column codec.
    pub fn with_codec(mut self, codec: impl Into<String>) -> Self {
        self.codec = Some(codec.into());
        self
    }
    /// Builder: human short label.
    pub fn with_short_name(mut self, short_name: impl Into<String>) -> Self {
        self.short_name = Some(short_name.into());
        self
    }
    /// Builder: description.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
    /// Builder: UCUM unit.
    pub fn with_unit(mut self, unit: impl Into<String>) -> Self {
        self.unit = Some(unit.into());
        self
    }
    /// Builder: fixed-point scale (physical = raw × scale).
    pub fn with_scale(mut self, scale: f64) -> Self {
        self.scale = Some(scale);
        self
    }
    /// Builder: the column may contain NULLs (#330) — its payload carries a validity mask.
    pub fn nullable(mut self) -> Self {
        self.nullable = true;
        self
    }
    /// Builder: a contract column a block may omit (ADR-0065 §8) — see [`optional`](Column#structfield.optional).
    pub fn optional(mut self) -> Self {
        self.optional = true;
        self
    }
    /// Builder: anchor the column's values in a reference frame — the epoch slot (ADR-0056 §6.1).
    pub fn with_referencing(mut self, referencing: crate::referencing::Referenced) -> Self {
        self.referencing = Some(referencing);
        self
    }
    /// Builder: classify the column's PHI tier (ADR-0056 §6.3).
    pub fn with_sensitivity(mut self, sensitivity: crate::schema::Sensitivity) -> Self {
        self.sensitivity = sensitivity;
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableSpec {
    pub columns: Vec<Column>,
    pub rows: u64,
    /// Optional secondary index column enabling O(1)-ish random row `take`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row_index: Option<String>,
    /// Set on every leaf of a partitioned table (ADR-0064), never on a single block. Skipped when
    /// `None`, so every single-block spec serializes byte for byte as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partition: Option<super::Partition>,
    /// The Vortex **edition** the block's payload was written under, e.g. `"core2026.08.3"`
    /// (ADR-0024, 2026-11-15 amendment): the frozen set of encoding wire IDs the writer was allowed
    /// to emit, as Vortex names it (`<family><year>.<month>.<version>`).
    ///
    /// Sealed, so a product states which Vortex codec contract its table bytes were written under.
    /// It is a **necessary** condition for "same input → same bytes", not a sufficient one: two
    /// blocks of the same rows under different editions may legitimately differ, but the same
    /// edition alone does not make them equal, because the edition bounds which encodings the
    /// writer may use, not which it picks. That choice is the writer's codec set (tessera excludes
    /// ALP, ALPRD, integer Pco, Delta and OnPair, all inside `core2026.08.3`) and the pinned
    /// Vortex's compressor, which tessera changes only as a format event (ADR-0024) and names in
    /// [`TableSpec::codec_profile`].
    ///
    /// **Set by the writer, not the caller.** `tessera-io`'s table sealing paths (`table_block`,
    /// the staged jobs, `push_table` / `open_table`) record the edition they write under
    /// (`tessera_io::table::TABLE_EDITION`) on every table block and partition leaf, and refuse a
    /// spec that names a different one: they can only write their own. Leave it `None` when
    /// building a spec. A caller that encodes with `table::encode` and builds its own `BlockRef`
    /// bypasses this and must set the field itself.
    ///
    /// **Absent** means the block was sealed before the field existed, by the vortex 0.75 writer
    /// (before #635), which had no edition gate: which encodings it could emit is known only from
    /// that writer build. Such a block still reads and verifies; its manifest bytes and hashes are
    /// unchanged, because `None` is skipped on serialization, as for [`TableSpec::partition`].
    ///
    /// Reading never consults it: Vortex decodes by the wire IDs in the file, whatever edition
    /// admitted them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edition: Option<String>,
    /// The tessera **codec profile** the block's payload was written with, e.g. `"tessera-table-v1"`
    /// (ADR-0024, 2026-11-16 amendment, #869): the writer's codec set (which of the edition's
    /// schemes the compressor may choose from) together with the pinned Vortex compressor that
    /// chooses among them. Named `tessera-table-v<N>`; `N` moves on any change to the codec set or
    /// on a Vortex bump that changes the compressor's choices, each a format event.
    ///
    /// Sealed, beside [`TableSpec::edition`], so that "same input + same edition + same profile →
    /// same bytes" can be checked from the product alone: two blocks of the same rows that record
    /// the same edition and the same profile must have the same payload bytes. It promises
    /// determinism of bytes, **not** decodability: a reader decodes by the wire IDs in the file,
    /// which the edition bounds.
    ///
    /// **Set by the writer, not the caller**, through the same path as `edition`
    /// (`tessera_io::table::TABLE_CODEC_PROFILE`, on every table block and partition leaf). The
    /// writer refuses a spec that names another profile. Leave it `None` when building a spec.
    ///
    /// **Readers never refuse on it.** An unknown profile (from a newer writer) and an absent one
    /// (a block sealed before #869, including every `0.1.0-alpha.1`–`alpha.5` product) both decode
    /// and verify normally. Absent is omitted on serialization, so such a block keeps its manifest
    /// bytes and hashes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codec_profile: Option<String>,
}

pub struct TableBlock {
    pub name: String,
    pub spec: TableSpec,
}

impl TableBlock {
    pub fn new(name: impl Into<String>, spec: TableSpec) -> Self {
        TableBlock {
            name: name.into(),
            spec,
        }
    }
}

impl Block for TableBlock {
    fn name(&self) -> &str {
        &self.name
    }
    fn kind(&self) -> BlockKind {
        BlockKind::Table
    }
    fn spec_json(&self) -> crate::Result<serde_json::Value> {
        crate::canonical::to_json_value(&self.spec)
    }
    fn digest(&self) -> crate::Result<String> {
        // Spike: digest the spec. Real impl digests the encoded column chunks.
        Ok(crate::hash::digest(&serde_json::to_vec(&self.spec)?))
    }
}

#[cfg(feature = "table-arrow")]
impl TableBlock {
    /// Write the columnar payload via arrow/parquet. Not yet implemented.
    pub fn write_parquet(&self, _path: &std::path::Path) -> crate::Result<()> {
        Err(crate::Error::Unimplemented(
            "TableBlock::write_parquet (arrow backend)",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::Column;

    /// The set of JSON keys a `Column` serialises to, sorted.
    ///
    /// Sorted deliberately: these assertions are about **which** keys are emitted (the
    /// `skip_serializing_if` back-compat guarantee), never about their order. And order is not ours
    /// to assert — `serde_json`'s `preserve_order` feature is enabled under workspace feature
    /// unification (Vortex pulls it in) and disabled for a bare `cargo test -p tessera-core`, so an
    /// order-sensitive assertion here passes in one invocation and fails in the other. The sealed
    /// bytes are unaffected either way: the seal canonicalises through RFC 8785 JCS, which sorts.
    fn json_keys(c: &Column) -> Vec<String> {
        let v = serde_json::to_value(c).unwrap();
        let mut keys: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        keys
    }

    #[test]
    fn builders_attach_annotation_triad_and_scale() {
        let c = Column::new("en", "i2")
            .with_short_name("energy")
            .with_description("Calibrated per-photon energy")
            .with_unit("keV")
            .with_scale(0.1);
        assert_eq!(c.name, "en");
        assert_eq!(c.dtype, "i2");
        assert_eq!(c.short_name.as_deref(), Some("energy"));
        assert_eq!(c.unit.as_deref(), Some("keV"));
        assert_eq!(c.scale, Some(0.1));
        assert_eq!(c.codec, None);
    }

    #[test]
    fn bare_column_skips_annotation_fields_on_serialize() {
        // Back-compat: an unannotated column serializes to exactly the legacy shape (name+dtype),
        // so existing on-disk `.tsra` specs and content hashes are unaffected.
        let bare = Column::new("ms", "u4");
        assert_eq!(json_keys(&bare), ["dtype", "name"]);
    }

    #[test]
    fn annotation_round_trips_through_json() {
        let c = Column::new("lt", "i2").with_unit("ns").with_scale(0.001);
        let back: Column = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(back, c);
    }

    /// A non-nullable column must serialize with NO `nullable` key at all — not `"nullable":false`.
    ///
    /// This is the corpus-stability guarantee for #330: every committed fixture in
    /// `tessera/corpus/` has non-null columns, and its `manifest_hash` / `content_hash` are
    /// pinned goldens. If this field serialized unconditionally, every one of those hashes would
    /// change and the whole conformance corpus would need regeneration.
    #[test]
    fn non_nullable_column_emits_no_nullable_key() {
        let c = Column::new("t", "u8");
        let keys = json_keys(&c);
        assert!(
            !keys.contains(&"nullable".to_string()),
            "non-nullable column must not emit the key: {keys:?}"
        );
        assert_eq!(keys, ["dtype", "name"]);
    }

    /// A nullable column round-trips, and the flag is explicit in the JSON when set.
    #[test]
    fn nullable_flag_round_trips() {
        let c = Column::new("e", "i2").nullable();
        assert!(c.nullable);
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v.get("nullable").and_then(|b| b.as_bool()), Some(true));
        let back: Column = serde_json::from_value(v).unwrap();
        assert_eq!(back, c);
    }

    /// A manifest written before #330 has no `nullable` key; it must deserialize as non-nullable
    /// rather than failing, so existing `.tsra` files stay readable.
    #[test]
    fn legacy_column_json_without_nullable_deserializes_as_non_nullable() {
        let c: Column = serde_json::from_str(r#"{"name":"t","dtype":"u8"}"#).unwrap();
        assert!(!c.nullable);
        assert_eq!(c.name, "t");
    }

    #[test]
    fn legacy_spec_without_annotation_deserializes() {
        // A pre-#307 column (no annotation keys) must still parse (fields default to None).
        let c: Column = serde_json::from_str(r#"{"name":"t","dtype":"u8"}"#).unwrap();
        assert_eq!(c.name, "t");
        assert!(c.unit.is_none() && c.scale.is_none() && c.description.is_none());
    }

    /// ADR-0056 §6.1 — the epoch slot. `unit`+`scale` size a tick; only `referencing` says what it
    /// is counted *from*, which is the difference between an absolute instant and a duration
    /// (ADR-0046 §2). An ingested Arrow `Timestamp(us, tz)` lands here.
    #[test]
    fn referencing_carries_the_epoch_a_unit_and_scale_cannot() {
        use crate::referencing::{Referenced, Transform};
        let ts = Column::new("acq_time", "i8")
            .with_unit("s")
            .with_scale(1e-6)
            .with_referencing(
                Referenced {
                    transform: Transform::Affine1d {
                        slope: 1e-6,
                        intercept: 0.0,
                    },
                    unit: Some("s".into()),
                    vocabulary: None,
                    frame: Some("epoch".into()),
                }
                .with_epoch("unix"),
            );
        let r = ts.referencing.as_ref().expect("epoch anchor present");
        assert_eq!(r.frame.as_deref(), Some("epoch:unix"));
        // The frame must be in ADR-0032's pinned vocabulary, not free text — `epoch:<instant>` is
        // the refinement the existing validator already accepts, so no new frame vocabulary lands.
        assert!(r.vocabularies_pinned());
        // …and the whole column round-trips (the descriptor is sealed, so it must serialise).
        let back: Column = serde_json::from_str(&serde_json::to_string(&ts).unwrap()).unwrap();
        assert_eq!(back, ts);
    }

    /// ADR-0056 §6.3 — a column's PHI tier. Generic ingest stamps `Unknown` on anything the
    /// operator did not classify, because `Public` there would seal a false safety claim.
    #[test]
    fn sensitivity_round_trips_and_unknown_is_expressible() {
        use crate::schema::Sensitivity;
        let c = Column::new("patient_id", "str").with_sensitivity(Sensitivity::Unknown);
        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains(r#""sensitivity":"unknown""#), "got {json}");
        let back: Column = serde_json::from_str(&json).unwrap();
        assert_eq!(back.sensitivity, Sensitivity::Unknown);
    }

    /// The corpus-stability guarantee for ADR-0056 §6, stated the same way #330 stated it for
    /// `nullable`: both new fields are `skip_serializing_if`-guarded, so a column that uses neither
    /// serialises to the **pre-ADR-0056 byte sequence**. Every committed fixture in `tessera/corpus/`
    /// is such a column; if either field serialised unconditionally, every pinned `content_hash` and
    /// `manifest_hash` in the corpus would move and the format would have silently revved.
    ///
    /// Note the two fields differ in *why* they are omitted: `referencing` is `Option::is_none`, but
    /// `sensitivity` is not an `Option` — it is omitted by value, when it equals the `Public` default.
    #[test]
    fn unreferenced_unclassified_column_emits_neither_key() {
        use crate::schema::Sensitivity;
        let annotated = Column::new("en", "i2").with_unit("keV").with_scale(0.1);
        assert_eq!(json_keys(&annotated), ["dtype", "name", "scale", "unit"]);
        // The explicit-Public case is the one that would bite: it must also emit nothing.
        let explicit = Column::new("ms", "u4").with_sensitivity(Sensitivity::Public);
        assert_eq!(
            json_keys(&explicit),
            ["dtype", "name"],
            "an explicitly-Public column must serialise like a pre-ADR-0056 one"
        );
    }

    /// A spec sealed before `edition` existed (the vortex 0.75 writer, before #635), verbatim.
    const PRE_EDITION_SPEC: &str = r#"{"columns":[{"name":"t","dtype":"u8"}],"rows":3}"#;

    /// ADR-0024's 2026-11-15 amendment: a pre-edition spec still parses, reads as "no edition
    /// recorded", and re-serialises to exactly its old bytes, so the manifest and its hashes stay
    /// put.
    #[test]
    fn pre_edition_spec_parses_without_edition_and_reserialises_byte_identically() {
        let spec: super::TableSpec = serde_json::from_str(PRE_EDITION_SPEC).unwrap();
        assert_eq!(spec.edition, None);
        assert_eq!(serde_json::to_string(&spec).unwrap(), PRE_EDITION_SPEC);
    }

    /// A recorded edition is serialised under the `edition` key and survives the round trip.
    #[test]
    fn edition_round_trips_through_json() {
        let mut spec: super::TableSpec = serde_json::from_str(PRE_EDITION_SPEC).unwrap();
        spec.edition = Some("core2026.08.3".into());
        let json = serde_json::to_value(&spec).unwrap();
        assert_eq!(json["edition"], "core2026.08.3");
        let back: super::TableSpec = serde_json::from_value(json).unwrap();
        assert_eq!(back.edition.as_deref(), Some("core2026.08.3"));
    }

    /// A spec sealed with an edition but before `codec_profile` existed (#635 to #869), verbatim.
    const PRE_PROFILE_SPEC: &str =
        r#"{"columns":[{"name":"t","dtype":"u8"}],"rows":3,"edition":"core2026.08.3"}"#;

    /// ADR-0024's 2026-11-16 amendment: a spec without a profile parses as "no profile recorded"
    /// and re-serialises to exactly its old bytes, so its manifest and hashes stay put.
    #[test]
    fn pre_profile_spec_parses_without_profile_and_reserialises_byte_identically() {
        let spec: super::TableSpec = serde_json::from_str(PRE_PROFILE_SPEC).unwrap();
        assert_eq!(spec.codec_profile, None);
        assert_eq!(serde_json::to_string(&spec).unwrap(), PRE_PROFILE_SPEC);
    }

    /// A recorded profile is serialised under `codec_profile`, after `edition`, and a profile this
    /// build has never heard of parses as data, not as an error (readers never refuse on it).
    #[test]
    fn codec_profile_round_trips_and_an_unknown_one_parses() {
        let mut spec: super::TableSpec = serde_json::from_str(PRE_PROFILE_SPEC).unwrap();
        spec.codec_profile = Some("tessera-table-v99".into());
        let text = serde_json::to_string(&spec).unwrap();
        assert!(
            text.ends_with(r#","edition":"core2026.08.3","codec_profile":"tessera-table-v99"}"#),
            "{text}"
        );
        let back: super::TableSpec = serde_json::from_str(&text).unwrap();
        assert_eq!(back.codec_profile.as_deref(), Some("tessera-table-v99"));
    }
}

#[cfg(test)]
mod contract_difference_tests {
    use super::*;

    #[test]
    fn first_difference_agrees_with_equality_for_all_pairs() {
        let reference = crate::referencing::Referenced::identity(Some("ns".into()));
        let mut contracts = Vec::new();
        for dtype in ["i4", "f8"] {
            for nullable in [false, true] {
                for unit in [None, Some(""), Some("ns")] {
                    for scale in [None, Some(0.0), Some(-0.0), Some(0.001), Some(f64::NAN)] {
                        for referencing in [None, Some(&reference)] {
                            contracts.push(ValueContract {
                                dtype,
                                nullable,
                                unit,
                                scale,
                                referencing,
                            });
                        }
                    }
                }
            }
        }
        for &left in &contracts {
            for &right in &contracts {
                assert_eq!(left.first_difference(right).is_none(), left == right);
            }
        }
    }

    #[test]
    fn diagnostic_values_render_readably() {
        assert_eq!(
            format!(
                "{}={}",
                ContractField::Scale,
                ContractValue::Scale(Some(0.001))
            ),
            "scale=0.001"
        );
        assert_eq!(ContractValue::Scale(None).to_string(), "absent");
        let referenced = crate::referencing::Referenced::identity(Some("ns".into()));
        assert_eq!(
            ContractValue::Referencing(Some(referenced)).to_string(),
            r#"{"transform":{"kind":"identity"},"unit":"ns"}"#
        );
        assert_eq!(ContractValue::Referencing(None).to_string(), "absent");
        assert_eq!(ContractValue::Unit(Some(String::new())).to_string(), "\"\"");
        assert_eq!(ContractValue::Unit(None).to_string(), "absent");
    }

    #[test]
    fn typed_fields_follow_contract_order_and_full_equality() {
        let expected = Column {
            name: "value".into(),
            dtype: "i4".into(),
            ..Column::default()
        };
        let mut found = Column {
            dtype: "f8".into(),
            nullable: true,
            unit: Some("".into()),
            scale: Some(0.001),
            referencing: Some(crate::referencing::Referenced::identity(Some("ns".into()))),
            ..expected.clone()
        };
        let cases = [
            (
                ContractField::Dtype,
                ContractValue::Dtype("i4".into()),
                ContractValue::Dtype("f8".into()),
            ),
            (
                ContractField::Nullable,
                ContractValue::Nullable(false),
                ContractValue::Nullable(true),
            ),
            (
                ContractField::Unit,
                ContractValue::Unit(None),
                ContractValue::Unit(Some("".into())),
            ),
            (
                ContractField::Scale,
                ContractValue::Scale(None),
                ContractValue::Scale(Some(0.001)),
            ),
            (
                ContractField::Referencing,
                ContractValue::Referencing(None),
                ContractValue::Referencing(found.referencing.clone()),
            ),
        ];
        for (field, want_expected, want_found) in cases {
            let left = ValueContract::from(&expected);
            let right = ValueContract::from(&found);
            assert_eq!(same_value_contract(left, right), left == right);
            assert_eq!(
                left.first_difference(right),
                Some((field, want_expected, want_found))
            );
            match field {
                ContractField::Dtype => found.dtype = expected.dtype.clone(),
                ContractField::Nullable => found.nullable = expected.nullable,
                ContractField::Unit => found.unit = expected.unit.clone(),
                ContractField::Scale => found.scale = expected.scale,
                ContractField::Referencing => found.referencing = expected.referencing.clone(),
            }
        }
        assert!(same_value_contract((&expected).into(), (&found).into()));
        found.description = Some("annotation".into());
        assert!(same_value_contract((&expected).into(), (&found).into()));
        found.scale = Some(f64::NAN);
        let contract = ValueContract::from(&found);
        assert!(!same_value_contract(contract, contract));
        assert!(matches!(
            contract.first_difference(contract),
            Some((ContractField::Scale, _, _))
        ));
        found.scale = Some(-0.0);
        let mut positive_zero = found.clone();
        positive_zero.scale = Some(0.0);
        assert!(same_value_contract(
            (&found).into(),
            (&positive_zero).into()
        ));
    }

    #[test]
    fn name_precedes_contract_fields_at_the_given_position() {
        assert_eq!(
            column_contract_difference(
                2,
                "energy",
                ValueContract::decoded("i4", false),
                "time",
                ValueContract::decoded("f8", true)
            ),
            Some(SchemaMismatchKind::ColumnName {
                index: 2,
                expected: "energy".into(),
                found: "time".into(),
            })
        );
    }
}
