//! The **ADR-0056 §2 Arrow→Tessera type map** — three lanes, and no lossy lane.
//!
//! Every table-shaped generic source reaches the waist through here: Parquet
//! ([`crate::parquet_table`]) and Arrow IPC / Feather (this module's [`read_arrow_ipc`]) both hand
//! over `RecordBatch`es, and this is what turns them into `ColumnData`.
//!
//! # The three lanes
//!
//! ADR-0056 §2 rejected the originally-proposed "lossy-map-with-warning" tier: ADR-0025 says ingest is
//! lossless, a warning printed to a terminal that scrolls away does not travel with the artifact, and a
//! seal over silently-degraded values asserts *under a signature* that those values are the truth. So:
//!
//! - **clean** — bit-faithful, no transformation;
//! - **lossless-with-recorded-transform** — reversible, with the parameters written **inside the seal**
//!   so the artifact carries its own recovery instructions;
//! - **reject** — a hard error naming the column, the reason, and the exact escape hatch.
//!
//! | Arrow type | Lane | Target | Recorded |
//! | --- | --- | --- | --- |
//! | `Int8…64`, `UInt8…64` | clean | `I8…I64`, `U8…U64` | — |
//! | `Float32/64` | clean | `F32`, `F64` | NaN canonicalised (§5 H4) |
//! | `Boolean` | clean | `Bool` | `len` bits, never trailing padding |
//! | `Utf8`, `LargeUtf8`, `Utf8View` | clean | `Utf8` | a view is a runtime encoding |
//! | `Null` | clean | `Nullable{I8, all-absent}` | — |
//! | `RunEndEncoded` | clean | decoded through | `run_end_decoded` |
//! | `Float16` | recorded | `F32` | `f16_widen` |
//! | `Decimal128(p ≤ 18, s)` | recorded | `I64` + `scale = 10⁻ˢ` | `decimal_fixed_point` |
//! | `Date32/64`, `Time32/64`, `Duration`, `Timestamp` | recorded | `I32`/`I64` + unit + scale | `epoch_anchor` / `tz_to_utc` |
//! | `Dictionary` | recorded | `Utf8` **values**, not codes | `dictionary_materialised` |
//! | `FixedSizeList<scalar, N ≤ 8>` | recorded | `N` columns `name.0…` | `fixed_list_expand` |
//! | `Struct` | flatten | dotted columns | `struct_flatten` |
//! | `Decimal128(p > 18)`, `Decimal256`, `Binary*`, `Interval`, `List`, `Map`, `Union` | reject | — | §2/§3 |
//!
//! # Two boundary rules that are not in the table, and are load-bearing
//!
//! **Nullability is decided by the data, never by the schema.** A column is wrapped in
//! `ColumnData::Nullable` iff it actually contains at least one null — *not* iff its Arrow field is
//! declared nullable. This is what makes ADR-0056 §5's three-producer fixture pass: pyarrow, polars
//! and DuckDB disagree about whether a column with no nulls is declared nullable, and honouring the
//! declaration would seal the same logical table three different ways. What is lost is the producer's
//! "could have been null but wasn't", which is schema nuance rather than a value — and §2 says ingest
//! imports the *logical values*.
//!
//! **Column order and dtype width are preserved exactly.** Source order is sealed order (the Merkle
//! root is over the encoded block, and the block encodes columns in order), and an `Int8` stays an
//! `I8` rather than being widened to the machine word. Both would otherwise be places where a
//! re-encode quietly editorialised.
//!
//! # Why two mappings that look like losses are not
//!
//! **Dictionary → values, dropping the source's codes.** Preserving the writer's integer codes would
//! import the writer's *dictionary order* into `content_hash` — and pandas' `Categorical` order is
//! insertion-time, so the same logical data from pandas, polars and Spark would seal to three
//! different hashes. Vortex re-derives FSST/dictionary encoding for low-cardinality repeats on the far
//! side, so the compression the source thought it was preserving is reclaimed anyway.
//!
//! **Decimal → `I64` + `scale`, never `F64`.** Decimal→float is lossy for exactly the accounting-like
//! data that uses decimals, and the cast is FMA-sensitive across architectures (§5 H3). `Column.scale`
//! is what that field exists for.
//!
//! # Timezones: hazard H1 is closed by never asking
//!
//! `Timestamp(_, Some(tz))` in Arrow's model stores UTC-normalised `i64` ticks; the zone is a *display*
//! annotation. So this module reads the raw `i64` value buffer and never calls a zone-aware arrow
//! function — no tzdb is consulted, on any build, which is why ADR-0057 §5's finding that `--features
//! sql` flips `arrow-array/chrono-tz` cannot reach a sealed value. The source zone is not discarded
//! silently: it is sealed in the `tz_to_utc{from}` transform record, which is the only reason calling
//! this lossless is honest.

use std::sync::Arc;

use arrow_array::cast::AsArray;
use arrow_array::types::{
    Date32Type, Date64Type, Decimal128Type, DurationMicrosecondType, DurationMillisecondType,
    DurationNanosecondType, DurationSecondType, Float16Type, Float32Type, Float64Type, Int16Type,
    Int32Type, Int64Type, Int8Type, Time32MillisecondType, Time32SecondType, Time64MicrosecondType,
    Time64NanosecondType, TimestampMicrosecondType, TimestampMillisecondType,
    TimestampNanosecondType, TimestampSecondType, UInt16Type, UInt32Type, UInt64Type, UInt8Type,
};
use arrow_array::{Array, ArrayRef, RecordBatch};
use arrow_schema::{DataType, Field, IntervalUnit, TimeUnit};
use tessera_core::block::table::Column;
use tessera_core::referencing::{Referenced, Transform};
use tessera_core::{Error, IngestTransform, Result};
use tessera_io::table::ColumnData;

use crate::canonical::{
    canonicalise, concat_columns, transform, unclassified_column, CanonicalTable, TableBuilder,
};

fn he(e: impl std::fmt::Display) -> Error {
    Error::Invalid(format!("ingest: {e}"))
}

/// A rejection, phrased ADR-0056 §2 style: name the column, the reason, and the **runnable** escape
/// hatch. "If a code path ever emits a warning without a runnable next command, that path is a bug"
/// (§9) — the same standard applies to errors, more so.
fn reject(column: &str, what: &str, why: &str) -> Error {
    he(format!(
        "column '{column}' has type {what}; {why}\n  \
         drop it:    tessera ingest table <FILE> --exclude {column}\n  \
         keep bytes: tessera ingest blob <FILE>   # bit-faithful, opaque payload (ADR-0038)"
    ))
}

/// How many components a `FixedSizeList` may be expanded into before it is a volume rather than a
/// tuple (ADR-0056 §2).
///
/// A width-3 list is a coordinate; a width-262144 one is a flattened 64³ volume whose home is the
/// *array* primitive, and silently minting 262144 columns would be the "perpetuate the misuse"
/// failure §1 forbids. The bound is what makes `ingest analyze`'s recommendation the right next step
/// instead.
pub const MAX_FIXED_LIST_WIDTH: usize = 8;

/// Canonicalise a set of `RecordBatch`es (all sharing one schema) into a flat Tessera table.
///
/// `exclude` drops columns by source name before any mapping — which is what makes `--exclude` a real
/// escape hatch for a rejected column rather than advice the user cannot act on.
pub fn canonicalise_batches(batches: &[RecordBatch], exclude: &[String]) -> Result<CanonicalTable> {
    let Some(first) = batches.first() else {
        return Ok(CanonicalTable::default());
    };
    let schema = first.schema();
    // An `--exclude` that matches nothing is a typo, and a typo that silently does nothing is the worst
    // outcome here: the operator believes they dropped a PHI column. The CSV lane already errors on
    // this, so the two lanes agree rather than differing by accident.
    // **Every `--exclude` must name a real LEAF path**, dotted or not.
    //
    // Checking only the part before the dot let `--exclude patient.nmae` pass and then match nothing —
    // so the column the operator believed they had dropped was **sealed**. That is the same
    // false-confidence failure the `--column-meta` typo check exists to prevent, and worse here, because
    // the column in question is the one they thought was PHI.
    //
    // The leaf paths are enumerated from the schema *before* anything is decoded, which also makes the
    // advice in a rejected-column error work: an excluded leaf is skipped before `map_field` can reject
    // it (see below).
    let leaves = schema_leaf_paths(&schema);
    if let Some(unknown) = exclude.iter().find(|e| !leaves.contains(*e)) {
        return Err(he(format!(
            "--exclude names '{unknown}', which is not a column in this file\n  columns: {}",
            leaves.join(" · ")
        )));
    }
    let mut b = TableBuilder::new();
    // Per top-level field, fold every batch's chunk into one column. Nullability is resolved by the
    // fold (see the module docs), so a column whose only null is in the last row group still seals as
    // one nullable column.
    for (idx, field) in schema.fields().iter().enumerate() {
        if exclude.iter().any(|e| e == field.name()) {
            continue;
        }
        // A struct or fixed-size list fans out into several columns, so the unit of folding is
        // "named leaves", not "one column".
        let mut folded: Vec<(String, Column, ColumnData)> = Vec::new();
        for batch in batches {
            let array = batch.column(idx);
            let mut leaves = Vec::new();
            map_field(field.name(), field, array, exclude, &mut b, &mut leaves)?;
            if folded.is_empty() {
                folded = leaves;
                continue;
            }
            if folded.len() != leaves.len() {
                return Err(he(format!(
                    "column '{}' expanded to {} leaves in one chunk and {} in another",
                    field.name(),
                    folded.len(),
                    leaves.len()
                )));
            }
            for (acc, next) in folded.iter_mut().zip(leaves) {
                if acc.0 != next.0 {
                    return Err(he(format!(
                        "column '{}' changed name between chunks ('{}' → '{}')",
                        field.name(),
                        acc.0,
                        next.0
                    )));
                }
                let merged = concat_columns(
                    &acc.0,
                    std::mem::replace(&mut acc.2, ColumnData::Bool(vec![])),
                    next.2,
                )?;
                acc.2 = merged;
            }
        }
        for (name, column, mut data) in folded {
            canonicalise(&mut b, &name, &mut data);
            // The nullable flag on the sealed `Column` follows the data, exactly as the wrapper does.
            let column = if matches!(data, ColumnData::Nullable { .. }) {
                column.nullable()
            } else {
                column
            };
            b.push(column, data)?;
        }
    }
    Ok(b.finish())
}

/// Every **leaf path** the schema will flatten into, in order — the set `--exclude` is validated against.
///
/// Derived from the schema rather than from a mapping pass, so it is available *before* anything is
/// decoded. That ordering is what lets an excluded leaf be skipped before `map_field` can reject it, and
/// what turns a typo in a dotted name into an error instead of a silent no-op.
fn schema_leaf_paths(schema: &arrow_schema::Schema) -> Vec<String> {
    let mut out = Vec::with_capacity(schema.fields().len());
    for f in schema.fields() {
        push_leaf_paths(f.name(), f, &mut out);
    }
    out
}

