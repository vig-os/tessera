//! The generic-ingest **canonicalisation boundary** (ADR-0056 §2/§5, ADR-0057 §1) — the one place a
//! foreign type system becomes Tessera's.
//!
//! # Where this sits
//!
//! ADR-0057 §1 locates the thin waist at the pair (`ColumnData`, `ArrayData`) in `tessera-io`, with
//! **canonicalisation upstream of it, here in `tessera-ingest`**. So the pipeline is:
//!
//! ```text
//! source bytes ──▶ decoder (parquet / arrow-ipc / csv) ──▶ THIS MODULE ──▶ ColumnData ──▶ Vortex ──▶ seal
//!                  ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^      ^^^^^^^^^^^     ^^^^^^^^^^   ^^^^^^^^^^^^^^^^
//!                  third-party, feature-gated              ours            the waist     tessera-io
//! ```
//!
//! # Why canonicalisation is the determinism gate
//!
//! Tessera's own codecs are already cross-arch byte-deterministic and gated by the conformance
//! corpus. Generic ingest moves the risk **upstream**: two arrow-rs versions can hand the primitive
//! two different `ColumnData` for the same Parquet file, and both then encode deterministically to
//! two *different* `content_hash`es. So the gate is not our encode — it is this boundary, and naming
//! it is half the fix (ADR-0056 §5).
//!
//! The hazards this module closes, by number (the full table is in ADR-0056 §5):
//!
//! - **H4 — NaN payload bits.** IEEE-754 lets a `NaN` carry any payload, and producers differ; the
//!   payload carries no numeric information, so it is producer noise that would otherwise reach
//!   `content_hash`. Every `NaN` is rewritten to the quiet default at the boundary
//!   ([`canonicalise_floats`]). **`-0.0` is deliberately NOT touched** — it is a legitimate measured
//!   value, and the S13 clinical gate guarantees bit-exact round-trip of `-0.0` specifically.
//! - **H5 — values under a NULL.** A foreign source carries whatever the writer happened to leave in
//!   a masked slot. `ColumnData::Nullable` already normalises masked slots on *encode*, but that is
//!   too late for anything inspecting values in between, so the normalisation is pulled forward to
//!   here ([`normalise_null_slots`]).
//! - **H8 — locale.** Nothing in this module consults a locale: numbers are parsed by Rust std's
//!   `str::parse`, which is correctly-rounded and locale-independent (unlike C `strtod`).
//!
//! Both H4 and H5 are **recorded** in the seal when they actually fire, not merely performed — a
//! transform list that says `null_slot_normalisation` happened is a receipt, and ADR-0056 §2 rejected
//! the alternative of printing a warning that scrolls away.
//!
//! # What "flat" means, precisely
//!
//! The boundary is `ColumnData`: `I8…I64`, `U8…U64`, `F32`, `F64`, `Bool`, `Utf8`, plus a
//! non-nestable `Nullable` wrapper. No List, Struct, Map, Decimal, Date, Timestamp or Dictionary.
//! Everything a source can carry either maps into that set cleanly, maps into it with a recorded
//! transform, or is rejected with a runnable next command — there is no lossy lane (ADR-0056 §2).

use std::collections::BTreeMap;

use tessera_core::block::table::{Column, TableSpec};
use tessera_core::manifest::Manifest;
use tessera_core::schema::Sensitivity;
use tessera_core::{Error, IngestTransform, ProductBuilder, Result};
use tessera_io::table::{ColumnData, TableData};
use tessera_io::BlockPayload;

/// The block name every generic-ingest product stores its payload under.
///
/// Fixed, not derived from the source filename: the builtin `table` / `array` schemas declare a
/// `"data"` block role (`tessera_core::schema`), and a reader projecting a column asks for
/// `blocks/data`. Deriving it from the input would make `content_hash` a function of the *filename*.
pub const GENERIC_BLOCK: &str = "data";

/// Transform names recorded in the seal (ADR-0056 §6.2). Constants rather than string literals at
/// the call sites, because these names are **sealed data** — a typo is a permanent, unqueryable
/// record, and a rename is a format change.
pub mod transform {
    /// A `Float16` source column widened to `F32` — exact for every finite value.
    pub const F16_WIDEN: &str = "f16_widen";
    /// A `Decimal128(p ≤ 18, s)` source column carried as `I64` + `Column.scale = 10⁻ˢ`.
    pub const DECIMAL_FIXED_POINT: &str = "decimal_fixed_point";
    /// A dictionary/categorical column materialised to its values, dropping the source's codes.
    pub const DICTIONARY_MATERIALISED: &str = "dictionary_materialised";
    /// A run-end-encoded column decoded through to its logical values.
    pub const RUN_END_DECODED: &str = "run_end_decoded";
    /// A `FixedSizeList<scalar, N>` expanded into `N` columns named `parent.0 … parent.N-1`.
    pub const FIXED_LIST_EXPAND: &str = "fixed_list_expand";
    /// A `Struct` flattened into dotted columns — a renaming, not a semantic change.
    pub const STRUCT_FLATTEN: &str = "struct_flatten";
    /// An absolute-instant column carried as integer ticks anchored to a named epoch.
    pub const EPOCH_ANCHOR: &str = "epoch_anchor";
    /// A zone-annotated timestamp carried as UTC ticks, with the source zone sealed.
    pub const TZ_TO_UTC: &str = "tz_to_utc";
    /// Masked-out slots zeroed at the boundary (ADR-0056 §5 H5).
    pub const NULL_SLOT_NORMALISATION: &str = "null_slot_normalisation";
    /// `NaN` payload bits rewritten to the quiet default (ADR-0056 §5 H4).
    pub const NAN_CANONICALISATION: &str = "nan_canonicalisation";
    /// A CSV read under an operator-declared schema — never an inferred one (ADR-0056 §8).
    pub const CSV_EXPLICIT_SCHEMA: &str = "csv_explicit_schema";
    /// Field texts an operator declared as NULL in a CSV (`--null-token`). Recorded because it changes
    /// what the values mean: a reader comparing back to the source cannot otherwise tell a NULL from
    /// the literal string `NA`.
    pub const CSV_NULL_TOKENS: &str = "csv_null_tokens";
    /// A Fortran-ordered (column-major) source buffer rewritten to C order (ADR-0056 §11).
    pub const FORTRAN_TO_C_ORDER: &str = "fortran_to_c_order";
    /// An interleaved record array de-interleaved into columns — the #193 transpose, one format over.
    pub const RECORD_DEINTERLEAVE: &str = "record_deinterleave";
}

