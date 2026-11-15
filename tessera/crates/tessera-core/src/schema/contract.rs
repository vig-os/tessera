//! The **column contract** of a product schema, keyed by block role (ADR-0065 §8, #809).
//!
//! A [`ProductSchema`](super::ProductSchema)'s `fields` and `blocks` say which metadata a product
//! carries and how many blocks play each role. They say nothing about a block's *inside*. The
//! contract does: per table role an ordered column list ([`TableContract`]), per array role a dtype
//! and a rank ([`ArrayContract`]). [`ProductSchema::check_table`](super::ProductSchema::check_table)
//! and [`ProductSchema::check_array`](super::ProductSchema::check_array) check one block's spec
//! against its role and return a typed [`ContractViolation`] naming the role and the column.
//!
//! The shape is the #801 drafts' (`tests/fixtures/801-consumer-schemas/*.tables.json`); `arrays` is
//! ADR-0065's addition. What is **carried but not checked** here:
//! - every column attribute but `name`, `dtype` and `nullable`: `unit`, `scale`, `description`,
//!   `short_name`, `codec`, `referencing` and `sensitivity` (so a block may declare a column
//!   `public` that the contract marks `identifying`), and an array's `unit`;
//! - `sequence_key` and `order`: the sequence-key checks run over the rows (#833);
//! - `keys` and `references`: sealed with the schema, checked nowhere yet (#780).
//!
//! Nor is the contract checked against itself (#844): a key naming an undeclared column or
//! `"producer"` order with a non-empty `sequence_key` parses and is carried. A contract that lists a
//! column twice also parses, but no block can satisfy it: `check_table` refuses every block of that
//! role with a [`ContractViolation::ColumnOrder`] for the repeated column.
//!
//! The contract is additive: an older binary deserialising a schema that carries it drops it
//! without a word (ADR-0065 §8, G6). Making that a typed refusal is #830.

use serde::{Deserialize, Serialize};

use crate::block::array::ArraySpec;
use crate::block::table::{Column, TableSpec};
use crate::block::BlockKind;

/// The contract for one **table** role: its columns in storage order, and the row-order and key
/// declarations that travel with them.
///
/// **Serialisation rule** (sealed bytes, so fixed): `columns` is always written; every other field
/// is left out when it holds its default (an empty list, [`TableOrder::SequenceKey`]), as is every
/// defaulted field of [`ArrayContract`], [`ColumnReference`] and [`Column`]. Absent and empty
/// therefore mean the same and serialise the same.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TableContract {
    /// The columns, in the order a block of this role stores them. A column marked
    /// [`optional`](Column#structfield.optional) may be absent from a block altogether; every other column must be
    /// present, in this order.
    pub columns: Vec<Column>,
    /// The ordered key columns the rows are sorted by (#802, ADR-0065 §7.2). Meant to be empty
    /// when [`order`](Self::order) is [`TableOrder::Producer`]; nothing enforces that yet (#844).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sequence_key: Vec<String>,
    /// How the rows are ordered. Absent in JSON means [`TableOrder::SequenceKey`].
    #[serde(default, skip_serializing_if = "TableOrder::is_sequence_key")]
    pub order: TableOrder,
    /// Candidate keys: each entry is a list of column names whose values are unique per row.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<Vec<String>>,
    /// Foreign-key-like references from this table's columns to another block (#780).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
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
    /// The array dtype, in [`ArraySpec::dtype`]'s vocabulary (e.g. `"int16"`), compared verbatim.
    pub dtype: String,
    /// The number of dimensions.
    pub rank: u32,
    /// The named axes in storage order. Empty means the contract leaves them to the producer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub axes: Vec<String>,
    /// UCUM unit of the sample values. Carried, not checked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