/// [`schema_leaf_paths`] for one field.
///
/// A field whose own name contains a dot is a leaf path in its own right, which is why matching is always
/// against these **whole paths** and never against a prefix split on `.` — splitting first would look for
/// a field called `a` when the operator wrote `a.b` and meant a column of that name.
fn push_leaf_paths(name: &str, field: &Field, out: &mut Vec<String>) {
    match field.data_type() {
        DataType::Struct(children) => {
            for child in children {
                push_leaf_paths(&child_name(name, child.name()), child, out);
            }
        }
        DataType::FixedSizeList(_, width) => match usize::try_from(*width) {
            // A list narrow enough to expand contributes one leaf per component.
            Ok(w) if w > 0 && w <= MAX_FIXED_LIST_WIDTH => {
                for k in 0..w {
                    out.push(child_name(name, &k.to_string()));
                }
            }
            // Anything else is going to be rejected by `map_field` (too wide, or zero-width), so the only
            // meaningful exclusion is the whole field — and enumerating 262144 component paths for a
            // flattened volume would be absurd as well as useless.
            _ => out.push(name.to_string()),
        },
        _ => out.push(name.to_string()),
    }
}

/// The per-leaf name a struct flatten or fixed-list expansion produces: `parent.child`.
///
/// Dotted, because pandas, polars and DuckDB already present nested Parquet this way, so a reader
/// arrives with the right expectation. A flatten is a **renaming**, not a semantic change — and a
/// collision with an existing top-level column is an error in [`TableBuilder::push`], never a silent
/// suffix (ADR-0056 §3).
fn child_name(prefix: &str, child: &str) -> String {
    format!("{prefix}.{child}")
}

/// Map one (possibly nested) field into named flat leaves.
fn map_field(
    name: &str,
    field: &Field,
    array: &ArrayRef,
    exclude: &[String],
    b: &mut TableBuilder,
    out: &mut Vec<(String, Column, ColumnData)>,
) -> Result<()> {
    // Skip an excluded leaf **before decoding it**. This is what makes the advice in a rejection
    // actionable: a nested `binary` column's error says "--exclude pos.blob", and filtering after the
    // mapping would mean `map_field` had already rejected it, so following the advice still failed.
    if exclude.iter().any(|e| e == name) {
        return Ok(());
    }
    match field.data_type() {
        // ── §3: a struct flatten is a renaming. Recurse. ──
        DataType::Struct(children) => {
            b.record(IngestTransform::new(transform::STRUCT_FLATTEN).on_column(name));
            let sa = array.as_struct();
            let first_leaf = out.len();
            for (i, child) in children.iter().enumerate() {
                let child_array = sa.column(i).clone();
                map_field(
                    &child_name(name, child.name()),
                    child,
                    &child_array,
                    exclude,
                    b,
                    out,
                )?;
            }
            // **Apply the STRUCT's own validity to every leaf it flattened into.**
            //
            // Arrow permits perfectly valid child data underneath a **null parent row**
            // (`pyarrow.StructArray.from_arrays(…, mask=…)` writes exactly that, and it survives an
            // Arrow IPC round-trip), and walking the children directly reads straight past it. Without
            // this, a row the source says is *absent* sealed as present child values — unmasked,
            // unrecorded, and verifying. §3 calls a flatten a **renaming**, and a renaming must not
            // change which rows exist.
            //
            // Applied after the recursion rather than inside it, so a nested struct's absence reaches
            // the leaf through however many levels: each level masks the leaves below it in turn.
            if let Some(parent_valid) = validity_of(sa) {
                for (_, _, data) in out[first_leaf..].iter_mut() {
                    mask_absent_rows(data, &parent_valid);
                }
            }
            Ok(())
        }
        // ── §2: a schema-fixed narrow tuple expands into N columns. ──
        DataType::FixedSizeList(child, width) => {
            let width = usize::try_from(*width)
                .map_err(|_| he(format!("column '{name}' has a negative list width")))?;
            if width > MAX_FIXED_LIST_WIDTH {
                return Err(he(format!(
                    "column '{name}' is fixed_size_list<{}>[{width}], which is wider than {} — at that \
                     width it is a flattened grid, not a tuple, and its home is the ARRAY primitive.\n  \
                     see what it looks like:  tessera ingest analyze <FILE>\n  \
                     drop it:                 tessera ingest table <FILE> --exclude {name}\n  \
                     keep bytes:              tessera ingest blob <FILE>",
                    child.data_type(),
                    MAX_FIXED_LIST_WIDTH
                )));
            }
            if width == 0 {
                return Err(he(format!(
                    "column '{name}' is fixed_size_list<{}>[0]; a zero-width tuple carries no values, \
                     so expanding it would drop the column silently\n  \
                     drop it explicitly: tessera ingest table <FILE> --exclude {name}",
                    child.data_type()
                )));
            }
            if !child.data_type().is_primitive() && !matches!(child.data_type(), DataType::Boolean)
            {
                return Err(reject(
                    name,
                    &format!("fixed_size_list<{}>", child.data_type()),
                    "only a list of scalars expands into columns (a list of lists has no fixed \
                     column shape)",
                ));
            }
            b.record(
                IngestTransform::new(transform::FIXED_LIST_EXPAND)
                    .on_column(name)
                    .with("n", serde_json::json!(width)),
            );
            let fl = array.as_fixed_size_list();
            let values = fl.values();
            for k in 0..width {
                // An expanded component can be excluded by its dotted path, like any other leaf.
                if exclude
                    .iter()
                    .any(|e| *e == child_name(name, &k.to_string()))
                {
                    continue;
                }
                // Gather element k of every row: logical row r sits at values[r * width + k].
                let indices: Vec<Option<usize>> = (0..fl.len())
                    .map(|r| {
                        // A null list makes every expanded component null for that row — the only
                        // honest reading, since the tuple as a whole was absent.
                        if fl.is_null(r) {
                            None
                        } else {
                            Some(r * width + k)
                        }
                    })
                    .collect();
                let data = gather(values, &indices, &child_name(name, &k.to_string()))?;
                let leaf = child_name(name, &k.to_string());
                out.push((
                    leaf.clone(),
                    unclassified_column(&leaf, data.numpy_code()),
                    data,
                ));
            }
            Ok(())
        }
        _ => {
            let (column, data) = map_leaf(name, field, array, b)?;
            out.push((name.to_string(), column, data));
            Ok(())
        }
    }
}

/// Build the validity mask for an array, or `None` when it has no nulls at all.
///
/// The nullable-by-presence rule in one place: `null_count() == 0` ⇒ no wrapper, whatever the field
/// declared.
fn validity_of(array: &dyn Array) -> Option<Vec<bool>> {
    if array.null_count() == 0 {
        return None;
    }
    Some((0..array.len()).map(|i| array.is_valid(i)).collect())
}

/// Intersect a parent's validity into a leaf column: a row absent in the parent is absent in the leaf.
///
/// The composition rule for nested nullability. Promotes a non-nullable leaf to nullable when the parent
/// has any null, and ANDs the two masks when it is already nullable — so a leaf that was null for its
/// own reason stays null, and one whose parent was absent becomes null too.
///
/// The value under a newly-masked slot is left as-is; [`crate::canonical::canonicalise`] zeroes every
/// masked slot at the boundary afterwards (H5), which is the single place that normalisation lives.
fn mask_absent_rows(data: &mut ColumnData, parent_valid: &[bool]) {
    if parent_valid.iter().all(|v| *v) {
        return;
    }
    match data {
        ColumnData::Nullable { validity, .. } => {
            for (v, p) in validity.iter_mut().zip(parent_valid) {
                *v = *v && *p;
            }
        }
        other => {
            let values = std::mem::replace(other, ColumnData::Bool(Vec::new()));
            *other = ColumnData::Nullable {
                values: Box::new(values),
                validity: parent_valid.to_vec(),
            };
        }
    }
}

/// Wrap `values` in [`ColumnData::Nullable`] if `array` carries nulls.
fn with_validity(array: &dyn Array, values: ColumnData) -> ColumnData {
    match validity_of(array) {
        None => values,
        Some(validity) => ColumnData::Nullable {
            values: Box::new(values),
            validity,
        },
    }
}