fn he(e: impl std::fmt::Display) -> Error {
    Error::Invalid(format!("ingest: {e}"))
}

/// A canonicalised generic table: flat columns ready to hand to the waist, plus the **receipt** of
/// every transformation applied to get there.
///
/// Built through [`TableBuilder`], which is what enforces the invariants (unique names, equal row
/// counts, no nested `Nullable`) rather than leaving them to each backend to remember.
#[derive(Debug, Default)]
pub struct CanonicalTable {
    /// Columns in source order. Order is part of identity — the Merkle root is over the encoded
    /// block, and the block encodes columns in this order — so a backend must never reorder.
    pub columns: Vec<(Column, ColumnData)>,
    /// Deduplicated, name-then-column ordered transform records (ADR-0056 §6.2).
    pub transforms: Vec<IngestTransform>,
}

impl CanonicalTable {
    /// Column names, in source order — the input the name-pattern checks work from.
    pub fn column_names(&self) -> Vec<&str> {
        self.columns.iter().map(|(c, _)| c.name.as_str()).collect()
    }

    /// Row count — the shared length every column is checked against at insert time.
    pub fn rows(&self) -> usize {
        self.columns.first().map(|(_, d)| d.len()).unwrap_or(0)
    }

    /// The `TableSpec` describing these columns, with the declared `rows`.
    pub fn spec(&self) -> Result<TableSpec> {
        Ok(TableSpec {
            columns: self.columns.iter().map(|(c, _)| c.clone()).collect(),
            rows: u64::try_from(self.rows()).map_err(|e| he(format!("row count overflow: {e}")))?,
            // No secondary index: a generic table has no known monotone key, and inventing one
            // would be a semantic guess (ADR-0056 §1). An operator who knows better attaches it
            // with the metadata-edit path.
            row_index: None,
        })
    }

    /// The payload data, in column order.
    pub fn data(&self) -> TableData {
        self.columns
            .iter()
            .map(|(c, d)| (c.name.clone(), d.clone()))
            .collect()
    }
}

/// Accumulates canonicalised columns while enforcing the boundary's invariants.
///
/// Every generic backend goes through this rather than pushing onto a `Vec` itself, so the rules are
/// stated once: a name collision is an **error** (ADR-0056 §3 — never a silent suffix), every column
/// has the same length, and a transform is recorded once no matter how many columns triggered it.
#[derive(Debug, Default)]
pub struct TableBuilder {
    columns: Vec<(Column, ColumnData)>,
    /// Keyed by `(name, column-or-empty)` so the same transform on two columns records twice but the
    /// same transform on the same column records once. `BTreeMap` ⇒ deterministic emission order,
    /// which matters because this list is sealed.
    transforms: BTreeMap<(String, String), IngestTransform>,
}

impl TableBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a canonicalised column.
    ///
    /// Errors on a duplicate name or a length mismatch. Both are producer bugs that would otherwise
    /// surface as a corrupt-looking payload much later: a duplicate name makes column projection
    /// ambiguous, and a short column makes `TableSpec.rows` a lie.
    pub fn push(&mut self, column: Column, data: ColumnData) -> Result<()> {
        if let Some((existing, _)) = self.columns.iter().find(|(c, _)| c.name == column.name) {
            return Err(he(format!(
                "duplicate column name '{}' after flattening (a struct flatten is a renaming, and \
                 Tessera will not silently suffix one away — rename the source column, or drop one \
                 with --exclude {})",
                existing.name, existing.name
            )));
        }
        // The table backend carries a validity mask for the fixed-width numeric dtypes only
        // (`tessera-io::table::validate_nullable`). A nullable `str`/`b1` column is therefore
        // unrepresentable *today* — a limitation of the encoder, not a format decision — and it is
        // caught here rather than left to surface from the codec, because only the boundary knows the
        // column's name and can offer something to do about it.
        if let ColumnData::Nullable { values, .. } = &data {
            if matches!(**values, ColumnData::Bool(_) | ColumnData::Utf8(_)) {
                return Err(he(format!(
                    "column '{}' is a nullable '{}', which the table backend cannot encode yet \
                     (validity masks currently cover the fixed-width numeric dtypes only; #457).\n  \
                     drop it:              tessera ingest table <FILE> --exclude {}\n  \
                     or fill the nulls in the source first, if a neutral value is meaningful for \
                     this column\n  \
                     keep every byte:      tessera ingest blob <FILE>",
                    column.name,
                    values.numpy_code(),
                    column.name
                )));
            }
        }
        if let Some(expected) = self.columns.first().map(|(_, d)| d.len()) {
            if data.len() != expected {
                return Err(he(format!(
                    "column '{}' has {} rows but the table has {expected} (a ragged table cannot be \
                     sealed — TableSpec.rows is a single number)",
                    column.name,
                    data.len()
                )));
            }
        }
        self.columns.push((column, data));
        Ok(())
    }

    /// Record a transform that applies to the whole table (no single column).
    pub fn record(&mut self, transform: IngestTransform) {
        let column = transform
            .params
            .get("column")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        self.transforms
            .entry((transform.name.clone(), column))
            .or_insert(transform);
    }

    /// Finish, yielding the canonical table and its ordered transform receipt.
    pub fn finish(self) -> CanonicalTable {
        CanonicalTable {
            columns: self.columns,
            transforms: self.transforms.into_values().collect(),
        }
    }

    /// Whether a column of this name is already present — the pre-check a struct flatten uses to
    /// report the *source* path rather than the flattened one.
    pub fn has(&self, name: &str) -> bool {
        self.columns.iter().any(|(c, _)| c.name == name)
    }
}

/// **H4** — rewrite every `NaN` payload to the quiet default, in place.
///
/// Returns `true` if any value was rewritten, which is what makes the recorded transform a receipt
/// rather than a blanket claim: the overwhelmingly common float column contains no `NaN` at all and
/// records nothing.
///
/// `-0.0` is left alone, on purpose. ADR-0056 §5 groups it with `NaN` under H4, but the two are not
/// alike: a `NaN` payload is uninterpreted bits that IEEE-754 explicitly leaves to the producer,
/// while `-0.0` is a distinguishable value whose bit-exact round-trip the S13 clinical gate
/// guarantees. Normalising it would be the kind of silent value change §2 rejects.
pub fn canonicalise_floats(data: &mut ColumnData) -> bool {
    match data {
        ColumnData::F32(v) => {
            let mut hit = false;
            for x in v.iter_mut() {
                if x.is_nan() {
                    *x = f32::NAN;
                    hit = true;
                }
            }
            hit
        }
        ColumnData::F64(v) => {
            let mut hit = false;
            for x in v.iter_mut() {
                if x.is_nan() {
                    *x = f64::NAN;
                    hit = true;
                }
            }
            hit
        }
        ColumnData::Nullable { values, .. } => canonicalise_floats(values),
        _ => false,
    }
}