/// One way a block's spec breaks its role's contract. Every variant names the role, and the column
/// where there is one. Raised inside [`crate::Error::Contract`].
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum ContractViolation {
    /// The schema declares no contract for this role and block kind ([`BlockKind::Table`] from
    /// `check_table`, [`BlockKind::Array`] from `check_array`).
    #[error("no {} role '{role}' is declared", kind_word(*kind))]
    UnknownRole { kind: BlockKind, role: String },
    /// The block has a column the role does not declare.
    #[error("table '{role}' has column '{column}', which the contract does not declare")]
    UnknownColumn { role: String, column: String },
    /// The block has the same column twice.
    #[error("table '{role}' has column '{column}' more than once")]
    DuplicateColumn { role: String, column: String },
    /// A column the contract declares non-optional is absent.
    #[error("table '{role}' is missing the non-optional column '{column}'")]
    MissingColumn { role: String, column: String },
    /// A declared column is present but not in the contract's order. `block_position` is the
    /// column's 0-based index in the block's spec, not in the contract.
    #[error(
        "table '{role}' has column '{column}' at block position {block_position}, out of the contract's order"
    )]
    ColumnOrder {
        role: String,
        column: String,
        block_position: usize,
    },
    /// A column's dtype differs from the declared one.
    #[error(
        "table '{role}' column '{column}' has dtype '{found}', the contract declares '{declared}'"
    )]
    ColumnDtype {
        role: String,
        column: String,
        declared: String,
        found: String,
    },
    /// A column's nullability differs from the declared one, in either direction.
    #[error(
        "table '{role}' column '{column}' has nullable = {found}, the contract declares {declared}"
    )]
    ColumnNullability {
        role: String,
        column: String,
        declared: bool,
        found: bool,
    },
    /// An array's dtype differs from the declared one.
    #[error("array '{role}' has dtype '{found}', the contract declares '{declared}'")]
    ArrayDtype {
        role: String,
        declared: String,
        found: String,
    },
    /// An array's rank differs from the declared one.
    #[error("array '{role}' has rank {found}, the contract declares {declared}")]
    ArrayRank {
        role: String,
        declared: u32,
        found: u32,
    },
    /// An array's axis names differ from the declared ones.
    #[error("array '{role}' has axes {found:?}, the contract declares {declared:?}")]
    ArrayAxes {
        role: String,
        declared: Vec<String>,
        found: Vec<String>,
    },
}

impl TableContract {
    /// Check a table block's columns against this contract (see
    /// [`ProductSchema::check_table`](super::ProductSchema::check_table)).
    pub(crate) fn check(&self, role: &str, spec: &TableSpec) -> Result<(), ContractViolation> {
        let v = |make: fn(String, String) -> ContractViolation, column: &str| {
            Err(make(role.to_owned(), column.to_owned()))
        };
        let declared = |name: &str| self.columns.iter().any(|c| c.name == name);
        for (i, c) in spec.columns.iter().enumerate() {
            if !declared(&c.name) {
                return v(
                    |role, column| ContractViolation::UnknownColumn { role, column },
                    &c.name,
                );
            }
            if spec.columns[..i].iter().any(|p| p.name == c.name) {
                return v(
                    |role, column| ContractViolation::DuplicateColumn { role, column },
                    &c.name,
                );
            }
        }
        // Every spec column is declared and unique. Walk the contract in order: each declared
        // column is either the next spec column, or optional and absent here.
        let mut next = 0;
        for d in &self.columns {
            match spec.columns.get(next) {
                Some(s) if s.name == d.name => {
                    check_column(role, d, s)?;
                    next += 1;
                }
                _ => match spec.columns.iter().position(|s| s.name == d.name) {
                    Some(block_position) => {
                        return Err(ContractViolation::ColumnOrder {
                            role: role.to_owned(),
                            column: d.name.clone(),
                            block_position,
                        })
                    }
                    None if d.optional => {}
                    None => {
                        return v(
                            |role, column| ContractViolation::MissingColumn { role, column },
                            &d.name,
                        )
                    }
                },
            }
        }
        Ok(())
    }
}

/// The word a refusal message uses for a block kind.
fn kind_word(kind: BlockKind) -> &'static str {
    match kind {
        BlockKind::Array => "array",
        BlockKind::Table => "table",
        BlockKind::ChunkIndex => "chunk-index",
        BlockKind::Blob => "blob",
    }
}

fn check_column(role: &str, declared: &Column, found: &Column) -> Result<(), ContractViolation> {
    if declared.dtype != found.dtype {
        return Err(ContractViolation::ColumnDtype {
            role: role.to_owned(),
            column: found.name.clone(),
            declared: declared.dtype.clone(),
            found: found.dtype.clone(),
        });
    }
    if declared.nullable != found.nullable {
        return Err(ContractViolation::ColumnNullability {
            role: role.to_owned(),
            column: found.name.clone(),
            declared: declared.nullable,
            found: found.nullable,
        });
    }
    Ok(())
}

impl ArrayContract {
    /// Check an array block's spec against this contract (see
    /// [`ProductSchema::check_array`](super::ProductSchema::check_array)).
    pub(crate) fn check(&self, role: &str, spec: &ArraySpec) -> Result<(), ContractViolation> {
        if self.dtype != spec.dtype {
            return Err(ContractViolation::ArrayDtype {
                role: role.to_owned(),
                declared: self.dtype.clone(),
                found: spec.dtype.clone(),
            });
        }
        // A rank beyond u32 cannot equal a declared u32 rank; saturate rather than wrap.
        let found = u32::try_from(spec.shape.len()).unwrap_or(u32::MAX);
        if self.rank != found {
            return Err(ContractViolation::ArrayRank {
                role: role.to_owned(),
                declared: self.rank,
                found,
            });
        }
        if !self.axes.is_empty() && self.axes != spec.axes {
            return Err(ContractViolation::ArrayAxes {
                role: role.to_owned(),
                declared: self.axes.clone(),
                found: spec.axes.clone(),
            });
        }
        Ok(())
    }
}