/// Map one scalar (non-nested) Arrow array to a flat column.
fn map_leaf(
    name: &str,
    field: &Field,
    array: &ArrayRef,
    b: &mut TableBuilder,
) -> Result<(Column, ColumnData)> {
    let dt = field.data_type();
    // Integers, floats, bool, string, null: the clean lane. Extracted through one helper because the
    // dictionary / run-end lanes need exactly the same leaf mapping for their *values* array.
    if let Some(data) = map_scalar(dt, array)? {
        let mut column = unclassified_column(name, data.numpy_code());
        // An Arrow extension type is carried best-effort on its storage type, with the extension name
        // recorded in the description so a reader is not left guessing why an `i8` means what it
        // means (ADR-0056 §2, the extension-types row).
        if let Some(ext) = field.metadata().get("ARROW:extension:name") {
            column = column.with_description(format!("[arrow-ext:{ext}]"));
        }
        return Ok((column, data));
    }

    match dt {
        // ── recorded: exact widening ──
        DataType::Float16 => {
            b.record(IngestTransform::new(transform::F16_WIDEN).on_column(name));
            let a = array.as_primitive::<Float16Type>();
            let values = ColumnData::F32(a.values().iter().map(|h| h.to_f32()).collect());
            Ok((
                unclassified_column(name, "f4"),
                with_validity(array.as_ref(), values),
            ))
        }

        // ── recorded: decimal as fixed point, never as float ──
        DataType::Decimal128(precision, scale) => {
            // i64 carries 18 full decimal digits; above that there is no lossless integer carrier, and
            // ADR-0056 §2 rejects rather than reaching for f64.
            if *precision > 18 {
                return Err(he(format!(
                    "column '{name}' is decimal128({precision},{scale}); no lossless integer carrier \
                     exists above precision 18, and decimal→float is lossy for exactly the data that \
                     uses decimals.\n  \
                     drop it:    tessera ingest table <FILE> --exclude {name}\n  \
                     keep bytes: tessera ingest blob <FILE>"
                )));
            }
            if *scale < 0 {
                return Err(he(format!(
                    "column '{name}' is decimal128({precision},{scale}) with a negative scale, which \
                     Column.scale (physical = raw × scale) cannot express as a decimal exponent"
                )));
            }
            let a = array.as_primitive::<Decimal128Type>();
            // Every value fits i64 by the precision check above, so a failure here is a producer
            // writing out-of-range values for its own declared precision — worth an error, not a wrap.
            let mut values = Vec::with_capacity(a.len());
            for i in 0..a.len() {
                if a.is_null(i) {
                    values.push(0);
                    continue;
                }
                values.push(i64::try_from(a.value(i)).map_err(|_| {
                    he(format!(
                        "column '{name}': decimal value at row {i} does not fit i64 despite declared \
                         precision {precision}"
                    ))
                })?);
            }
            let factor = decimal_scale_factor(*scale)?;
            b.record(
                IngestTransform::new(transform::DECIMAL_FIXED_POINT)
                    .on_column(name)
                    .with("scale", serde_json::json!(factor))
                    .with("precision", serde_json::json!(precision)),
            );
            let column = unclassified_column(name, "i8").with_scale(factor);
            Ok((column, with_validity(array.as_ref(), ColumnData::I64(values))))
        }

        // ── recorded: absolute instants = ticks + an epoch ──
        DataType::Date32 => {
            let a = array.as_primitive::<Date32Type>();
            let values = ColumnData::I32(a.values().to_vec());
            Ok((
                epoch_column(b, name, "i4", 86_400.0, "unix"),
                with_validity(array.as_ref(), values),
            ))
        }
        DataType::Date64 => {
            let a = array.as_primitive::<Date64Type>();
            let values = ColumnData::I64(a.values().to_vec());
            Ok((
                epoch_column(b, name, "i8", 1e-3, "unix"),
                with_validity(array.as_ref(), values),
            ))
        }
        DataType::Timestamp(unit, tz) => {
            // The raw i64 buffer, per hazard H1: Arrow already stores UTC-normalised ticks and the
            // zone is a display annotation, so nothing here consults a tzdb.
            let values = ColumnData::I64(match unit {
                TimeUnit::Second => array.as_primitive::<TimestampSecondType>().values().to_vec(),
                TimeUnit::Millisecond => array
                    .as_primitive::<TimestampMillisecondType>()
                    .values()
                    .to_vec(),
                TimeUnit::Microsecond => array
                    .as_primitive::<TimestampMicrosecondType>()
                    .values()
                    .to_vec(),
                TimeUnit::Nanosecond => array
                    .as_primitive::<TimestampNanosecondType>()
                    .values()
                    .to_vec(),
            });
            if let Some(zone) = tz {
                // Lossless only *because* the source zone is sealed. Without this record the column
                // would silently lose which wall-clock it was written against.
                b.record(
                    IngestTransform::new(transform::TZ_TO_UTC)
                        .on_column(name)
                        .with("from", serde_json::json!(zone.as_ref())),
                );
            }
            Ok((
                epoch_column(b, name, "i8", tick_seconds(unit), "unix"),
                with_validity(array.as_ref(), values),
            ))
        }

        // ── recorded: elapsed / time-of-day = ticks with NO epoch ──
        DataType::Time32(unit) => {
            let values = ColumnData::I32(match unit {
                TimeUnit::Second => array.as_primitive::<Time32SecondType>().values().to_vec(),
                TimeUnit::Millisecond => array
                    .as_primitive::<Time32MillisecondType>()
                    .values()
                    .to_vec(),
                other => {
                    return Err(he(format!(
                        "column '{name}' is time32({other:?}), which Arrow does not define"
                    )))
                }
            });
            Ok((
                elapsed_column(name, "i4", tick_seconds(unit)),
                with_validity(array.as_ref(), values),
            ))
        }
        DataType::Time64(unit) => {
            let values = ColumnData::I64(match unit {
                TimeUnit::Microsecond => array
                    .as_primitive::<Time64MicrosecondType>()
                    .values()
                    .to_vec(),
                TimeUnit::Nanosecond => array
                    .as_primitive::<Time64NanosecondType>()
                    .values()
                    .to_vec(),
                other => {
                    return Err(he(format!(
                        "column '{name}' is time64({other:?}), which Arrow does not define"
                    )))
                }
            });
            Ok((
                elapsed_column(name, "i8", tick_seconds(unit)),
                with_validity(array.as_ref(), values),
            ))
        }
        DataType::Duration(unit) => {
            let values = ColumnData::I64(match unit {
                TimeUnit::Second => array.as_primitive::<DurationSecondType>().values().to_vec(),
                TimeUnit::Millisecond => array
                    .as_primitive::<DurationMillisecondType>()
                    .values()
                    .to_vec(),
                TimeUnit::Microsecond => array
                    .as_primitive::<DurationMicrosecondType>()
                    .values()
                    .to_vec(),
                TimeUnit::Nanosecond => array
                    .as_primitive::<DurationNanosecondType>()
                    .values()
                    .to_vec(),
            });
            Ok((
                elapsed_column(name, "i8", tick_seconds(unit)),
                with_validity(array.as_ref(), values),
            ))
        }

        // ── recorded: dictionaries and run-ends are *encodings*; decode through to values ──
        DataType::Dictionary(key, _) => {
            b.record(IngestTransform::new(transform::DICTIONARY_MATERIALISED).on_column(name));
            let (values, indices) = dictionary_parts(array, key, name)?;
            let data = gather(&values, &indices, name)?;
            Ok((unclassified_column(name, data.numpy_code()), data))
        }
        DataType::RunEndEncoded(run_ends, _) => {
            b.record(IngestTransform::new(transform::RUN_END_DECODED).on_column(name));
            let (values, indices) = run_end_parts(array, run_ends.data_type(), name)?;
            let data = gather(&values, &indices, name)?;
            Ok((unclassified_column(name, data.numpy_code()), data))
        }

        // ── reject, each with its reason and its escape hatch ──
        DataType::Binary | DataType::LargeBinary | DataType::BinaryView => Err(reject(
            name,
            "binary",
            "Tessera tables carry no opaque byte columns (base64-in-utf8 would lie about the type \
             and defeat the codec)",
        )),
        DataType::FixedSizeBinary(n) => Err(reject(
            name,
            &format!("fixed_size_binary({n})"),
            "Tessera tables carry no opaque byte columns",
        )),
        DataType::Decimal256(p, s) => Err(reject(
            name,
            &format!("decimal256({p},{s})"),
            "no lossless integer carrier exists for 256-bit decimals",
        )),
        DataType::Interval(unit) => Err(reject(
            name,
            &format!("interval({})", interval_name(unit)),
            "an interval encodes calendar arithmetic (months/days are not a fixed duration) that \
             Tessera does not preserve; store the resolved duration instead",
        )),
        DataType::List(_) | DataType::LargeList(_) | DataType::ListView(_)
        | DataType::LargeListView(_) => Err(he(format!(
            "column '{name}' is a list; TableSpec.rows is a single number, so a per-row variable \
             count has no flat representation. Exploding one row into N would destroy the source-row \
             identity a reader joins sibling columns on — a semantic transformation, which ADR-0056 \
             §1 forbids performing silently.\n  \
             drop it:    tessera ingest table <FILE> --exclude {name}\n  \
             keep bytes: tessera ingest blob <FILE>"
        ))),
        DataType::Map(..) => Err(reject(
            name,
            "map",
            "a map is a per-row variable set of keys, which no fixed column schema can express",
        )),
        DataType::Union(..) => Err(reject(
            name,
            "union",
            "a union's dtype varies per row, and a sealed column has exactly one dtype",
        )),
        other => Err(reject(
            name,
            &format!("{other}"),
            "no lossless mapping into Tessera's flat column set is defined",
        )),
    }
}

/// The clean lane: the Arrow types that map to a `ColumnData` variant with no transformation at all.
///
/// Returns `None` for anything that needs the recorded or reject lane. Factored out because the
/// dictionary and run-end lanes must map *their values array* through exactly this same table — a
/// `Dictionary<Int32, Utf8>` and a plain `Utf8` must produce byte-identical columns, or the
/// materialisation would not be the no-op it claims to be.
fn map_scalar(dt: &DataType, array: &ArrayRef) -> Result<Option<ColumnData>> {
    macro_rules! prim {
        ($t:ty, $variant:path) => {{
            let a = array.as_primitive::<$t>();
            Some($variant(a.values().to_vec()))
        }};
    }
    let values = match dt {
        DataType::Int8 => prim!(Int8Type, ColumnData::I8),
        DataType::Int16 => prim!(Int16Type, ColumnData::I16),
        DataType::Int32 => prim!(Int32Type, ColumnData::I32),
        DataType::Int64 => prim!(Int64Type, ColumnData::I64),
        DataType::UInt8 => prim!(UInt8Type, ColumnData::U8),
        DataType::UInt16 => prim!(UInt16Type, ColumnData::U16),
        DataType::UInt32 => prim!(UInt32Type, ColumnData::U32),
        DataType::UInt64 => prim!(UInt64Type, ColumnData::U64),
        DataType::Float32 => prim!(Float32Type, ColumnData::F32),
        DataType::Float64 => prim!(Float64Type, ColumnData::F64),
        // Iterate exactly `len` bits. A `BooleanArray`'s buffer is bit-packed, and its trailing bits
        // are unspecified padding — reading the whole byte would put producer noise in the column.
        DataType::Boolean => {
            let a = array.as_boolean();
            Some(ColumnData::Bool((0..a.len()).map(|i| a.value(i)).collect()))
        }
        // A string *view* is a runtime layout, not a logical type: materialise owned strings so the
        // sealed bytes do not depend on how the producer chose to lay out its buffers.
        DataType::Utf8 => {
            let a = array.as_string::<i32>();
            Some(ColumnData::Utf8(
                (0..a.len())
                    .map(|i| {
                        if a.is_null(i) {
                            String::new()
                        } else {
                            a.value(i).to_string()
                        }
                    })
                    .collect(),
            ))
        }
        DataType::LargeUtf8 => {
            let a = array.as_string::<i64>();
            Some(ColumnData::Utf8(
                (0..a.len())
                    .map(|i| {
                        if a.is_null(i) {
                            String::new()
                        } else {
                            a.value(i).to_string()
                        }
                    })
                    .collect(),
            ))
        }
        DataType::Utf8View => {
            let a = array.as_string_view();
            Some(ColumnData::Utf8(
                (0..a.len())
                    .map(|i| {
                        if a.is_null(i) {
                            String::new()
                        } else {
                            a.value(i).to_string()
                        }
                    })
                    .collect(),
            ))
        }
        // An all-null column still has a shape. `I8` is the narrowest carrier, and the mask is what
        // carries the meaning.
        DataType::Null => {
            return Ok(Some(ColumnData::Nullable {
                values: Box::new(ColumnData::I8(vec![0; array.len()])),
                validity: vec![false; array.len()],
            }))
        }
        _ => None,
    };
    Ok(values.map(|v| with_validity(array.as_ref(), v)))
}

/// `10⁻ˢ` as an **exact literal**, for the `Column.scale` a decimal column carries.
///
/// Not `10f64.powi(-s)`: `powi` is explicitly documented as permitted to differ between platforms and
/// between optimisation levels, and this value is **sealed** — it lands in `Column.scale`, inside
/// `manifest_hash`. A cross-platform difference in the last bit would make the same Parquet seal two
/// ways, which is hazard **H3**'s family (FMA-sensitive arithmetic) arriving through our own code rather
/// than a decoder's.
///
/// A table of literals is exact by construction: the compiler parses each with the same
/// correctly-rounded algorithm `str::parse` uses, and nothing is computed at runtime. Bounded at 18
/// because §2 rejects `Decimal128(p > 18)`, so no larger scale can reach here.
fn decimal_scale_factor(scale: i8) -> Result<f64> {
    const FACTORS: [f64; 19] = [
        1e0, 1e-1, 1e-2, 1e-3, 1e-4, 1e-5, 1e-6, 1e-7, 1e-8, 1e-9, 1e-10, 1e-11, 1e-12, 1e-13,
        1e-14, 1e-15, 1e-16, 1e-17, 1e-18,
    ];
    usize::try_from(scale)
        .ok()
        .and_then(|i| FACTORS.get(i).copied())
        .ok_or_else(|| {
            he(format!(
                "decimal scale {scale} is outside 0..=18, which precision ≤ 18 cannot reach"
            ))
        })
}

/// Seconds per tick for an Arrow time unit — the `Column.scale` a tick column carries.
fn tick_seconds(unit: &TimeUnit) -> f64 {
    match unit {
        TimeUnit::Second => 1.0,
        TimeUnit::Millisecond => 1e-3,
        TimeUnit::Microsecond => 1e-6,
        TimeUnit::Nanosecond => 1e-9,
    }
}

fn interval_name(unit: &IntervalUnit) -> &'static str {
    match unit {
        IntervalUnit::YearMonth => "year_month",
        IntervalUnit::DayTime => "day_time",
        IntervalUnit::MonthDayNano => "month_day_nano",
    }
}