/// **H5** — zero every masked-out slot at the boundary, in place.
///
/// `ColumnData::Nullable` already normalises masked slots on **encode**, which is what makes
/// `content_hash` independent of whatever sat under a null. That stays; this pulls the same
/// normalisation *forward*, because a foreign source genuinely does carry bytes under its nulls and
/// anything reading `values` before encode would otherwise see producer noise.
///
/// The consequence must be stated plainly wherever it is documented, and is (ADR-0056 §5): **null
/// semantics after ingest are Tessera's, not the source's.** A Parquet writer that stored `NaN` under
/// a nullable `f64` slot reads back `0.0` with `validity = false`. Returns `true` if the column was
/// nullable at all, since that is exactly when the transform is worth recording.
pub fn normalise_null_slots(data: &mut ColumnData) -> bool {
    let ColumnData::Nullable { values, validity } = data else {
        return false;
    };
    macro_rules! zero {
        ($v:expr, $zero:expr) => {{
            for (i, slot) in $v.iter_mut().enumerate() {
                if !validity.get(i).copied().unwrap_or(false) {
                    *slot = $zero;
                }
            }
        }};
    }
    match &mut **values {
        ColumnData::I8(v) => zero!(v, 0),
        ColumnData::I16(v) => zero!(v, 0),
        ColumnData::I32(v) => zero!(v, 0),
        ColumnData::I64(v) => zero!(v, 0),
        ColumnData::U8(v) => zero!(v, 0),
        ColumnData::U16(v) => zero!(v, 0),
        ColumnData::U32(v) => zero!(v, 0),
        ColumnData::U64(v) => zero!(v, 0),
        ColumnData::F32(v) => zero!(v, 0.0),
        ColumnData::F64(v) => zero!(v, 0.0),
        ColumnData::Bool(v) => zero!(v, false),
        ColumnData::Utf8(v) => zero!(v, String::new()),
        // `validate_nullable` rejects nesting, so this arm is unreachable through any public
        // constructor; returning early rather than panicking keeps the boundary total.
        ColumnData::Nullable { .. } => {}
    }
    true
}

/// Apply both value-canonicalisation rules to a column and record whichever fired.
///
/// One call site for the pair, so a new backend cannot forget one of them: the ordering matters
/// (nulls are zeroed first, so a `NaN` that sat *under* a null is gone before the `NaN` scan runs and
/// does not produce a spurious `nan_canonicalisation` record).
pub fn canonicalise(b: &mut TableBuilder, name: &str, data: &mut ColumnData) {
    if normalise_null_slots(data) {
        b.record(IngestTransform::new(transform::NULL_SLOT_NORMALISATION).on_column(name));
    }
    if canonicalise_floats(data) {
        b.record(IngestTransform::new(transform::NAN_CANONICALISATION).on_column(name));
    }
}

/// Promote a column to [`ColumnData::Nullable`] with an all-present validity mask.
///
/// The half of [`concat_columns`] that makes the nullable-by-presence rule composable: a source whose
/// first chunk has no nulls and whose second does must end up with one nullable column, not an error.
pub(crate) fn promote_nullable(data: ColumnData) -> ColumnData {
    match data {
        already @ ColumnData::Nullable { .. } => already,
        values => {
            let validity = vec![true; values.len()];
            ColumnData::Nullable {
                values: Box::new(values),
                validity,
            }
        }
    }
}

/// Append `next` to `acc`, promoting to [`ColumnData::Nullable`] if either side is.
///
/// Chunked sources (Parquet row groups, Arrow IPC record batches) hand over one array per chunk, and
/// **nullability is decided per chunk by the data, not by the schema** (see
/// [`crate::arrow_table`]'s boundary rules). So a column whose third row group is the only one
/// containing a null must still seal as one nullable column — which means the merge, not the
/// per-chunk mapping, is where the promotion happens.
pub fn concat_columns(name: &str, acc: ColumnData, next: ColumnData) -> Result<ColumnData> {
    // If either side carries a mask, both must, so the merged validity is total.
    if matches!(acc, ColumnData::Nullable { .. }) || matches!(next, ColumnData::Nullable { .. }) {
        let acc = promote_nullable(acc);
        let next = promote_nullable(next);
        let (
            ColumnData::Nullable {
                values: av,
                validity: mut amask,
            },
            ColumnData::Nullable {
                values: bv,
                validity: bmask,
            },
        ) = (acc, next)
        else {
            unreachable!("both sides were just promoted to Nullable");
        };
        amask.extend(bmask);
        return Ok(ColumnData::Nullable {
            values: Box::new(concat_columns(name, *av, *bv)?),
            validity: amask,
        });
    }
    macro_rules! join {
        ($a:expr, $b:expr, $variant:path) => {{
            let mut a = $a;
            a.extend($b);
            $variant(a)
        }};
    }
    let merged = match (acc, next) {
        (ColumnData::I8(a), ColumnData::I8(b)) => join!(a, b, ColumnData::I8),
        (ColumnData::I16(a), ColumnData::I16(b)) => join!(a, b, ColumnData::I16),
        (ColumnData::I32(a), ColumnData::I32(b)) => join!(a, b, ColumnData::I32),
        (ColumnData::I64(a), ColumnData::I64(b)) => join!(a, b, ColumnData::I64),
        (ColumnData::U8(a), ColumnData::U8(b)) => join!(a, b, ColumnData::U8),
        (ColumnData::U16(a), ColumnData::U16(b)) => join!(a, b, ColumnData::U16),
        (ColumnData::U32(a), ColumnData::U32(b)) => join!(a, b, ColumnData::U32),
        (ColumnData::U64(a), ColumnData::U64(b)) => join!(a, b, ColumnData::U64),
        (ColumnData::F32(a), ColumnData::F32(b)) => join!(a, b, ColumnData::F32),
        (ColumnData::F64(a), ColumnData::F64(b)) => join!(a, b, ColumnData::F64),
        (ColumnData::Bool(a), ColumnData::Bool(b)) => join!(a, b, ColumnData::Bool),
        (ColumnData::Utf8(a), ColumnData::Utf8(b)) => join!(a, b, ColumnData::Utf8),
        (a, b) => {
            return Err(he(format!(
                "column '{name}': cannot concatenate a '{}' chunk onto a '{}' column — the source \
                 changed this column's dtype between chunks, which no single sealed column can \
                 represent",
                b.numpy_code(),
                a.numpy_code()
            )))
        }
    };
    Ok(merged)
}

