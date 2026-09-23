//! Table block — columnar storage. arrow/parquet backend (feature `table-arrow`).
//!
//! Listmode events, spectra, ROIs. Columnar (never row-major compound — see fd5 #193: a
//! single-column projection on compound costs a full-table read). Per-column codecs, and an
//! optional secondary index for fast random per-event `take` (Lance-style).

use serde::{Deserialize, Serialize};

use super::{Block, BlockKind};

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
}

/// `skip_serializing_if` predicate — `bool::not` is not usable here (it takes `self` by value,
/// serde passes `&bool`).
fn is_false(b: &bool) -> bool {
    !*b
}

impl Column {
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableSpec {
    pub columns: Vec<Column>,
    pub rows: u64,
    /// Optional secondary index column enabling O(1)-ish random row `take`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row_index: Option<String>,
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
        Ok(serde_json::to_value(&self.spec)?)
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
        let v = serde_json::to_value(&bare).unwrap();
        let obj = v.as_object().unwrap();
        assert_eq!(obj.keys().collect::<Vec<_>>(), vec!["name", "dtype"]);
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
        let v = serde_json::to_value(&c).unwrap();
        let obj = v.as_object().unwrap();
        assert!(
            !obj.contains_key("nullable"),
            "non-nullable column must not emit the key: {obj:?}"
        );
        assert_eq!(obj.keys().collect::<Vec<_>>(), vec!["name", "dtype"]);
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
}