/// A column of integer ticks **anchored to an epoch** — an absolute instant.
///
/// This is what `Column.referencing` was added for (ADR-0056 §6.1): `unit` + `scale` alone would make
/// the column indistinguishable from a duration, which ADR-0046 §2 forbids. The descriptor spells the
/// whole mapping out: `seconds-since-epoch = ticks × scale`, in frame `epoch:<instant>`.
fn epoch_column(
    b: &mut TableBuilder,
    name: &str,
    dtype: &str,
    scale: f64,
    instant: &str,
) -> Column {
    b.record(
        IngestTransform::new(transform::EPOCH_ANCHOR)
            .on_column(name)
            .with("epoch", serde_json::json!(instant))
            .with("scale", serde_json::json!(scale)),
    );
    unclassified_column(name, dtype)
        .with_unit("s")
        .with_scale(scale)
        .with_referencing(
            Referenced {
                transform: Transform::Affine1d {
                    slope: scale,
                    intercept: 0.0,
                },
                unit: Some("s".into()),
                vocabulary: None,
                frame: Some("epoch".into()),
            }
            .with_epoch(instant),
        )
}

/// A column of integer ticks with **no** epoch — an elapsed quantity or a time-of-day.
///
/// Deliberately carries no `referencing`: ADR-0046's model is that an elapsed quantity is *not*
/// anchored, and inventing an epoch for a `Duration` would assert something the source never said.
fn elapsed_column(name: &str, dtype: &str, scale: f64) -> Column {
    unclassified_column(name, dtype)
        .with_unit("s")
        .with_scale(scale)
}

/// The (values, logical→physical index) pair of a dictionary array.
fn dictionary_parts(
    array: &ArrayRef,
    key: &DataType,
    name: &str,
) -> Result<(ArrayRef, Vec<Option<usize>>)> {
    macro_rules! parts {
        ($t:ty) => {{
            let d = array.as_dictionary::<$t>();
            (
                d.values().clone(),
                (0..d.len())
                    .map(|i| if d.is_null(i) { None } else { d.key(i) })
                    .collect(),
            )
        }};
    }
    Ok(match key {
        DataType::Int8 => parts!(Int8Type),
        DataType::Int16 => parts!(Int16Type),
        DataType::Int32 => parts!(Int32Type),
        DataType::Int64 => parts!(Int64Type),
        DataType::UInt8 => parts!(UInt8Type),
        DataType::UInt16 => parts!(UInt16Type),
        DataType::UInt32 => parts!(UInt32Type),
        DataType::UInt64 => parts!(UInt64Type),
        other => {
            return Err(he(format!(
            "column '{name}' is a dictionary keyed by {other}, which Arrow does not define as a \
                 dictionary key type"
        )))
        }
    })
}

/// The (values, logical→physical index) pair of a run-end-encoded array.
fn run_end_parts(
    array: &ArrayRef,
    run_ends: &DataType,
    name: &str,
) -> Result<(ArrayRef, Vec<Option<usize>>)> {
    macro_rules! parts {
        ($t:ty) => {{
            let r = array
                .as_any()
                .downcast_ref::<arrow_array::RunArray<$t>>()
                .ok_or_else(|| he(format!("column '{name}': run-end array failed to downcast")))?;
            let values = r.values().clone();
            // Only the run-level mapping here; `gather` folds in the values array's own validity for
            // every lane, so a run whose *value* is null still yields a null row.
            let idx: Vec<Option<usize>> =
                (0..r.len()).map(|i| Some(r.get_physical_index(i))).collect();
            (values, idx)
        }};
    }
    Ok(match run_ends {
        DataType::Int16 => parts!(Int16Type),
        DataType::Int32 => parts!(Int32Type),
        DataType::Int64 => parts!(Int64Type),
        other => {
            return Err(he(format!(
                "column '{name}' is run-end encoded with {other} run ends, which Arrow does not \
                 define"
            )))
        }
    })
}

/// Materialise a column by taking `indices` out of `values`.
///
/// The single mechanism behind three lanes — dictionary materialisation, run-end decoding and
/// fixed-size-list expansion all reduce to "a logical→physical index map plus a values array", so
/// they share one implementation rather than three subtly different loops. `None` ⇒ that row is null.
///
/// Rather than gathering element-by-element through an `enum`, this maps the *whole* values array
/// once through [`map_scalar`] and then indexes the result. That keeps the dictionary/run-end lanes
/// byte-identical to the plain lane by construction: they cannot drift apart, because there is only
/// one mapping.
fn gather(values: &ArrayRef, indices: &[Option<usize>], name: &str) -> Result<ColumnData> {
    let mapped = map_scalar(values.data_type(), values)?.ok_or_else(|| {
        he(format!(
            "column '{name}': encoded column's values are {}, which has no flat mapping (an encoding \
             may only wrap a scalar type)",
            values.data_type()
        ))
    })?;
    // **Fold the values array's OWN validity into the index map**, so a live index pointing at a null
    // value is a null row.
    //
    // This is the correctness heart of the shared gather, and its absence was a real defect: each
    // lane's `*_parts` derived `indices` from *its own* level of nullity (a null dictionary key, a null
    // list slot) and only the run-end lane also consulted the values array. So a
    // `Dictionary<Int32, Utf8>` whose *values* contained a null decoded that row to a **present
    // zero** — non-nullable, no transform record, a corrupt product that verifies. It also broke the
    // dictionary lane's entire justification (ADR-0056 §2: a dictionary must produce the column a plain
    // array would), because the plain column is *rejected* when it is a nullable `str` while the
    // encoded one silently sealed a wrong value.
    //
    // Doing it here rather than in each `*_parts` is what makes the three lanes equivalent **by
    // construction** instead of by three authors remembering the same rule.
    let resolved: Vec<Option<usize>> = indices
        .iter()
        .map(|i| match i {
            Some(p) if values.is_null(*p) => None,
            other => *other,
        })
        .collect();
    let indices = resolved.as_slice();
    // `map_scalar` may itself have wrapped the values in `Nullable`. Unwrap to the flat values; the
    // mask built below is the authoritative one, because `indices` now accounts for nullity at every
    // level — the key/slot AND the value it points at.
    let flat = match mapped {
        ColumnData::Nullable { values, .. } => *values,
        other => other,
    };
    let has_null = indices.iter().any(Option::is_none);
    macro_rules! take {
        ($v:expr, $zero:expr, $variant:path) => {{
            let src = $v;
            let mut out = Vec::with_capacity(indices.len());
            for (row, idx) in indices.iter().enumerate() {
                match idx {
                    None => out.push($zero),
                    Some(i) => out.push(
                        src.get(*i)
                            .ok_or_else(|| {
                                he(format!(
                                    "column '{name}': encoded index {i} at row {row} is out of range \
                                     for {} values",
                                    src.len()
                                ))
                            })?
                            .clone(),
                    ),
                }
            }
            $variant(out)
        }};
    }
    let taken = match flat {
        ColumnData::I8(v) => take!(v, 0, ColumnData::I8),
        ColumnData::I16(v) => take!(v, 0, ColumnData::I16),
        ColumnData::I32(v) => take!(v, 0, ColumnData::I32),
        ColumnData::I64(v) => take!(v, 0, ColumnData::I64),
        ColumnData::U8(v) => take!(v, 0, ColumnData::U8),
        ColumnData::U16(v) => take!(v, 0, ColumnData::U16),
        ColumnData::U32(v) => take!(v, 0, ColumnData::U32),
        ColumnData::U64(v) => take!(v, 0, ColumnData::U64),
        ColumnData::F32(v) => take!(v, 0.0, ColumnData::F32),
        ColumnData::F64(v) => take!(v, 0.0, ColumnData::F64),
        ColumnData::Bool(v) => take!(v, false, ColumnData::Bool),
        ColumnData::Utf8(v) => take!(v, String::new(), ColumnData::Utf8),
        ColumnData::Nullable { .. } => {
            unreachable!("the Nullable wrapper was stripped above")
        }
    };
    Ok(if has_null {
        ColumnData::Nullable {
            values: Box::new(taken),
            validity: indices.iter().map(Option::is_some).collect(),
        }
    } else {
        taken
    })
}

/// Lazily canonicalised Arrow IPC chunks — the streaming counterpart to [`read_arrow_table`], and
/// the IPC twin of `parquet_table::parquet_chunks`.
///
/// Here rather than beside the Parquet chunker for the reason #509/#540 moved [`read_arrow_table`]
/// here: `parquet_table` is `#[cfg(feature = "parquet")]`, so an IPC reader living there makes
/// `--features arrow` a configuration nobody can build. Nothing in it needs Parquet — it calls
/// [`arrow_ipc_batches`] and [`canonicalise_batch`], both right here.
///
/// There is no `batch_rows` knob, deliberately: the IPC container's own record batches ARE the read
/// unit, and re-cutting them would buffer exactly what streaming exists to avoid.
pub fn arrow_ipc_chunks(
    path: &std::path::Path,
    exclude: &[String],
) -> Result<impl Iterator<Item = Result<CanonicalTable>> + use<>> {
    let exclude = exclude.to_vec();
    Ok(arrow_ipc_batches(path)?.map(move |b| b.and_then(|b| canonicalise_batch(&b, &exclude))))
}

/// Read + canonicalise an Arrow IPC / Feather file into a flat Tessera table.
///
/// Lives here rather than beside the Parquet entry point, which is where it started. Keeping the two
/// `--from` entry points side by side read better, but it gated the **Arrow IPC** reader behind the
/// `parquet` feature and so made `--features arrow` a configuration nobody could build (#509) — which
/// also meant the `arrow-rs/arrow-ipc` decoder digest described a build that could not exist. Nothing
/// in it ever needed Parquet: it calls [`read_arrow_ipc`] and [`canonicalise_batches`], both right
/// here. Readability of a pair of call sites is not worth a lane that cannot be selected.
pub fn read_arrow_table(path: &std::path::Path, exclude: &[String]) -> Result<CanonicalTable> {
    let batches = read_arrow_ipc(path)?;
    canonicalise_batches(&batches, exclude)
}

/// A **lazy** Arrow IPC batch reader — the streaming counterpart to [`read_arrow_ipc`], which collects.
///
/// Returns a fresh reader per call, because streaming needs two traversals: nullability is a whole-file
/// property (ADR-0029 by-presence), so the shape pass must finish before the encode pass knows what
/// schema to declare. An IPC *file* carries a footer with the record-batch index, so it is seekable and a
/// second pass is I/O rather than a correctness problem. (The IPC *stream* format has no footer and would
/// not be re-openable; `FileReader` already refuses it, which is why that distinction stays out of here.)
pub fn arrow_ipc_batches(
    path: &std::path::Path,
) -> Result<impl Iterator<Item = Result<RecordBatch>> + use<>> {
    let file =
        std::fs::File::open(path).map_err(|e| he(format!("open {}: {e}", path.display())))?;
    let reader = arrow_ipc::reader::FileReader::try_new(std::io::BufReader::new(file), None)
        .map_err(|e| {
            he(format!(
                "{} is not a readable Arrow IPC file: {e} (a Feather v1 file is not Arrow IPC; \
                 re-save it as Feather v2 / .arrow)",
                path.display()
            ))
        })?;
    let display = path.display().to_string();
    Ok(reader.map(move |b| b.map_err(|e| he(format!("read {display}: {e}")))))
}