/// Everything a generic backend must supply to seal its product, in one place.
///
/// A struct rather than a dozen positional arguments because every generic backend passes the same
/// set and a positional seam of this width is where a `source_label` silently ends up in the `name`
/// slot. The lifetimes are all borrows — nothing here is owned.
pub struct GenericIngest<'a> {
    /// Product name (the human handle, and an identity input).
    pub name: &'a str,
    /// Acquisition timestamp — from the operator or the spec, **never** the clock or an mtime, or
    /// re-running the same ingest would produce a different `id`.
    pub timestamp: &'a str,
    /// One-line product description.
    pub description: &'a str,
    /// The source container this was normalised from (`"parquet"` | `"arrow"` | `"csv"` | …), landing
    /// in the builtin schema's recommended `source_format` field.
    pub source_format: &'a str,
    /// The file the bytes were read from — hashed into the `ingested_from` edge.
    pub source_path: &'a std::path::Path,
    /// ADR-0040 PHI hygiene: replaces the input **path** in the sealed edge (an absolute path on
    /// clinical data is itself PHI). The path is still used to read and to digest the bytes.
    pub source_label: Option<&'a str>,
    /// Edges the declarative engine threads in (`derived_from`, `ingested_via_spec`), after
    /// `ingested_from`.
    pub extra_sources: &'a [tessera_core::provenance::Source],
    /// Which decoder read the bytes — sealed into the recipe bag as ADR-0056 §6a's triple.
    pub decoder: crate::decoder::Decoder,
    /// The operator's own recipe, if the spec supplied one. The decoder record is added to it, never
    /// over it.
    pub generation: Option<tessera_core::Generation>,
    /// Operator-supplied per-column semantics (ADR-0056 §7's `--column-meta`), applied to the
    /// columns before seal.
    pub column_meta: &'a crate::column_meta::ColumnMeta,
}

/// Seal a canonicalised table as a generic `table` product (ADR-0056 §7's builtin schema).
/// The manifest facts a generic-table ingest declares, independent of how its blocks get written.
///
/// Two writers implement it — [`ProductBuilder`] for the batch path and `tessera_io::WriteSession` for
/// the streaming one — so [`declare_generic_table`] is the single place either path decides what a
/// `table` product's manifest says. That matters because the two must seal the *same* manifest for the
/// same input (#458), and a twin maintained by hand is exactly the drift this codebase keeps finding: a
/// fact added to one and forgotten in the other moves `manifest_hash` while leaving `content_hash`
/// identical, which a content-hash comparison cannot see. Add a fact here and both paths get it; add it
/// to one writer only and it does not compile.
pub trait DeclareTable {
    fn declare_field(&mut self, id: &str, value: serde_json::Value) -> Result<()>;
    fn declare_generation(&mut self, generation: tessera_core::Generation) -> Result<()>;
    fn declare_ingest_transform(&mut self, transforms: Vec<IngestTransform>) -> Result<()>;
    fn declare_source(&mut self, source: tessera_core::provenance::Source) -> Result<()>;
    fn declare_study(&mut self, study: &str) -> Result<()>;
    fn declare_producer(&mut self, producer: tessera_core::Producer) -> Result<()>;
}

impl DeclareTable for ProductBuilder {
    fn declare_field(&mut self, id: &str, value: serde_json::Value) -> Result<()> {
        self.with_field(id, value);
        Ok(())
    }
    fn declare_generation(&mut self, generation: tessera_core::Generation) -> Result<()> {
        self.with_generation(generation);
        Ok(())
    }
    fn declare_ingest_transform(&mut self, transforms: Vec<IngestTransform>) -> Result<()> {
        self.with_ingest_transform(transforms);
        Ok(())
    }
    fn declare_source(&mut self, source: tessera_core::provenance::Source) -> Result<()> {
        self.add_source(source);
        Ok(())
    }
    fn declare_study(&mut self, study: &str) -> Result<()> {
        self.with_study(study);
        Ok(())
    }
    fn declare_producer(&mut self, producer: tessera_core::Producer) -> Result<()> {
        self.with_producer(producer);
        Ok(())
    }
}

impl DeclareTable for tessera_io::WriteSession {
    fn declare_field(&mut self, id: &str, value: serde_json::Value) -> Result<()> {
        self.with_field(id, value)?;
        Ok(())
    }
    fn declare_generation(&mut self, generation: tessera_core::Generation) -> Result<()> {
        self.with_generation(generation)?;
        Ok(())
    }
    fn declare_ingest_transform(&mut self, transforms: Vec<IngestTransform>) -> Result<()> {
        self.with_ingest_transform(transforms)?;
        Ok(())
    }
    fn declare_source(&mut self, source: tessera_core::provenance::Source) -> Result<()> {
        self.add_source(source)?;
        Ok(())
    }
    fn declare_study(&mut self, study: &str) -> Result<()> {
        self.with_study(study)?;
        Ok(())
    }
    fn declare_producer(&mut self, producer: tessera_core::Producer) -> Result<()> {
        self.with_producer(producer)?;
        Ok(())
    }
}

/// Declare every generic-table manifest fact on `w`, in the order the seal depends on.
///
/// `transforms` is passed separately from `opts` because the streaming path accumulates it across
/// batches (its receipt is only complete once the last one is read) while the batch path has it on the
/// folded table. Everything else is identical by construction.
pub fn declare_generic_table(
    w: &mut impl DeclareTable,
    opts: &GenericIngest<'_>,
    transforms: &[IngestTransform],
) -> Result<()> {
    w.declare_field(
        "source_format",
        serde_json::Value::String(opts.source_format.to_string()),
    )?;
    w.declare_generation(opts.decoder.record_into(opts.generation.clone()))?;
    // Empty stays ABSENT rather than an empty list: a product where nothing fired must seal exactly as
    // it did before the receipt existed.
    if !transforms.is_empty() {
        w.declare_ingest_transform(transforms.to_vec())?;
    }
    // ADR-0040: `source_label` replaces the path in the sealed edge (an absolute clinical path is itself
    // PHI); the bytes are still read from, and digested at, the real path.
    let source_ref = opts
        .source_label
        .map(str::to_string)
        .unwrap_or_else(|| opts.source_path.display().to_string());
    w.declare_source(crate::provenance::ingested_from(
        &[opts.source_path],
        source_ref,
    )?)?;
    // Order matters for the seal: `ingested_from` first, then whatever the engine threaded in.
    for s in opts.extra_sources {
        w.declare_source(s.clone())?;
    }
    Ok(())
}

