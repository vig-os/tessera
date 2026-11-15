//! The **column contract** of a product schema, keyed by block role (ADR-0065 §8, #809).
//!
//! A [`ProductSchema`](super::ProductSchema)'s `fields` and `blocks` say which metadata a product
//! carries and how many blocks play each role. They say nothing about a block's *inside*. The
//! contract does: per table role an ordered column list ([`TableContract`]), per array role a dtype
//! and a rank ([`ArrayContract`]).
//!
//! The shape is the #801 drafts' (`tests/fixtures/801-consumer-schemas/*.tables.json`); `arrays` is
//! ADR-0065's addition. Here the contract is parsed, carried and sealed with the schema; no block is
//! checked against it yet. Checking a block's columns against its role is the rest of #809, the
//! sequence-key checks over the rows are #833, `keys` and `references` are #780, and the contract's
//! own consistency (a column listed twice, a key naming an undeclared column) is #844.
//!
//! The contract is additive: an older binary deserialising a schema that carries it drops it
//! without a word (ADR-0065 §8, G6). Making that a typed refusal is #830.

use serde::{Deserialize, Serialize};

use crate::block::table::Column;

/// The contract for one **table** role: its columns in storage order, and the row-order and key
/// declarations that travel with them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TableContract {
    /// The columns, in the order a block of this role stores them. A column marked
    /// [`optional`](Column#structfield.optional) may be absent from a block altogether; every other column must be
    /// present, in this order.
    pub columns: Vec<Column>,
    /// The ordered key columns the rows are sorted by (#802, ADR-0065 §7.2). Meant to be empty
    /// when [`order`](Self::order) is [`TableOrder::Producer`]; nothing enforces that yet (#844).
    #[serde(default)]
    pub sequence_key: Vec<String>,
    /// How the rows are ordered. Absent in JSON means [`TableOrder::SequenceKey`].
    #[serde(default, skip_serializing_if = "TableOrder::is_sequence_key")]
    pub order: TableOrder,
    /// Candidate keys: each entry is a list of column names whose values are unique per row.
    #[serde(default)]
    pub keys: Vec<Vec<String>>,
    /// Foreign-key-like references from this table's columns to another block (#780).
    #[serde(default)]
    pub references: Vec<ColumnReference>,
}

/// How a table role's rows are ordered.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TableOrder {
    /// The rows are in [`TableContract::sequence_key`] order. The JSON default (key absent).
    #[default]
    SequenceKey,
    /// The rows are in the producer's own order and the sequence key is empty, as the drafts mark
    /// single-producer tables (JSON `"producer"`).
    Producer,
}

impl TableOrder {
    fn is_sequence_key(&self) -> bool {
        *self == TableOrder::SequenceKey
    }
}

/// A reference from some of a table's columns to columns of another block (the drafts' shape).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColumnReference {
    /// The referencing columns of this table.
    pub columns: Vec<String>,
    /// What they point at.
    pub to: ReferenceTarget,
    /// Free-text explanation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// The target of a [`ColumnReference`]: columns of a block, in this product or in a source product.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReferenceTarget {
    /// Where the target block lives.
    pub scope: ReferenceScope,
    /// The target block's role.
    pub block: String,
    /// The referenced columns, positionally matching [`ColumnReference::columns`].
    pub columns: Vec<String>,
}

/// Where a reference's target block lives. JSON: `"block"` or `"source(<role>)"`; any other string
/// is refused when the schema is parsed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum ReferenceScope {
    /// A block of the same product.
    Block,
    /// A block of the source product this product's provenance names under the given role.
    Source(String),
}

impl TryFrom<String> for ReferenceScope {
    type Error = String;
    fn try_from(s: String) -> Result<Self, String> {
        if s == "block" {
            return Ok(ReferenceScope::Block);
        }
        match s.strip_prefix("source(").and_then(|r| r.strip_suffix(')')) {
            Some(role) if !role.is_empty() => Ok(ReferenceScope::Source(role.to_owned())),
            _ => Err(format!(
                "reference scope '{s}' is neither \"block\" nor \"source(<role>)\""
            )),
        }
    }
}

impl From<ReferenceScope> for String {
    fn from(s: ReferenceScope) -> String {
        match s {
            ReferenceScope::Block => "block".into(),
            ReferenceScope::Source(role) => format!("source({role})"),
        }
    }
}

/// The contract for one **array** role (ADR-0065's addition; no #801 draft has one).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArrayContract {
    /// The array dtype, in [`ArraySpec::dtype`](crate::block::array::ArraySpec::dtype)'s vocabulary
    /// (e.g. `"int16"`).
    pub dtype: String,
    /// The number of dimensions.
    pub rank: u32,
    /// The named axes in storage order. Empty means the contract leaves them to the producer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub axes: Vec<String>,
    /// UCUM unit of the sample values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}