/// The decoded size an Arrow IPC file implies, for the streaming threshold.
///
/// Metadata only: the footer's schema gives the row width and each record-batch block its length, so no
/// buffer is decoded. As with Parquet, reading the footer to choose a code path is sound where trusting it
/// for *identity* would not be (#502) — a wrong estimate costs speed, and batch==stream is pinned by test.
/// **Routing only**, and a deliberate under-estimate for compressed IPC — see the comment inside.
pub fn arrow_ipc_size_estimate(path: &std::path::Path) -> Result<u64> {
    let file =
        std::fs::File::open(path).map_err(|e| he(format!("open {}: {e}", path.display())))?;
    let md =
        std::fs::File::metadata(&file).map_err(|e| he(format!("stat {}: {e}", path.display())))?;
    // On-disk size, which is a FLOOR and is documented as one. IPC is uncompressed by default, so for
    // the common file it already IS the decoded size — but the format permits per-buffer LZ4_FRAME or
    // ZSTD compression, and a compressed file therefore under-estimates. Reading the footer's
    // compression field per record batch would tighten it; it is not worth the open, because the only
    // consequence of under-estimating is that a large compressed IPC file routes to the whole-file
    // path when streaming would have served it better. Routing cannot move a hash (the
    // batch-equals-stream tests pin that), so a conservative floor is the right trade and a wrong
    // guess costs memory, never correctness.
    Ok(md.len())
}

/// Read an **Arrow IPC / Feather** file into record batches.
///
/// The `--from arrow` source. Arrow IPC is the one container whose on-disk logical types are exactly
/// Arrow's, so it needs no format-specific mapping at all beyond this read — the whole of §2 above is
/// shared with Parquet.
pub fn read_arrow_ipc(path: &std::path::Path) -> Result<Vec<RecordBatch>> {
    let file =
        std::fs::File::open(path).map_err(|e| he(format!("open {}: {e}", path.display())))?;
    let reader = arrow_ipc::reader::FileReader::try_new(std::io::BufReader::new(file), None)
        .map_err(|e| {
            he(format!(
                "{} is not a readable Arrow IPC file: {e} (a Feather v1 file is not Arrow IPC; \
                 re-save it as Feather v2 / .arrow)",
                path.display()
            ))
        })?;
    let mut out = Vec::new();
    for batch in reader {
        out.push(batch.map_err(|e| he(format!("read {}: {e}", path.display())))?);
    }
    Ok(out)
}

/// A single-batch convenience for the tests + the array lane's structured-npy path: canonicalise one
/// `RecordBatch`.
pub fn canonicalise_batch(batch: &RecordBatch, exclude: &[String]) -> Result<CanonicalTable> {
    canonicalise_batches(std::slice::from_ref(batch), exclude)
}