/// The metadata tiers a `table` product's manifest layers, in **ascending** precedence.
///
/// Separate from [`GenericIngest`] (what the lane itself knows) because these come from the *spec*
/// and from its resolved parents, and held as three fields rather than one pre-merged map because
/// they must be **layered, not merged**: a value inherited from a parent must never clobber a
/// product-own default, and a pre-merged map cannot express that difference.
///
/// The streaming path has no choice about applying these before the first block commits. A streamed
/// `.tsra` is written once, straight to disk, with no post-seal hook — so a tier plumbed any later
/// is dropped **silently**, and that moves `manifest_hash` while leaving `content_hash` identical,
/// which is exactly the divergence class a content-hash comparison cannot see. The batch path
/// reaches the same manifest from the other side (`engine::apply_spec_metadata` re-seals a built
/// product), and the batch-equals-stream tests over BOTH hashes are what hold the two together.
///
/// **There is no `generation` field, deliberately.** A table's recipe already arrives on
/// [`GenericIngest::generation`], where [`declare_generic_table`] folds the ADR-0056 §6a decoder
/// triple into it. A second generation tier here would make two writers of one field, which is how
/// that triple came to be deleted by a spec-declared recipe in the first place.
#[derive(Debug, Default, Clone, Copy)]
pub struct MetadataTiers<'a> {
    /// Schema-flagged identity inherited from the `derived_from` parents (ADR-0058 §5) — the
    /// LOWEST tier, applied before the lane's own fields so a product-own default overrides it.
    pub inherited: Option<&'a BTreeMap<String, serde_json::Value>>,
    /// The parents' first-class `study` grouping key, when this product declares none of its own.
    pub inherited_study: Option<&'a str>,
    /// The spec's `[product.metadata]` — the HIGHEST tier; an explicit operator value wins over
    /// both the inherited identity and the lane's own default.
    pub metadata: Option<&'a BTreeMap<String, serde_json::Value>>,
    /// The spec's `[product.producer]` identity (ADR-0058 §1) — who/what made this product.
    pub producer: Option<&'a tessera_core::Producer>,
}

/// Declare every `table` manifest fact **and** the spec's metadata tiers, in the one order that is
/// correct.
///
/// One function rather than a before-hook and an after-hook, because the tiers are only right
/// relative to [`declare_generic_table`]: the inherited floor has to go down first so the lane's own
/// `source_format` survives it, and the spec's overrides have to go on last so an operator value
/// wins. Two functions would make that ordering a convention a caller could get wrong, which on this
/// path means a silently different `manifest_hash`.
pub fn declare_table_with_tiers(
    w: &mut impl DeclareTable,
    opts: &GenericIngest<'_>,
    transforms: &[IngestTransform],
    tiers: &MetadataTiers<'_>,
) -> Result<()> {
    // (1) INHERITED — lowest. `study` is a first-class field, not metadata.
    if let Some(s) = tiers.inherited_study {
        w.declare_study(s)?;
    }
    for (k, v) in tiers.inherited.into_iter().flatten() {
        w.declare_field(k, v.clone())?;
    }
    // (2) The lane's own facts, including any product-own default — beats inherited, loses to spec.
    declare_generic_table(w, opts, transforms)?;
    // (3) SPEC `[product.metadata]` — highest, so an explicit operator value wins over everything.
    for (k, v) in tiers.metadata.into_iter().flatten() {
        w.declare_field(k, v.clone())?;
    }
    if let Some(pr) = tiers.producer {
        w.declare_producer(pr.clone())?;
    }
    Ok(())
}

pub fn to_table_product(
    table: &CanonicalTable,
    opts: &GenericIngest<'_>,
) -> Result<(Manifest, Vec<BlockPayload>)> {
    let mut spec = table.spec()?;
    // Operator semantics land INSIDE the seal, which is the half of §7 that keeps generic ingest from
    // being a wrapper: "annotate it later" is a real path (the content-addressed metadata edit), but
    // it must not be the only one.
    opts.column_meta.apply(&mut spec.columns)?;
    let data = table.data();
    let (block_ref, payload) = tessera_io::table::table_block(GENERIC_BLOCK, &spec, &data)?;
    let mut b = ProductBuilder::new("table", opts.name, opts.description, opts.timestamp);
    b.add_block_ref(block_ref);
    // Every manifest fact goes through the shared applier, so the streaming path cannot declare a
    // different set (#458).
    declare_generic_table(&mut b, opts, &table.transforms)?;
    Ok((b.seal()?, vec![payload]))
}

/// Seal a canonicalised dense grid as a generic `array` product (ADR-0056 §7's builtin schema).
///
/// The array twin of [`to_table_product`], and deliberately the same shape: the same `GenericIngest`
/// options, the same sealed decoder record, the same `ingested_from` edge, the same `source_format`
/// field. ADR-0057 §1's note that "the array lane needs no `arrow` dependency at all" is why the two
/// are separate functions rather than one generic over the primitive — but everything *around* the
/// payload has to stay identical, or the two lanes would drift into producing differently-shaped
/// products for the same operator input.
///
/// `transforms` comes from the decoder rather than a `CanonicalTable`, because an array has no
/// per-column boundary to accumulate them at — the whole grid is one value space.
pub fn to_array_product(
    spec: &tessera_core::block::array::ArraySpec,
    data: &tessera_io::array::ArrayData,
    transforms: &[IngestTransform],
    opts: &GenericIngest<'_>,
) -> Result<(Manifest, Vec<BlockPayload>)> {
    if !opts.column_meta.is_empty() {
        // `--column-meta` annotates COLUMNS. Silently ignoring it on an array would let an operator
        // believe they had classified something (ADR-0056 §7's whole concern), so it is an error that
        // names the alternative.
        return Err(he(
            "--column-meta describes table columns and an array has none; annotate the array's \
             sample values instead with the array spec's own unit/scale, or attach product metadata \
             with --meta",
        ));
    }
    let (block_ref, payload) = tessera_io::array::array_block(GENERIC_BLOCK, spec, data)?;
    let mut b = ProductBuilder::new("array", opts.name, opts.description, opts.timestamp);
    b.add_block_ref(block_ref);
    b.with_field(
        "source_format",
        serde_json::Value::String(opts.source_format.to_string()),
    );
    b.with_generation(opts.decoder.record_into(opts.generation.clone()));
    if !transforms.is_empty() {
        b.with_ingest_transform(transforms.to_vec());
    }
    let source_ref = opts
        .source_label
        .map(str::to_string)
        .unwrap_or_else(|| opts.source_path.display().to_string());
    b.add_source(crate::provenance::ingested_from(
        &[opts.source_path],
        source_ref,
    )?);
    for s in opts.extra_sources {
        b.add_source(s.clone());
    }
    Ok((b.seal()?, vec![payload]))
}

/// Parse a `name:dtype` column declaration (the CSV lane's `--column`, ADR-0056 §8).
///
/// Accepts the fd5 numpy-style codes the table backend already speaks (`i1 i2 i4 i8`, `u1 u2 u4 u8`,
/// `f4 f8`, `b1`, `str`) plus a `?` suffix marking the column nullable (`age:i4?`). Deliberately a
/// small closed vocabulary: this is the operator's *assertion* about the file, and an assertion in a
/// vocabulary nobody can enumerate is not checkable.
pub fn parse_column_decl(decl: &str) -> Result<(String, String, bool)> {
    let (name, dtype) = decl.split_once(':').ok_or_else(|| {
        he(format!(
            "column declaration '{decl}' is not NAME:DTYPE (e.g. --column energy:f8, \
             --column label:str, --column age:i4? for nullable)"
        ))
    })?;
    if name.is_empty() {
        return Err(he(format!("column declaration '{decl}' has an empty name")));
    }
    let (dtype, nullable) = match dtype.strip_suffix('?') {
        Some(base) => (base, true),
        None => (dtype, false),
    };
    if !ACCEPTED_DTYPES.contains(&dtype) {
        return Err(he(format!(
            "column '{name}' declares dtype '{dtype}', which Tessera tables do not carry.\n  \
             accepted: {}\n  (append '?' to allow NULLs, e.g. {name}:{}?)",
            ACCEPTED_DTYPES.join(" · "),
            ACCEPTED_DTYPES[0]
        )));
    }
    Ok((name.to_string(), dtype.to_string(), nullable))
}

/// The flat dtype vocabulary a generic table column may declare — `ColumnData`'s own set, minus the
/// `Nullable` wrapper (which is the `?` suffix, not a dtype).
pub const ACCEPTED_DTYPES: &[&str] = &[
    "i1", "i2", "i4", "i8", "u1", "u2", "u4", "u8", "f4", "f8", "b1", "str",
];

/// An empty [`ColumnData`] of the given fd5 dtype code, pre-sized for `rows` rows.
///
/// The allocation seam the row-oriented lanes (CSV) accumulate into. Keeping it here rather than in
/// each backend means the dtype vocabulary has exactly one definition, next to the one that
/// validates it.
pub fn empty_column(dtype: &str, rows: usize) -> Result<ColumnData> {
    let d = match dtype {
        "i1" => ColumnData::I8(Vec::with_capacity(rows)),
        "i2" => ColumnData::I16(Vec::with_capacity(rows)),
        "i4" => ColumnData::I32(Vec::with_capacity(rows)),
        "i8" => ColumnData::I64(Vec::with_capacity(rows)),
        "u1" => ColumnData::U8(Vec::with_capacity(rows)),
        "u2" => ColumnData::U16(Vec::with_capacity(rows)),
        "u4" => ColumnData::U32(Vec::with_capacity(rows)),
        "u8" => ColumnData::U64(Vec::with_capacity(rows)),
        "f4" => ColumnData::F32(Vec::with_capacity(rows)),
        "f8" => ColumnData::F64(Vec::with_capacity(rows)),
        "b1" => ColumnData::Bool(Vec::with_capacity(rows)),
        "str" => ColumnData::Utf8(Vec::with_capacity(rows)),
        other => return Err(he(format!("unsupported column dtype '{other}'"))),
    };
    Ok(d)
}

/// The `Column` a generic backend stamps for a source column it could not classify.
///
/// [`Sensitivity::Unknown`] is the whole point (ADR-0056 §7): generic ingest has neither of the
/// vendor paths' defences — GE listmode never sees PHI, DICOM classifies at the door per PS3.15 —
/// so `Public` here would seal "safe in clear" over what might be a column of patient identifiers.
pub fn unclassified_column(name: &str, dtype: &str) -> Column {
    Column::new(name, dtype).with_sensitivity(Sensitivity::Unknown)
}

/// Identify a table-shaped source from its **magic bytes** (ADR-0056 §4).
///
/// Returns the `--from` backend name, or `None` when nothing matched.
///
/// # Why magic bytes and not the extension
///
/// ADR-0056 §4 asks for `--from` to be "sniffed from magic bytes when unambiguous and required when
/// not", and §9 separately rejects extension sniffing outright: it "trains users into a habit that
/// breaks the moment the extension lies (`.dat`, `.bin`, a `.parquet` that is really Feather), and
/// every file-type misdetection becomes a Tessera bug". Magic bytes cannot lie in that way.
///
/// CSV has no magic bytes, and that is not a gap this function should paper over: a CSV needs declared
/// column dtypes anyway, so the operator is already being explicit, and `--from csv` costs them
/// nothing they were not already paying.
pub fn sniff_table_format(path: &std::path::Path) -> Result<Option<&'static str>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f =
        std::fs::File::open(path).map_err(|e| he(format!("open {}: {e}", path.display())))?;
    let mut head = [0u8; 8];
    let n = f
        .read(&mut head)
        .map_err(|e| he(format!("read {}: {e}", path.display())))?;
    let head = &head[..n];

    // Arrow IPC *file* format: "ARROW1\0\0" at the start (the streaming format has no such header,
    // which is why only the file format is sniffable — and only the file format is seekable enough to
    // ingest anyway).
    if head.starts_with(b"ARROW1") {
        return Ok(Some("arrow"));
    }
    // Parquet: "PAR1" at BOTH ends. Checking the tail too is what distinguishes a real Parquet from a
    // truncated one — a file with the header and no footer has no readable metadata, and failing here
    // with "not a readable Parquet file" beats failing deep inside the reader.
    if head.starts_with(b"PAR1") {
        let len = f
            .seek(SeekFrom::End(0))
            .map_err(|e| he(format!("seek {}: {e}", path.display())))?;
        if len >= 8 {
            f.seek(SeekFrom::End(-4))
                .map_err(|e| he(format!("seek {}: {e}", path.display())))?;
            let mut tail = [0u8; 4];
            f.read_exact(&mut tail)
                .map_err(|e| he(format!("read {}: {e}", path.display())))?;
            if &tail == b"PAR1" {
                return Ok(Some("parquet"));
            }
        }
        return Err(he(format!(
            "{} starts with the Parquet magic but has no closing PAR1 footer — the file is truncated \
             (Parquet writes its metadata LAST, so a partial write leaves exactly this).\n  \
             preserve it as-is: tessera ingest blob {}",
            path.display(),
            path.display()
        )));
    }
    Ok(None)
}