/// Build a `RecordBatch` from named arrays — used by the tests and by fixture generation.
pub fn record_batch(columns: Vec<(&str, ArrayRef)>) -> Result<RecordBatch> {
    let fields: Vec<Arc<Field>> = columns
        .iter()
        .map(|(n, a)| Arc::new(Field::new(*n, a.data_type().clone(), a.null_count() > 0)))
        .collect();
    let schema = Arc::new(arrow_schema::Schema::new(fields));
    RecordBatch::try_new(schema, columns.into_iter().map(|(_, a)| a).collect())
        .map_err(|e| he(format!("build record batch: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::builder::{FixedSizeListBuilder, Int32Builder, StringDictionaryBuilder};
    use arrow_array::{
        BinaryArray, BooleanArray, Date32Array, Date64Array, Decimal128Array, DurationSecondArray,
        Float16Array, Float32Array, Float64Array, Int32Array, Int64Array, Int8Array,
        IntervalYearMonthArray, LargeStringArray, ListArray, NullArray, RunArray, StringArray,
        StringViewArray, StructArray, Time64MicrosecondArray, TimestampMicrosecondArray,
        UInt16Array,
    };
    use arrow_buffer::OffsetBuffer;
    use arrow_schema::Fields;

    fn one(name: &str, a: ArrayRef) -> Result<CanonicalTable> {
        canonicalise_batch(&record_batch(vec![(name, a)])?, &[])
    }

    /// The column, unwrapped from any nullable wrapper — for the assertions that care about values.
    fn values_of(t: &CanonicalTable, i: usize) -> ColumnData {
        match &t.columns[i].1 {
            ColumnData::Nullable { values, .. } => (**values).clone(),
            other => other.clone(),
        }
    }

    fn transforms(t: &CanonicalTable) -> Vec<&str> {
        t.transforms.iter().map(|x| x.name.as_str()).collect()
    }

    // ── the clean lane ────────────────────────────────────────────────────────────────────────

    #[test]
    fn integers_floats_bools_and_strings_map_clean_and_keep_their_width() {
        let t = canonicalise_batch(
            &record_batch(vec![
                ("i8", Arc::new(Int8Array::from(vec![-1, 2])) as ArrayRef),
                ("u16", Arc::new(UInt16Array::from(vec![7, 8]))),
                ("f32", Arc::new(Float32Array::from(vec![1.5, -0.5]))),
                ("f64", Arc::new(Float64Array::from(vec![1.5, -0.5]))),
                ("b", Arc::new(BooleanArray::from(vec![true, false]))),
                ("s", Arc::new(StringArray::from(vec!["a", "b"]))),
            ])
            .unwrap(),
            &[],
        )
        .unwrap();
        // Width is preserved exactly — an i8 is NOT widened to the machine word.
        let codes: Vec<&str> = t.columns.iter().map(|(c, _)| c.dtype.as_str()).collect();
        assert_eq!(codes, vec!["i1", "u2", "f4", "f8", "b1", "str"]);
        assert_eq!(values_of(&t, 0), ColumnData::I8(vec![-1, 2]));
        assert_eq!(
            values_of(&t, 5),
            ColumnData::Utf8(vec!["a".into(), "b".into()])
        );
        // Nothing was transformed, so the receipt is empty. A clean ingest must not claim otherwise.
        assert!(t.transforms.is_empty(), "got {:?}", transforms(&t));
    }

    /// A `BooleanArray` is bit-packed and its trailing bits are unspecified padding. Reading whole
    /// bytes would put producer noise in the column, so the map iterates exactly `len` bits — proven
    /// here with a length that is not a multiple of 8.
    #[test]
    fn boolean_reads_exactly_len_bits_never_the_trailing_padding() {
        let bits: Vec<bool> = (0..13).map(|i| i % 3 == 0).collect();
        let t = one("b", Arc::new(BooleanArray::from(bits.clone()))).unwrap();
        assert_eq!(values_of(&t, 0), ColumnData::Bool(bits));
    }

    /// The three UTF-8 layouts are *runtime encodings* of one logical type, so all three must produce
    /// the identical column — otherwise a producer's buffer-layout choice would reach `content_hash`.
    #[test]
    fn all_three_utf8_layouts_produce_one_identical_column() {
        let want = ColumnData::Utf8(vec!["alpha".into(), "beta".into()]);
        let small = one("s", Arc::new(StringArray::from(vec!["alpha", "beta"]))).unwrap();
        let large = one("s", Arc::new(LargeStringArray::from(vec!["alpha", "beta"]))).unwrap();
        let view = one("s", Arc::new(StringViewArray::from(vec!["alpha", "beta"]))).unwrap();
        assert_eq!(values_of(&small, 0), want);
        assert_eq!(values_of(&large, 0), want);
        assert_eq!(values_of(&view, 0), want);
    }

    #[test]
    fn an_all_null_column_keeps_its_shape_as_an_absent_mask() {
        let t = one("n", Arc::new(NullArray::new(4))).unwrap();
        let ColumnData::Nullable { values, validity } = &t.columns[0].1 else {
            panic!("expected a mask, got {:?}", t.columns[0].1)
        };
        assert_eq!(**values, ColumnData::I8(vec![0; 4]));
        assert_eq!(validity, &vec![false; 4]);
        assert!(t.columns[0].0.nullable);
    }

    // ── nullable-by-presence: the rule the three-producer fixture rides on ────────────────────

    #[test]
    fn nullability_follows_the_data_not_the_declared_field() {
        // Declared nullable (the Field is built with nullable = null_count > 0, so force it here by
        // constructing the schema directly) but containing no nulls ⇒ NO wrapper.
        let arr: ArrayRef = Arc::new(Int32Array::from(vec![1, 2, 3]));
        let schema = Arc::new(arrow_schema::Schema::new(vec![Field::new(
            "x",
            DataType::Int32,
            /* nullable = */ true,
        )]));
        let batch = RecordBatch::try_new(schema, vec![arr]).unwrap();
        let t = canonicalise_batch(&batch, &[]).unwrap();
        assert_eq!(
            t.columns[0].1,
            ColumnData::I32(vec![1, 2, 3]),
            "a nullable declaration with no nulls must not add a mask — pyarrow, polars and DuckDB \
             disagree about that declaration, and honouring it would seal one table three ways"
        );
        assert!(!t.columns[0].0.nullable);

        // Actually containing a null ⇒ wrapper.
        let t = one(
            "x",
            Arc::new(Int32Array::from(vec![Some(1), None, Some(3)])),
        )
        .unwrap();
        assert!(matches!(t.columns[0].1, ColumnData::Nullable { .. }));
        assert!(t.columns[0].0.nullable);
    }

    /// Chunked sources decide nullability per chunk, so the *fold* is where promotion must happen: a
    /// column whose only null is in the second batch is one nullable column, not an error.
    #[test]
    fn a_null_in_only_one_chunk_still_yields_one_nullable_column() {
        let b1 = record_batch(vec![(
            "x",
            Arc::new(Int32Array::from(vec![1, 2])) as ArrayRef,
        )])
        .unwrap();
        let b2 = record_batch(vec![(
            "x",
            Arc::new(Int32Array::from(vec![Some(3), None])) as ArrayRef,
        )])
        .unwrap();
        let t = canonicalise_batches(&[b1, b2], &[]).unwrap();
        let ColumnData::Nullable { values, validity } = &t.columns[0].1 else {
            panic!(
                "expected one merged nullable column, got {:?}",
                t.columns[0].1
            )
        };
        assert_eq!(**values, ColumnData::I32(vec![1, 2, 3, 0]));
        assert_eq!(validity, &vec![true, true, true, false]);
    }

    /// **H5** — whatever the producer left under a null is gone, and the fact is recorded.
    ///
    /// The array is built from a values buffer plus a separate null mask, **not** from an `Option`
    /// iterator: an `Option` iterator writes the dtype default into masked slots, so a test built that
    /// way asserts nothing about H5 at all. This is the only construction that puts real producer
    /// garbage under a null, and it is also the only way the hazard can occur in practice — Parquet
    /// stores no values under its nulls, so H5 is reachable exactly at this in-memory arrow boundary
    /// (a library consumer handing us a `RecordBatch`), which is why the corpus cannot cover it and
    /// this test must.
    #[test]
    fn garbage_under_a_null_is_zeroed_and_recorded() {
        let values = arrow_buffer::ScalarBuffer::<i32>::from(vec![1, i32::MAX, 3]);
        let nulls = arrow_buffer::NullBuffer::from(vec![true, false, true]);
        let arr = arrow_array::PrimitiveArray::<Int32Type>::new(values, Some(nulls));
        // The garbage really is there before we touch it — otherwise this test would be vacuous.
        assert_eq!(arr.values()[1], i32::MAX);

        let t = one("x", Arc::new(arr)).unwrap();
        let ColumnData::Nullable { values, validity } = &t.columns[0].1 else {
            panic!("expected a mask")
        };
        assert_eq!(
            **values,
            ColumnData::I32(vec![1, 0, 3]),
            "the masked slot must be the dtype default, not the producer's leftover"
        );
        assert_eq!(validity, &vec![true, false, true]);
        assert!(transforms(&t).contains(&transform::NULL_SLOT_NORMALISATION));
    }

    // ── the recorded lane ─────────────────────────────────────────────────────────────────────

    #[test]
    fn float16_widens_exactly_and_records_it() {
        let a = Float16Array::from(vec![
            half::f16::from_f32(1.5),
            half::f16::from_f32(-0.25),
            half::f16::from_f32(0.0),
        ]);
        let t = one("h", Arc::new(a)).unwrap();
        assert_eq!(t.columns[0].0.dtype, "f4");
        assert_eq!(values_of(&t, 0), ColumnData::F32(vec![1.5, -0.25, 0.0]));
        assert_eq!(transforms(&t), vec![transform::F16_WIDEN]);
    }

    #[test]
    fn decimal_becomes_fixed_point_integers_never_floats() {
        // 123.4567 and -0.0001 at scale 4.
        let a = Decimal128Array::from(vec![1_234_567_i128, -1_i128])
            .with_precision_and_scale(18, 4)
            .unwrap();
        let t = one("amount", Arc::new(a)).unwrap();
        assert_eq!(t.columns[0].0.dtype, "i8", "an integer carrier, not f8");
        assert_eq!(values_of(&t, 0), ColumnData::I64(vec![1_234_567, -1]));
        // physical = raw × scale, so 1234567 × 1e-4 = 123.4567 exactly.
        assert_eq!(t.columns[0].0.scale, Some(1e-4));
        let rec = t
            .transforms
            .iter()
            .find(|x| x.name == transform::DECIMAL_FIXED_POINT)
            .expect("recorded");
        assert_eq!(rec.params["precision"], 18);
        assert_eq!(rec.params["scale"], 1e-4);
    }

    /// The sealed `Column.scale` must be an **exact literal**, never `10f64.powi(-s)`: `powi` may differ
    /// across platforms and optimisation levels, and this value rides inside `manifest_hash`. For several
    /// scales the two differ in the last bit, which is precisely how a cross-platform seal split starts.
    #[test]
    fn the_decimal_scale_factor_is_an_exact_literal() {
        for (scale, want) in [
            (0i8, 1e0f64),
            (1, 1e-1),
            (2, 1e-2),
            (4, 1e-4),
            (6, 1e-6),
            (10, 1e-10),
            (18, 1e-18),
        ] {
            let got = decimal_scale_factor(scale).unwrap();
            assert_eq!(
                got.to_bits(),
                want.to_bits(),
                "scale {scale}: {got:e} is not the literal {want:e}"
            );
            // …and the literal is what `str::parse` gives, i.e. correctly rounded.
            assert_eq!(got, format!("1e-{scale}").parse::<f64>().unwrap());
        }
        // Out of range for precision ≤ 18 — unreachable through the public path, but total.
        assert!(decimal_scale_factor(19).is_err());
        assert!(decimal_scale_factor(-1).is_err());
    }

    #[test]
    fn a_decimal_too_wide_for_an_integer_carrier_is_rejected_with_a_next_command() {
        let a = Decimal128Array::from(vec![1_i128])
            .with_precision_and_scale(28, 6)
            .unwrap();
        let err = one("amount", Arc::new(a)).unwrap_err().to_string();
        assert!(err.contains("decimal128(28,6)"), "got {err}");
        assert!(err.contains("above precision 18"), "got {err}");
        assert!(err.contains("--exclude amount"), "offers a next command");
        assert!(err.contains("ingest blob"), "offers the preserve tier");
    }

    /// ADR-0056 §6.1 / ADR-0046 §2: an absolute instant is ticks **plus an epoch**. Without the
    /// descriptor this column would be indistinguishable from a duration.
    #[test]
    fn timestamps_carry_raw_ticks_plus_an_epoch_anchor() {
        // 2024-01-01T00:00:00Z in microseconds.
        let ticks = 1_704_067_200_000_000_i64;
        let t = one(
            "acq",
            Arc::new(TimestampMicrosecondArray::from(vec![ticks])),
        )
        .unwrap();
        let col = &t.columns[0].0;
        assert_eq!(col.dtype, "i8");
        assert_eq!(values_of(&t, 0), ColumnData::I64(vec![ticks]), "RAW ticks");
        assert_eq!(col.unit.as_deref(), Some("s"));
        assert_eq!(col.scale, Some(1e-6));
        let r = col.referencing.as_ref().expect("the epoch slot is filled");
        assert_eq!(r.frame.as_deref(), Some("epoch:unix"));
        assert!(
            r.vocabularies_pinned(),
            "frame + unit are pinned vocabulary"
        );
        assert_eq!(
            r.transform,
            Transform::Affine1d {
                slope: 1e-6,
                intercept: 0.0
            },
            "seconds-since-epoch = ticks × 1e-6"
        );
        assert!(transforms(&t).contains(&transform::EPOCH_ANCHOR));
    }

    /// Hazard H1: a zone-annotated timestamp keeps the same raw ticks (Arrow already stores UTC) and
    /// the zone is **sealed** rather than dropped. That is the only reason calling it lossless is true.
    #[test]
    fn a_zoned_timestamp_keeps_utc_ticks_and_seals_the_source_zone() {
        let ticks = 1_704_067_200_000_000_i64;
        let naive: ArrayRef = Arc::new(TimestampMicrosecondArray::from(vec![ticks]));
        let zoned: ArrayRef = Arc::new(
            TimestampMicrosecondArray::from(vec![ticks]).with_timezone("America/New_York"),
        );
        let a = one("t", naive).unwrap();
        let b = one("t", zoned).unwrap();
        assert_eq!(
            values_of(&a, 0),
            values_of(&b, 0),
            "the zone is a display annotation; no tzdb is consulted, so the ticks are identical"
        );
        assert!(!transforms(&a).contains(&transform::TZ_TO_UTC));
        let rec = b
            .transforms
            .iter()
            .find(|x| x.name == transform::TZ_TO_UTC)
            .expect("the source zone is recorded");
        assert_eq!(rec.params["from"], "America/New_York");
    }

    #[test]
    fn dates_anchor_to_the_epoch_with_their_own_tick_size() {
        let d32 = one("d", Arc::new(Date32Array::from(vec![19723]))).unwrap();
        assert_eq!(
            d32.columns[0].0.scale,
            Some(86_400.0),
            "a Date32 tick is a day"
        );
        assert_eq!(
            d32.columns[0]
                .0
                .referencing
                .as_ref()
                .unwrap()
                .frame
                .as_deref(),
            Some("epoch:unix")
        );
        let d64 = one("d", Arc::new(Date64Array::from(vec![1_704_067_200_000]))).unwrap();
        assert_eq!(d64.columns[0].0.scale, Some(1e-3), "a Date64 tick is a ms");
    }

    /// The counterpart: an *elapsed* quantity must NOT get an epoch, or the format would assert an
    /// anchor the source never gave (ADR-0046).
    #[test]
    fn durations_and_times_of_day_carry_no_epoch() {
        let dur = one("d", Arc::new(DurationSecondArray::from(vec![90]))).unwrap();
        assert_eq!(dur.columns[0].0.unit.as_deref(), Some("s"));
        assert_eq!(dur.columns[0].0.scale, Some(1.0));
        assert!(
            dur.columns[0].0.referencing.is_none(),
            "a duration is not anchored to anything"
        );
        let tod = one(
            "t",
            Arc::new(Time64MicrosecondArray::from(vec![3_600_000_000])),
        )
        .unwrap();
        assert!(tod.columns[0].0.referencing.is_none());
        assert_eq!(tod.columns[0].0.scale, Some(1e-6));
    }

    /// The determinism property the dictionary lane exists for: materialising to values must give the
    /// **byte-identical** column a plain string array would, so three producers that differ only in
    /// whether (and how) they dictionary-encode seal to one hash.
    #[test]
    fn a_dictionary_materialises_to_exactly_the_plain_column() {
        let mut b = StringDictionaryBuilder::<arrow_array::types::Int32Type>::new();
        for v in ["red", "green", "red", "red", "green"] {
            b.append_value(v);
        }
        let dict = one("c", Arc::new(b.finish())).unwrap();
        let plain = one(
            "c",
            Arc::new(StringArray::from(vec![
                "red", "green", "red", "red", "green",
            ])),
        )
        .unwrap();
        assert_eq!(
            values_of(&dict, 0),
            values_of(&plain, 0),
            "the source's codes and dictionary ORDER must not reach the column"
        );
        assert_eq!(dict.columns[0].0.dtype, "str");
        assert_eq!(transforms(&dict), vec![transform::DICTIONARY_MATERIALISED]);
        // And the plain one records nothing, so the receipt distinguishes the two ingests.
        assert!(plain.transforms.is_empty());
    }

    #[test]
    fn a_run_end_encoded_column_decodes_to_exactly_the_plain_column() {
        let run_ends = Int32Array::from(vec![2, 5]);
        let values = Int32Array::from(vec![7, 9]);
        let rle: ArrayRef = Arc::new(RunArray::try_new(&run_ends, &values).unwrap());
        let decoded = one("x", rle).unwrap();
        let plain = one("x", Arc::new(Int32Array::from(vec![7, 7, 9, 9, 9]))).unwrap();
        assert_eq!(values_of(&decoded, 0), values_of(&plain, 0));
        assert_eq!(transforms(&decoded), vec![transform::RUN_END_DECODED]);
    }

    // ── nesting: flatten structs, expand narrow tuples, reject the rest ───────────────────────

    #[test]
    fn a_struct_flattens_to_dotted_columns() {
        let inner = StructArray::from(vec![
            (
                Arc::new(Field::new("x", DataType::Int32, false)),
                Arc::new(Int32Array::from(vec![1, 2])) as ArrayRef,
            ),
            (
                Arc::new(Field::new("y", DataType::Utf8, false)),
                Arc::new(StringArray::from(vec!["a", "b"])) as ArrayRef,
            ),
        ]);
        let t = one("pos", Arc::new(inner)).unwrap();
        assert_eq!(
            t.columns
                .iter()
                .map(|(c, _)| c.name.as_str())
                .collect::<Vec<_>>(),
            vec!["pos.x", "pos.y"],
            "dotted, which is how pandas/polars/DuckDB already present nested Parquet"
        );
        assert_eq!(values_of(&t, 0), ColumnData::I32(vec![1, 2]));
        assert!(transforms(&t).contains(&transform::STRUCT_FLATTEN));
    }

    /// **A null STRUCT row must null every column it flattened into.**
    ///
    /// Regression test, same class as the dictionary-values bug: Arrow permits perfectly valid child
    /// data underneath a null parent row (`pyarrow.StructArray.from_arrays(..., mask=…)` writes exactly
    /// that, and it survives an Arrow IPC round-trip), and the flatten walked each child directly. So a
    /// row the source says is **absent** sealed as present child values, unmasked and unrecorded.
    ///
    /// A struct flatten is a *renaming* (§3), and a renaming must not change which rows exist.
    #[test]
    fn a_null_struct_row_nulls_every_column_it_flattened_into() {
        let x: ArrayRef = Arc::new(Int32Array::from(vec![1, 2, 3]));
        let z: ArrayRef = Arc::new(Float64Array::from(vec![1.5, 2.5, 3.5]));
        // Row 1 is an absent struct, and the child arrays still carry live values there.
        let nulls = arrow_buffer::NullBuffer::from(vec![true, false, true]);
        let st = StructArray::try_new(
            Fields::from(vec![
                Field::new("x", DataType::Int32, false),
                Field::new("z", DataType::Float64, false),
            ]),
            vec![x, z],
            Some(nulls),
        )
        .unwrap();
        assert_eq!(
            st.column(0).as_primitive::<Int32Type>().value(1),
            2,
            "live child data under the null"
        );

        let t = one("pos", Arc::new(st)).unwrap();
        let by = |n: &str| {
            t.columns
                .iter()
                .find(|(c, _)| c.name == n)
                .map(|(c, d)| (c.clone(), d.clone()))
                .unwrap_or_else(|| panic!("column '{n}' missing"))
        };
        // EVERY column the struct flattened into is masked, not just the first.
        for (name, want) in [
            ("pos.x", ColumnData::I32(vec![1, 0, 3])),
            ("pos.z", ColumnData::F64(vec![1.5, 0.0, 3.5])),
        ] {
            let (col, data) = by(name);
            let ColumnData::Nullable { values, validity } = &data else {
                panic!("'{name}': a null struct row must mask its columns, got {data:?}")
            };
            assert_eq!(**values, want, "'{name}' masked slot is the dtype default");
            assert_eq!(validity, &vec![true, false, true], "'{name}'");
            assert!(col.nullable, "'{name}'");
        }

        // The `str` child is the sharper case: a nullable `str` column is *unrepresentable*
        // (#457), so the parent's validity must turn it into a clean rejection rather than a
        // silently-present value.
        let err = one(
            "pos",
            Arc::new(
                StructArray::try_new(
                    Fields::from(vec![Field::new("y", DataType::Utf8, false)]),
                    vec![Arc::new(StringArray::from(vec!["a", "b"])) as ArrayRef],
                    Some(arrow_buffer::NullBuffer::from(vec![true, false])),
                )
                .unwrap(),
            ),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("nullable 'str'"), "got {err}");
    }

    /// Nested structs: the parent's absence has to reach the leaf, not just the level below it.
    #[test]
    fn a_null_outer_struct_nulls_a_nested_leaf() {
        let deep: ArrayRef = Arc::new(Int32Array::from(vec![5, 6]));
        let inner = StructArray::try_new(
            Fields::from(vec![Field::new("deep", DataType::Int32, false)]),
            vec![deep],
            None,
        )
        .unwrap();
        let outer = StructArray::try_new(
            Fields::from(vec![Field::new(
                "inner",
                DataType::Struct(Fields::from(vec![Field::new(
                    "deep",
                    DataType::Int32,
                    false,
                )])),
                true,
            )]),
            vec![Arc::new(inner) as ArrayRef],
            // The OUTER struct's row 0 is absent; the inner struct and the leaf are both fully valid.
            Some(arrow_buffer::NullBuffer::from(vec![false, true])),
        )
        .unwrap();
        let t = one("outer", Arc::new(outer)).unwrap();
        let ColumnData::Nullable { values, validity } = &t.columns[0].1 else {
            panic!("expected a mask, got {:?}", t.columns[0].1)
        };
        assert_eq!(t.columns[0].0.name, "outer.inner.deep");
        assert_eq!(**values, ColumnData::I32(vec![0, 6]));
        assert_eq!(validity, &vec![false, true]);
    }

    /// §3: a flatten is a renaming, and a renaming that collides is an error — never a silent suffix,
    /// because a silently-suffixed column is one a reader's projection will never find.
    #[test]
    fn a_flatten_that_collides_with_a_real_column_is_an_error() {
        let inner = StructArray::from(vec![(
            Arc::new(Field::new("x", DataType::Int32, false)),
            Arc::new(Int32Array::from(vec![1, 2])) as ArrayRef,
        )]);
        let batch = record_batch(vec![
            ("pos.x", Arc::new(Int32Array::from(vec![9, 9])) as ArrayRef),
            ("pos", Arc::new(inner) as ArrayRef),
        ])
        .unwrap();
        let err = canonicalise_batch(&batch, &[]).unwrap_err().to_string();
        assert!(err.contains("duplicate column name 'pos.x'"), "got {err}");
    }

    #[test]
    fn a_narrow_fixed_size_list_expands_into_numbered_columns() {
        let mut b = FixedSizeListBuilder::new(Int32Builder::new(), 3);
        for row in [[1, 2, 3], [4, 5, 6]] {
            b.values().append_slice(&row);
            b.append(true);
        }
        let t = one("xyz", Arc::new(b.finish())).unwrap();
        assert_eq!(
            t.columns
                .iter()
                .map(|(c, _)| c.name.as_str())
                .collect::<Vec<_>>(),
            vec!["xyz.0", "xyz.1", "xyz.2"]
        );
        assert_eq!(values_of(&t, 0), ColumnData::I32(vec![1, 4]));
        assert_eq!(values_of(&t, 2), ColumnData::I32(vec![3, 6]));
        let rec = t
            .transforms
            .iter()
            .find(|x| x.name == transform::FIXED_LIST_EXPAND)
            .expect("recorded");
        assert_eq!(rec.params["n"], 3);
    }

    /// §1: a wide fixed list is a flattened grid, and its home is the ARRAY primitive. Minting 262144
    /// columns would perpetuate the misuse silently — so the error routes to `analyze` instead.
    #[test]
    fn a_wide_fixed_size_list_is_routed_to_the_array_primitive_not_expanded() {
        let mut b = FixedSizeListBuilder::new(Int32Builder::new(), 64);
        b.values().append_slice(&vec![0; 64]);
        b.append(true);
        let err = one("voxels", Arc::new(b.finish())).unwrap_err().to_string();
        assert!(err.contains("wider than 8"), "got {err}");
        assert!(err.contains("ARRAY primitive"), "got {err}");
        assert!(
            err.contains("ingest analyze"),
            "points at the judgment step"
        );
    }

    // ── the reject lane: every message names the column AND a runnable next command ───────────

    #[test]
    fn opaque_bytes_lists_maps_and_intervals_are_rejected_with_escape_hatches() {
        let cases: Vec<(&str, ArrayRef, &str)> = vec![
            (
                "payload",
                Arc::new(BinaryArray::from(vec![b"\x00\x01".as_ref()])),
                "no opaque byte columns",
            ),
            (
                "span",
                Arc::new(IntervalYearMonthArray::from(vec![13])),
                "calendar arithmetic",
            ),
            (
                "tags",
                Arc::new(ListArray::new(
                    Arc::new(Field::new("item", DataType::Int32, false)),
                    OffsetBuffer::new(vec![0, 2].into()),
                    Arc::new(Int32Array::from(vec![1, 2])),
                    None,
                )),
                "TableSpec.rows is a single number",
            ),
        ];
        for (name, array, needle) in cases {
            let err = one(name, array).unwrap_err().to_string();
            assert!(err.contains(name), "the error names the column: {err}");
            assert!(err.contains(needle), "expected '{needle}' in: {err}");
            assert!(
                err.contains("--exclude") && err.contains("ingest blob"),
                "every rejection offers a runnable next command: {err}"
            );
        }
    }

    /// **A typo in a dotted `--exclude` must be an error, not a silent no-op.**
    ///
    /// Only the part before the dot was checked, so `--exclude patient.nmae` passed validation and then
    /// matched nothing — and the column the operator believed they had dropped was **sealed**. That is
    /// the same false-confidence failure the `--column-meta` typo check exists to prevent, and it is
    /// worse here because the column in question is the one they thought was PHI.
    #[test]
    fn a_typo_in_a_dotted_exclude_is_an_error_not_a_silent_no_op() {
        let inner = StructArray::from(vec![
            (
                Arc::new(Field::new("mrn", DataType::Int32, false)),
                Arc::new(Int32Array::from(vec![1, 2])) as ArrayRef,
            ),
            (
                Arc::new(Field::new("age", DataType::Int32, false)),
                Arc::new(Int32Array::from(vec![30, 40])) as ArrayRef,
            ),
        ]);
        let batch = record_batch(vec![("patient", Arc::new(inner) as ArrayRef)]).unwrap();

        // The real leaf works…
        let t = canonicalise_batch(&batch, &["patient.mrn".to_string()]).unwrap();
        assert_eq!(
            t.columns
                .iter()
                .map(|(c, _)| c.name.as_str())
                .collect::<Vec<_>>(),
            vec!["patient.age"]
        );

        // …and a typo in the CHILD is refused, listing the leaves that exist.
        let err = canonicalise_batch(&batch, &["patient.nmae".to_string()])
            .unwrap_err()
            .to_string();
        assert!(err.contains("--exclude names 'patient.nmae'"), "got {err}");
        assert!(err.contains("patient.mrn"), "lists the real leaves: {err}");

        // A typo in the PARENT too, which the old prefix check did catch.
        assert!(canonicalise_batch(&batch, &["paitent.mrn".to_string()]).is_err());
    }

    /// **`--exclude` must be applied BEFORE the column is mapped**, or the advice printed by a
    /// rejection is a dead end: `map_field` would reject the nested binary leaf before the exclude
    /// filter ever ran, so the very command the error suggested still failed.
    #[test]
    fn excluding_a_rejected_nested_leaf_works() {
        let inner = StructArray::from(vec![
            (
                Arc::new(Field::new("keep", DataType::Int32, false)),
                Arc::new(Int32Array::from(vec![1, 2])) as ArrayRef,
            ),
            (
                Arc::new(Field::new("blob", DataType::Binary, false)),
                Arc::new(BinaryArray::from(vec![b"a".as_ref(), b"b".as_ref()])) as ArrayRef,
            ),
        ]);
        let batch = record_batch(vec![("pos", Arc::new(inner) as ArrayRef)]).unwrap();

        // Without the exclude it is rejected, and the error names the dotted leaf.
        let err = canonicalise_batch(&batch, &[]).unwrap_err().to_string();
        assert!(
            err.contains("pos.blob"),
            "the error names the dotted leaf: {err}"
        );

        // …and following that advice actually works.
        let t = canonicalise_batch(&batch, &["pos.blob".to_string()]).unwrap();
        assert_eq!(
            t.columns
                .iter()
                .map(|(c, _)| c.name.as_str())
                .collect::<Vec<_>>(),
            vec!["pos.keep"]
        );
    }

    /// A top-level field whose NAME literally contains a dot must be excludable. Splitting on the dot
    /// before trying the whole name would look for a field called `a` and find none.
    #[test]
    fn a_top_level_field_named_with_a_dot_can_be_excluded() {
        let batch = record_batch(vec![
            ("a.b", Arc::new(Int32Array::from(vec![1, 2])) as ArrayRef),
            ("keep", Arc::new(Int32Array::from(vec![3, 4])) as ArrayRef),
        ])
        .unwrap();
        let t = canonicalise_batch(&batch, &["a.b".to_string()]).unwrap();
        assert_eq!(
            t.columns
                .iter()
                .map(|(c, _)| c.name.as_str())
                .collect::<Vec<_>>(),
            vec!["keep"]
        );
    }

    /// The escape hatch the rejection advertises has to actually work, or the advice is a dead end.
    #[test]
    fn exclude_makes_a_rejected_column_ingestable() {
        let batch = record_batch(vec![
            ("keep", Arc::new(Int32Array::from(vec![1, 2])) as ArrayRef),
            (
                "payload",
                Arc::new(BinaryArray::from(vec![b"a".as_ref(), b"b".as_ref()])) as ArrayRef,
            ),
        ])
        .unwrap();
        assert!(canonicalise_batch(&batch, &[]).is_err());
        let t = canonicalise_batch(&batch, &["payload".to_string()]).unwrap();
        assert_eq!(
            t.columns
                .iter()
                .map(|(c, _)| c.name.as_str())
                .collect::<Vec<_>>(),
            vec!["keep"]
        );
    }

    #[test]
    fn column_order_is_source_order() {
        // Order is part of identity (the Merkle root is over the encoded block), so a backend must
        // never sort or regroup.
        let batch = record_batch(vec![
            ("z", Arc::new(Int32Array::from(vec![1])) as ArrayRef),
            ("a", Arc::new(Int32Array::from(vec![2])) as ArrayRef),
            ("m", Arc::new(Int32Array::from(vec![3])) as ArrayRef),
        ])
        .unwrap();
        let t = canonicalise_batch(&batch, &[]).unwrap();
        assert_eq!(
            t.columns
                .iter()
                .map(|(c, _)| c.name.as_str())
                .collect::<Vec<_>>(),
            vec!["z", "a", "m"]
        );
    }

    #[test]
    fn an_arrow_extension_type_is_carried_on_its_storage_type_and_named() {
        let mut meta = std::collections::HashMap::new();
        meta.insert("ARROW:extension:name".to_string(), "arrow.uuid".to_string());
        let field = Field::new("id", DataType::Int64, false).with_metadata(meta);
        let schema = Arc::new(arrow_schema::Schema::new(vec![field]));
        let batch =
            RecordBatch::try_new(schema, vec![Arc::new(Int64Array::from(vec![7]))]).unwrap();
        let t = canonicalise_batch(&batch, &[]).unwrap();
        assert_eq!(t.columns[0].0.dtype, "i8");
        assert_eq!(
            t.columns[0].0.description.as_deref(),
            Some("[arrow-ext:arrow.uuid]"),
            "a reader must not be left guessing why this i8 means what it means"
        );
    }

    #[test]
    fn nan_payloads_are_canonicalised_through_the_arrow_lane_too() {
        let odd = f64::from_bits(0x7ff8_0000_dead_beef);
        let t = one("x", Arc::new(Float64Array::from(vec![1.0, odd]))).unwrap();
        let ColumnData::F64(v) = values_of(&t, 0) else {
            unreachable!()
        };
        assert_eq!(v[1].to_bits(), f64::NAN.to_bits());
        assert!(transforms(&t).contains(&transform::NAN_CANONICALISATION));
    }

    #[test]
    fn an_empty_batch_list_is_an_empty_table_not_an_error() {
        let t = canonicalise_batches(&[], &[]).unwrap();
        assert_eq!(t.rows(), 0);
        assert!(t.columns.is_empty());
    }

    /// **A null inside an encoded column's VALUES array must survive the decode.**
    ///
    /// Regression test for a real defect, and the failure mode is the worst kind: an encoded column whose
    /// *values* array contained a null decoded to a **present zero**, non-nullable, with no transform
    /// record — a corrupt product that verifies. Three lanes gather through one index map, and only the
    /// run-end one was consulting the values array's own validity.
    ///
    /// It also broke the dictionary lane's whole reason for existing (ADR-0056 §2): a
    /// `Dictionary<Int32, Utf8>` must produce the byte-identical column a plain `Utf8` would, or the
    /// producer's choice to dictionary-encode reaches the seal. With the bug, the plain column was
    /// *rejected* (a nullable `str` is unrepresentable, #457) while the dictionary-encoded one silently
    /// sealed wrong values — a divergence worse than an inconsistency.
    #[test]
    fn a_null_in_an_encoded_columns_values_array_is_not_decoded_as_a_present_zero() {
        // A dictionary whose VALUES contain a null, reached by a non-null key.
        let values = Int32Array::from(vec![Some(7), None, Some(9)]);
        let keys = Int32Array::from(vec![0, 1, 2, 1]);
        let dict: ArrayRef = Arc::new(
            arrow_array::DictionaryArray::<arrow_array::types::Int32Type>::try_new(
                keys,
                Arc::new(values),
            )
            .unwrap(),
        );
        let t = one("d", dict).unwrap();
        let ColumnData::Nullable { values, validity } = &t.columns[0].1 else {
            panic!(
                "a null value reached through a live key must produce a mask, got {:?}",
                t.columns[0].1
            )
        };
        assert_eq!(**values, ColumnData::I32(vec![7, 0, 9, 0]));
        assert_eq!(validity, &vec![true, false, true, false]);
        assert!(t.columns[0].0.nullable, "the sealed Column says so too");

        // The equivalence the dictionary lane claims: the same logical data, plainly encoded.
        let plain = one(
            "d",
            Arc::new(Int32Array::from(vec![Some(7), None, Some(9), None])),
        )
        .unwrap();
        assert_eq!(
            t.columns[0].1, plain.columns[0].1,
            "dictionary-encoded and plain must produce the identical column"
        );

        // A null *element* inside a fixed-size list is the same question one level down.
        let list_values = Int32Array::from(vec![Some(1), None, Some(3), Some(4)]);
        let fsl: ArrayRef = Arc::new(
            arrow_array::FixedSizeListArray::try_new(
                Arc::new(Field::new("item", DataType::Int32, true)),
                2,
                Arc::new(list_values),
                None,
            )
            .unwrap(),
        );
        let t = canonicalise_batch(&record_batch(vec![("pair", fsl)]).unwrap(), &[]).unwrap();
        // Row 0 is [1, null], row 1 is [3, 4] — so `pair.1` is nullable and `pair.0` is not.
        let by = |n: &str| {
            t.columns
                .iter()
                .find(|(c, _)| c.name == n)
                .map(|(_, d)| d.clone())
                .unwrap()
        };
        assert_eq!(by("pair.0"), ColumnData::I32(vec![1, 3]));
        let ColumnData::Nullable { values, validity } = by("pair.1") else {
            panic!(
                "the null list element must produce a mask, got {:?}",
                by("pair.1")
            )
        };
        assert_eq!(*values, ColumnData::I32(vec![0, 4]));
        assert_eq!(validity, vec![false, true]);
    }

    /// **Sliced arrays must map to their sliced values.**
    ///
    /// A `RecordBatch` does not always own its buffers from index 0: a reader that splits a row group,
    /// or any caller that used `RecordBatch::slice`, hands over arrays with a non-zero offset. Several
    /// arms here read `values()` — the raw buffer — and the `FixedSizeList` arm does its own
    /// `r * width + k` arithmetic, so "does this respect the offset?" is a question with a wrong answer
    /// available. It happens to be right (arrow-rs's `ScalarBuffer` carries the slice, and
    /// `FixedSizeListArray::slice` slices its child too), but *happens to be right* is exactly what a
    /// test is for: nothing in the type map's own code would stop a future arm from reading through an
    /// offset, and the failure mode is silently sealing the wrong rows.
    #[test]
    fn sliced_arrays_map_to_their_sliced_values_not_the_whole_buffer() {
        // Offset 2, length 3, out of 6 rows — so a bug that ignored the offset would read [10,11,12]
        // and a bug that ignored the length would read to the end.
        let mut fsl = FixedSizeListBuilder::new(Int32Builder::new(), 2);
        for row in [[1, 2], [3, 4], [5, 6], [7, 8], [9, 10], [11, 12]] {
            fsl.values().append_slice(&row);
            fsl.append(true);
        }
        let batch = record_batch(vec![
            (
                "prim",
                Arc::new(Int32Array::from(vec![10, 11, 12, 13, 14, 15])) as ArrayRef,
            ),
            (
                "nullable",
                Arc::new(Int32Array::from(vec![
                    Some(10),
                    Some(11),
                    None,
                    Some(13),
                    None,
                    Some(15),
                ])) as ArrayRef,
            ),
            (
                "text",
                Arc::new(StringArray::from(vec!["a", "b", "c", "d", "e", "f"])) as ArrayRef,
            ),
            (
                "flag",
                Arc::new(BooleanArray::from(vec![
                    true, false, true, true, false, false,
                ])) as ArrayRef,
            ),
            ("pair", Arc::new(fsl.finish()) as ArrayRef),
        ])
        .unwrap();

        let sliced = batch.slice(2, 3);
        assert_eq!(sliced.num_rows(), 3);
        let t = canonicalise_batch(&sliced, &[]).unwrap();
        assert_eq!(t.rows(), 3);

        let by = |name: &str| {
            t.columns
                .iter()
                .find(|(c, _)| c.name == name)
                .map(|(_, d)| d.clone())
                .unwrap_or_else(|| panic!("column '{name}' missing"))
        };
        assert_eq!(by("prim"), ColumnData::I32(vec![12, 13, 14]));
        assert_eq!(
            by("text"),
            ColumnData::Utf8(vec!["c".into(), "d".into(), "e".into()])
        );
        assert_eq!(by("flag"), ColumnData::Bool(vec![true, true, false]));
        // The validity mask has to be sliced in step with the values, or a null lands on the wrong row.
        let ColumnData::Nullable { values, validity } = by("nullable") else {
            panic!("expected a mask")
        };
        assert_eq!(*values, ColumnData::I32(vec![0, 13, 0]));
        assert_eq!(validity, vec![false, true, false]);
        // And the expanded tuple columns: rows 2..5 of the pairs are [5,6], [7,8], [9,10].
        assert_eq!(by("pair.0"), ColumnData::I32(vec![5, 7, 9]));
        assert_eq!(by("pair.1"), ColumnData::I32(vec![6, 8, 10]));
    }

    /// The same question for the two *encoded* lanes, whose `gather` walks a logical→physical index map.
    #[test]
    fn sliced_dictionary_and_run_end_arrays_decode_their_sliced_region() {
        let mut b = StringDictionaryBuilder::<arrow_array::types::Int32Type>::new();
        for v in ["red", "green", "blue", "green", "red", "blue"] {
            b.append_value(v);
        }
        let dict: ArrayRef = Arc::new(b.finish());
        let t = one_sliced("c", dict, 2, 3).unwrap();
        assert_eq!(
            values_of(&t, 0),
            ColumnData::Utf8(vec!["blue".into(), "green".into(), "red".into()])
        );

        // Runs: [7,7] [9,9,9] [11] — rows 2..5 span the end of run 0 and all of run 1.
        let rle: ArrayRef = Arc::new(
            RunArray::try_new(
                &Int32Array::from(vec![2, 5, 6]),
                &Int32Array::from(vec![7, 9, 11]),
            )
            .unwrap(),
        );
        let t = one_sliced("x", rle, 2, 3).unwrap();
        assert_eq!(values_of(&t, 0), ColumnData::I32(vec![9, 9, 9]));
    }

    /// Canonicalise one column after slicing it — the seam the two slice tests share.
    fn one_sliced(name: &str, a: ArrayRef, offset: usize, len: usize) -> Result<CanonicalTable> {
        let batch = record_batch(vec![(name, a)])?;
        canonicalise_batch(&batch.slice(offset, len), &[])
    }

    /// A struct nested in a struct flattens all the way down — the recursion is not one level deep.
    #[test]
    fn nested_structs_flatten_recursively() {
        let leaf = StructArray::from(vec![(
            Arc::new(Field::new("deep", DataType::Int32, false)),
            Arc::new(Int32Array::from(vec![5])) as ArrayRef,
        )]);
        let mid = StructArray::from(vec![(
            Arc::new(Field::new(
                "inner",
                DataType::Struct(Fields::from(vec![Field::new(
                    "deep",
                    DataType::Int32,
                    false,
                )])),
                false,
            )),
            Arc::new(leaf) as ArrayRef,
        )]);
        let t = one("outer", Arc::new(mid)).unwrap();
        assert_eq!(
            t.columns
                .iter()
                .map(|(c, _)| c.name.as_str())
                .collect::<Vec<_>>(),
            vec!["outer.inner.deep"]
        );
    }
}