/// Identify an **array**-shaped source from its magic bytes (ADR-0056 §4).
///
/// `.npy` opens with `\x93NUMPY`; `.npz` is a zip, so it opens with `PK\x03\x04`. Both are unambiguous,
/// which means the array lane needs no `--from` in the common case — and, as in the table lane, the
/// extension is never consulted (§9: sniffing it "trains users into a habit that breaks the moment the
/// extension lies").
pub fn sniff_array_format(path: &std::path::Path) -> Result<Option<&'static str>> {
    use std::io::Read;
    let mut f =
        std::fs::File::open(path).map_err(|e| he(format!("open {}: {e}", path.display())))?;
    let mut head = [0u8; 8];
    let n = f
        .read(&mut head)
        .map_err(|e| he(format!("read {}: {e}", path.display())))?;
    let head = &head[..n];
    if head.starts_with(b"\x93NUMPY") {
        return Ok(Some("npy"));
    }
    // Any zip: a `.npz` is a zip of `.npy` members, and the member check happens when it is opened.
    if head.starts_with(b"PK\x03\x04") || head.starts_with(b"PK\x05\x06") {
        return Ok(Some("npz"));
    }
    Ok(None)
}

/// The error for an array source whose format could not be sniffed.
pub fn unknown_array_format_error(path: &std::path::Path) -> Error {
    he(format!(
        "cannot tell what kind of array {} is — no NumPy (\\x93NUMPY) or zip/.npz (PK) magic bytes, and \
         Tessera does not guess from the extension.\n  \
         say so explicitly:\n    \
           tessera ingest array {} <OUT> --from npy\n    \
           tessera ingest array {} <OUT_DIR> --from npz\n  \
         a headerless binary needs its geometry, which only you know:\n    \
           tessera ingest --spec …   # format = \"raw\", with shape + dtype\n  \
         or preserve it opaquely, losing query but keeping every byte:\n    \
           tessera ingest blob {} <OUT> …",
        path.display(),
        path.display(),
        path.display(),
        path.display()
    ))
}

/// The error for a source whose format could not be sniffed (ADR-0056 §4: `--from` is required when
/// the magic bytes are not unambiguous).
pub fn unknown_format_error(path: &std::path::Path) -> Error {
    he(format!(
        "cannot tell what kind of table {} is — no Parquet (PAR1) or Arrow IPC (ARROW1) magic bytes, \
         and Tessera does not guess from the extension (a '.parquet' that is really Feather would \
         become a Tessera bug).\n  \
         say so explicitly:\n    \
           tessera ingest table {} <OUT> --from parquet …\n    \
           tessera ingest table {} <OUT> --from arrow …\n    \
           tessera ingest table {} <OUT> --from csv --column id:i8 --column energy:f8 …\n  \
         or preserve it opaquely, losing query but keeping every byte:\n    \
           tessera ingest blob {} <OUT> …",
        path.display(),
        path.display(),
        path.display(),
        path.display(),
        path.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nan_payloads_canonicalise_but_negative_zero_survives() {
        // A signalling NaN with a producer-specific payload, and a quiet one: both must land on the
        // same bit pattern, because the payload is not data.
        let odd = f64::from_bits(0x7ff8_0000_dead_beef);
        assert!(odd.is_nan());
        let mut d = ColumnData::F64(vec![1.5, odd, -0.0, f64::NEG_INFINITY]);
        assert!(canonicalise_floats(&mut d), "a NaN was present");
        let ColumnData::F64(v) = &d else {
            unreachable!()
        };
        assert_eq!(v[1].to_bits(), f64::NAN.to_bits(), "NaN payload normalised");
        // -0.0 is a value, not noise: S13 guarantees its bit-exact round-trip.
        assert_eq!(v[2].to_bits(), (-0.0f64).to_bits(), "-0.0 preserved");
        assert!(v[3].is_infinite() && v[3] < 0.0, "-inf preserved");

        // No NaN ⇒ nothing recorded. The receipt must not claim a transform that did not happen.
        let mut clean = ColumnData::F64(vec![0.0, -0.0, 7.25]);
        assert!(!canonicalise_floats(&mut clean));
    }

    #[test]
    fn null_slots_are_zeroed_at_the_boundary() {
        // Deliberate garbage under the nulls — exactly what a foreign writer leaves there.
        let mut d = ColumnData::Nullable {
            values: Box::new(ColumnData::I32(vec![1, 0x7fff_ffff, 3])),
            validity: vec![true, false, true],
        };
        assert!(normalise_null_slots(&mut d));
        let ColumnData::Nullable { values, .. } = &d else {
            unreachable!()
        };
        assert_eq!(**values, ColumnData::I32(vec![1, 0, 3]));
        // A non-nullable column records nothing.
        assert!(!normalise_null_slots(&mut ColumnData::I32(vec![1, 2, 3])));
    }

    /// Ordering is load-bearing: a `NaN` sitting *under* a null is gone once the null slot is zeroed,
    /// so it must not also produce a `nan_canonicalisation` record. Otherwise the seal would claim a
    /// transform that changed nothing observable, and two writers that differ only in their
    /// under-null garbage would seal differently — the exact leak H5 exists to close.
    #[test]
    fn a_nan_hidden_under_a_null_records_only_the_null_normalisation() {
        let mut b = TableBuilder::new();
        let mut d = ColumnData::Nullable {
            values: Box::new(ColumnData::F64(vec![1.0, f64::NAN, 3.0])),
            validity: vec![true, false, true],
        };
        canonicalise(&mut b, "x", &mut d);
        let ts = b.finish().transforms;
        let names: Vec<&str> = ts.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec![transform::NULL_SLOT_NORMALISATION]);
    }

    #[test]
    fn duplicate_column_names_are_an_error_not_a_silent_suffix() {
        let mut b = TableBuilder::new();
        b.push(unclassified_column("a.b", "i4"), ColumnData::I32(vec![1]))
            .unwrap();
        let err = b
            .push(unclassified_column("a.b", "i4"), ColumnData::I32(vec![2]))
            .unwrap_err()
            .to_string();
        assert!(err.contains("duplicate column name 'a.b'"), "got {err}");
        assert!(err.contains("--exclude"), "the error offers a next command");
    }

    /// A nullable string/bool column is rejected **at the boundary**, with the column named and an
    /// escape hatch offered — not left to fail later inside the Vortex encoder, where the message
    /// mentions neither.
    #[test]
    fn a_nullable_string_or_bool_column_is_rejected_at_the_boundary() {
        for values in [
            ColumnData::Utf8(vec!["a".into(), String::new()]),
            ColumnData::Bool(vec![true, false]),
        ] {
            let code = values.numpy_code();
            let mut b = TableBuilder::new();
            let err = b
                .push(
                    unclassified_column("maybe", code),
                    ColumnData::Nullable {
                        values: Box::new(values),
                        validity: vec![true, false],
                    },
                )
                .unwrap_err()
                .to_string();
            assert!(err.contains(&format!("nullable '{code}'")), "got {err}");
            assert!(
                err.contains("--exclude maybe"),
                "offers a next command: {err}"
            );
            assert!(
                err.contains("ingest blob"),
                "offers the preserve tier: {err}"
            );
        }
        // The same dtypes are fine when they carry no nulls, which is the common case.
        let mut b = TableBuilder::new();
        b.push(
            unclassified_column("text", "str"),
            ColumnData::Utf8(vec!["a".into(), "b".into()]),
        )
        .expect("a non-nullable string column is ordinary");
    }

    #[test]
    fn ragged_columns_are_rejected() {
        let mut b = TableBuilder::new();
        b.push(unclassified_column("a", "i4"), ColumnData::I32(vec![1, 2]))
            .unwrap();
        let err = b
            .push(unclassified_column("b", "i4"), ColumnData::I32(vec![1]))
            .unwrap_err()
            .to_string();
        assert!(err.contains("1 rows but the table has 2"), "got {err}");
    }

    #[test]
    fn transforms_dedupe_per_column_and_emit_deterministically() {
        let mut b = TableBuilder::new();
        // Same transform, same column, twice → one record.
        b.record(IngestTransform::new(transform::F16_WIDEN).on_column("z"));
        b.record(IngestTransform::new(transform::F16_WIDEN).on_column("z"));
        // Same transform, different column → two records.
        b.record(IngestTransform::new(transform::F16_WIDEN).on_column("a"));
        b.record(IngestTransform::new(transform::NULL_SLOT_NORMALISATION).on_column("a"));
        let ts = b.finish().transforms;
        assert_eq!(ts.len(), 3);
        // Sorted by (name, column) — deterministic, because this list is sealed.
        let keys: Vec<(String, String)> = ts
            .iter()
            .map(|t| {
                (
                    t.name.clone(),
                    t.params["column"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        assert_eq!(
            keys,
            vec![
                (transform::F16_WIDEN.into(), "a".into()),
                (transform::F16_WIDEN.into(), "z".into()),
                (transform::NULL_SLOT_NORMALISATION.into(), "a".into()),
            ]
        );
    }

    #[test]
    fn column_declarations_parse_and_reject_clearly() {
        assert_eq!(
            parse_column_decl("energy:f8").unwrap(),
            ("energy".into(), "f8".into(), false)
        );
        assert_eq!(
            parse_column_decl("age:i4?").unwrap(),
            ("age".into(), "i4".into(), true)
        );
        assert_eq!(
            parse_column_decl("label:str").unwrap(),
            ("label".into(), "str".into(), false)
        );
        // No colon at all.
        let err = parse_column_decl("energy").unwrap_err().to_string();
        assert!(err.contains("NAME:DTYPE"), "got {err}");
        // A dtype Tessera tables cannot carry — the error must enumerate what they can.
        let err = parse_column_decl("t:datetime64").unwrap_err().to_string();
        assert!(err.contains("accepted:") && err.contains("f8"), "got {err}");
        // Empty name.
        assert!(parse_column_decl(":f8").is_err());
    }

    #[test]
    fn every_accepted_dtype_has_an_empty_column_and_the_two_lists_agree() {
        for d in ACCEPTED_DTYPES {
            let c = empty_column(d, 4).expect("accepted dtype allocates");
            assert_eq!(
                c.numpy_code(),
                *d,
                "empty_column('{d}') round-trips to a different code — the vocabulary has drifted"
            );
        }
        assert!(empty_column("f2", 1).is_err(), "f2 is array-only");
    }

    #[test]
    fn a_generic_column_is_stamped_unclassified() {
        let c = unclassified_column("patient_id", "str");
        assert_eq!(c.sensitivity, Sensitivity::Unknown);
        assert_eq!(c.dtype, "str");
    }

    #[test]
    fn magic_bytes_identify_parquet_and_arrow_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();

        // A well-formed Parquet skeleton: magic at both ends.
        let pq = dir.path().join("a.parquet");
        std::fs::write(&pq, b"PAR1\x00\x00\x00\x00\x00\x00\x00\x00PAR1").unwrap();
        assert_eq!(sniff_table_format(&pq).unwrap(), Some("parquet"));

        let arrow = dir.path().join("a.arrow");
        std::fs::write(&arrow, b"ARROW1\x00\x00rest").unwrap();
        assert_eq!(sniff_table_format(&arrow).unwrap(), Some("arrow"));

        // A CSV has no magic — and the extension is deliberately NOT consulted.
        let csv = dir.path().join("a.csv");
        std::fs::write(&csv, b"id,energy\n1,2.0\n").unwrap();
        assert_eq!(sniff_table_format(&csv).unwrap(), None);

        // The extension lying in the other direction must not fool it either.
        let liar = dir.path().join("really_a_csv.parquet");
        std::fs::write(&liar, b"id,energy\n1,2.0\n").unwrap();
        assert_eq!(sniff_table_format(&liar).unwrap(), None);
    }

    /// Parquet writes its metadata footer LAST, so a truncated file has the header and no footer —
    /// worth its own error, because the reader's failure deep inside is much harder to act on.
    #[test]
    fn a_truncated_parquet_is_diagnosed_rather_than_handed_to_the_reader() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("cut.parquet");
        std::fs::write(&p, b"PAR1\x01\x02\x03\x04\x05\x06").unwrap();
        let err = sniff_table_format(&p).unwrap_err().to_string();
        assert!(err.contains("truncated"), "got {err}");
        assert!(err.contains("ingest blob"), "offers the preserve tier");
    }

    #[test]
    fn the_unknown_format_error_lists_every_real_option() {
        let err = unknown_format_error(std::path::Path::new("x.dat")).to_string();
        for needle in [
            "--from parquet",
            "--from arrow",
            "--from csv",
            "ingest blob",
        ] {
            assert!(err.contains(needle), "expected '{needle}' in: {err}");
        }
    }

    #[test]
    fn an_empty_file_sniffs_as_nothing_rather_than_panicking() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("empty");
        std::fs::write(&p, b"").unwrap();
        assert_eq!(sniff_table_format(&p).unwrap(), None);
    }
}
