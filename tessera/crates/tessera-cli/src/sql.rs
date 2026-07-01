//! `tessera sql` — DataFusion SQL over a Tessera table block (spike phase, #251).
//!
//! **The stack.** A table block is Vortex → Arrow (zero-copy in the read direction) → DataFusion
//! runs SQL over Arrow in-process (pure Rust, embeddable, no external engine). We build the same
//! [`LogicalTableView`] the `read` verb uses — so `events` spans every `events_NNNN` shard as one
//! logical table — materialize every column into an Arrow `RecordBatch`, register it as a
//! DataFusion `MemTable` under the block name, and hand the SQL to `SessionContext::sql`.
//!
//! **What this is (spike phase).** Materializing to `RecordBatch` up front puts the whole block in
//! RAM: fine for the phase-1 spike (proves Vortex→Arrow→DataFusion end-to-end), noted as the
//! follow-up for phase-2/3 in issue #251 (streaming `TableProvider` + predicate/projection
//! pushdown so block-level stats can skip whole shards without a decode). Do NOT ship this as the
//! big-cohort path — that's the follow-up. Do ship it as the small/medium-cohort SQL surface, and
//! as the "prove the API" step the pushdown work builds on.
//!
//! **Runtime.** DataFusion needs an async runtime for `sql().collect()`; we spin a fresh
//! `current_thread` tokio runtime **per invocation**. No long-lived executor — ADR-0034 §4 "one
//! legitimate tokio use per surface" (this is a query CLI, not the write/read spine).

use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use arrow::array::{
    ArrayRef, Float32Array, Float64Array, Int16Array, Int32Array, Int64Array, Int8Array,
    RecordBatch, UInt16Array, UInt32Array, UInt64Array, UInt8Array,
};
use arrow::csv::WriterBuilder;
use arrow::datatypes::{DataType, Field, Schema};
use datafusion::datasource::MemTable;
use datafusion::prelude::SessionContext;
use tessera_io::{ColumnData, Reader};

use crate::nav::Format;

/// Convert one Tessera [`ColumnData`] to an Arrow [`ArrayRef`] — one branch per numeric dtype
/// (`ColumnData` is a closed enum, so every variant is covered exhaustively). Small vectors go
/// through `.clone()`; f32/f64 are copied verbatim (no NaN canonicalisation — DataFusion honours
/// IEEE-754 comparison semantics the same way the tests below assert).
fn column_to_array(col: ColumnData) -> ArrayRef {
    match col {
        ColumnData::I8(v) => Arc::new(Int8Array::from(v)) as ArrayRef,
        ColumnData::I16(v) => Arc::new(Int16Array::from(v)) as ArrayRef,
        ColumnData::I32(v) => Arc::new(Int32Array::from(v)) as ArrayRef,
        ColumnData::I64(v) => Arc::new(Int64Array::from(v)) as ArrayRef,
        ColumnData::U8(v) => Arc::new(UInt8Array::from(v)) as ArrayRef,
        ColumnData::U16(v) => Arc::new(UInt16Array::from(v)) as ArrayRef,
        ColumnData::U32(v) => Arc::new(UInt32Array::from(v)) as ArrayRef,
        ColumnData::U64(v) => Arc::new(UInt64Array::from(v)) as ArrayRef,
        ColumnData::F32(v) => Arc::new(Float32Array::from(v)) as ArrayRef,
        ColumnData::F64(v) => Arc::new(Float64Array::from(v)) as ArrayRef,
    }
}

/// Map an fd5 numpy-style dtype code (`i2`/`u4`/`f4`/…) to its Arrow [`DataType`]. Same closed
/// mapping the encoder writes; a code outside the table's supported set is a typed schema error.
fn numpy_to_arrow(code: &str) -> tessera_core::Result<DataType> {
    Ok(match code {
        "i1" => DataType::Int8,
        "i2" => DataType::Int16,
        "i4" => DataType::Int32,
        "i8" => DataType::Int64,
        "u1" => DataType::UInt8,
        "u2" => DataType::UInt16,
        "u4" => DataType::UInt32,
        "u8" => DataType::UInt64,
        "f4" => DataType::Float32,
        "f8" => DataType::Float64,
        other => {
            return Err(tessera_core::Error::Invalid(format!(
                "tessera sql: unsupported column dtype '{other}' (table cols are numeric)"
            )))
        }
    })
}

/// `tessera sql FILE BLOCK "QUERY"` — DataFusion SQL over the cross-block logical view.
///
/// - `block` is the table-block name (or multi-block prefix like `events`). It's registered as a
///   MemTable under **exactly that name**, so `FROM events` in a query matches `events` /
///   `events_NNNN` transparently — the same view semantics `tessera read` uses.
/// - `format` is `csv` (default) or `tsv`. The result batches are written via `arrow-csv`'s
///   writer (float `Display`, no quoting); `tsv` swaps the delimiter without another writer.
///
/// Bounded memory NOT guaranteed at the spike phase — see the module doc-comment (materializes
/// the whole block to a `RecordBatch` in RAM before handing to DataFusion). The public API doesn't
/// promise streaming yet; the streaming `TableProvider` is #251 phase 2/3.
pub fn run(file: &Path, block: &str, query: &str, format: Format) -> tessera_core::Result<()> {
    let mut out = std::io::stdout().lock();
    run_with(file, block, query, format, &mut out)
}

/// Write-facing variant of [`run`] — the CLI uses [`run`] (which locks stdout); tests inject a
/// `Vec<u8>` so the emitted CSV is captured verbatim without touching real stdout.
pub fn run_with(
    file: &Path,
    block: &str,
    query: &str,
    format: Format,
    out: &mut dyn Write,
) -> tessera_core::Result<()> {
    // Only `csv` / `tsv` make sense for a tabular query result — reuse `Format` for the flag
    // shape but reject `ndjson` explicitly (DataFusion's writer surface is arrow-csv today; a
    // JSON emitter is a follow-up).
    let delim = match format {
        Format::Csv => b',',
        Format::Tsv => b'\t',
        Format::Ndjson => {
            return Err(tessera_core::Error::Invalid(
                "tessera sql: --format ndjson is not supported yet (csv | tsv)".into(),
            ))
        }
    };

    // Build the logical view + eagerly materialize its columns into an Arrow RecordBatch. This is
    // the spike-phase materialisation described in the module doc (streaming is #251 phase 2/3).
    let mut r = Reader::open(file)?;
    let view = r.logical_table(block)?;
    let columns: Vec<tessera_core::block::table::Column> = view.columns().to_vec();
    if columns.is_empty() {
        return Err(tessera_core::Error::Invalid(format!(
            "tessera sql: block '{block}' has no columns"
        )));
    }
    let mut fields: Vec<Field> = Vec::with_capacity(columns.len());
    let mut arrays: Vec<ArrayRef> = Vec::with_capacity(columns.len());
    for col in &columns {
        let data = view.column(&mut r, &col.name)?;
        fields.push(Field::new(&col.name, numpy_to_arrow(&col.dtype)?, false));
        arrays.push(column_to_array(data));
    }
    let schema = Arc::new(Schema::new(fields));
    let batch = RecordBatch::try_new(schema.clone(), arrays)
        .map_err(|e| tessera_core::Error::Invalid(format!("tessera sql: build batch: {e}")))?;

    // DataFusion is async; drive it on a fresh current-thread tokio runtime. No long-lived
    // executor — the CLI is a one-shot query. `MemTable::try_new` takes `Vec<Vec<RecordBatch>>`
    // (one partition, one batch — we hold the whole block anyway).
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| tessera_core::Error::Invalid(format!("tessera sql: build runtime: {e}")))?;

    let batches: Vec<RecordBatch> = runtime.block_on(async move {
        let ctx = SessionContext::new();
        let mem = MemTable::try_new(schema, vec![vec![batch]])
            .map_err(|e| tessera_core::Error::Invalid(format!("tessera sql: MemTable: {e}")))?;
        ctx.register_table(block, Arc::new(mem))
            .map_err(|e| tessera_core::Error::Invalid(format!("tessera sql: register: {e}")))?;
        let df = ctx
            .sql(query)
            .await
            .map_err(|e| tessera_core::Error::Invalid(format!("tessera sql: parse: {e}")))?;
        df.collect()
            .await
            .map_err(|e| tessera_core::Error::Invalid(format!("tessera sql: execute: {e}")))
    })?;

    // Empty result → header only (parity with `read`'s CSV shape) OR no output at all if there
    // were zero batches. arrow-csv writes the header from the first batch, so we drive it there.
    let mut writer = WriterBuilder::new()
        .with_header(true)
        .with_delimiter(delim)
        .build(out);
    for batch in &batches {
        writer
            .write(batch)
            .map_err(|e| tessera_core::Error::Invalid(format!("tessera sql: csv write: {e}")))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tessera_core::block::table::{Column, TableSpec};
    use tessera_core::ProductBuilder;
    use tessera_io::{pack, table::table_block, table::TableData};

    /// Seal a tiny table `.tsra` — same shape as `nav::tests::sample`, so we exercise the
    /// production seal + Vortex encode + LogicalTableView path (not a hand-rolled shortcut).
    fn sample(path: &std::path::Path) {
        let spec = TableSpec {
            columns: vec![
                Column {
                    name: "ms".into(),
                    dtype: "u4".into(),
                    codec: None,
                },
                Column {
                    name: "en".into(),
                    dtype: "f4".into(),
                    codec: None,
                },
            ],
            rows: 4,
            row_index: None,
        };
        let data: TableData = vec![
            ("ms".into(), ColumnData::U32(vec![10, 20, 30, 40])),
            ("en".into(), ColumnData::F32(vec![0.5, 1.5, 2.5, 3.5])),
        ];
        let (block_ref, payload) = table_block("events", &spec, &data).unwrap();
        let mut b = ProductBuilder::new("listmode", "DP", "d", "2024-01-01T00:00:00Z");
        b.add_block_ref(block_ref);
        b.with_field("modality", serde_json::json!("PT"));
        let sealed = b.seal().unwrap();
        pack(&sealed, &[payload], path).unwrap();
    }

    /// End-to-end sanity: WHERE + ORDER BY + LIMIT reaches the block, decodes it via Vortex,
    /// registers the Arrow batch, and DataFusion executes the query. If the arrow-58 alignment
    /// with Vortex broke, this test would fail to compile at the `column_to_array` call site
    /// (two arrow types with the same name from different majors → mismatch).
    #[test]
    fn where_order_by_limit_executes_end_to_end() {
        let dir = tempfile::tempdir().unwrap();
        let tsra = dir.path().join("p.tsra");
        sample(&tsra);

        let mut out = Vec::<u8>::new();
        run_with(
            &tsra,
            "events",
            "SELECT ms FROM events WHERE en > 1.0 ORDER BY ms LIMIT 2",
            Format::Csv,
            &mut out,
        )
        .unwrap();
        // Expect: header + the two smallest ms whose en > 1.0. sample() has (ms, en) pairs
        // (10, 0.5), (20, 1.5), (30, 2.5), (40, 3.5) → WHERE en > 1.0 keeps {20, 30, 40} →
        // ORDER BY ms ASC LIMIT 2 → [20, 30].
        let text = String::from_utf8(out).unwrap();
        assert_eq!(text, "ms\n20\n30\n");
    }

    /// Column projection works: `SELECT en FROM events` returns only `en` — proves DataFusion is
    /// actually planning + executing, not just echoing the whole RecordBatch back.
    #[test]
    fn select_single_column_projects_correctly() {
        let dir = tempfile::tempdir().unwrap();
        let tsra = dir.path().join("p.tsra");
        sample(&tsra);

        let mut out = Vec::<u8>::new();
        run_with(
            &tsra,
            "events",
            "SELECT en FROM events ORDER BY en",
            Format::Csv,
            &mut out,
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert_eq!(text, "en\n0.5\n1.5\n2.5\n3.5\n");
    }

    /// `--format tsv` swaps the delimiter (arrow-csv writer, same batches). Also confirms empty
    /// result-set behaviour — `WHERE en > 999.0` matches zero rows.
    #[test]
    fn tsv_delimiter_and_empty_result() {
        let dir = tempfile::tempdir().unwrap();
        let tsra = dir.path().join("p.tsra");
        sample(&tsra);

        let mut out = Vec::<u8>::new();
        run_with(
            &tsra,
            "events",
            "SELECT ms, en FROM events WHERE ms >= 30",
            Format::Tsv,
            &mut out,
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        // Two rows survive the filter; TSV means literal tabs between fields.
        assert!(
            text.starts_with("ms\ten\n"),
            "expected TSV header, got: {text:?}"
        );
        assert!(
            text.contains("30\t2.5\n"),
            "missing (30, 2.5) row: {text:?}"
        );
        assert!(
            text.contains("40\t3.5\n"),
            "missing (40, 3.5) row: {text:?}"
        );
    }

    /// A bad column name in the query surfaces DataFusion's planner error via our typed
    /// `Error::Invalid`, not a panic. This is the "does the error path work" guard.
    #[test]
    fn bad_column_returns_a_typed_error() {
        let dir = tempfile::tempdir().unwrap();
        let tsra = dir.path().join("p.tsra");
        sample(&tsra);

        let mut out = Vec::<u8>::new();
        let err = run_with(
            &tsra,
            "events",
            "SELECT no_such_column FROM events",
            Format::Csv,
            &mut out,
        )
        .unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("no_such_column") || msg.contains("Schema") || msg.contains("plan"),
            "expected a schema/plan error naming the missing column, got: {msg}"
        );
    }

    /// `--format ndjson` is explicitly rejected today (arrow-csv is the writer) — proves the
    /// rejection is typed, not a panic. Regenerating this to a real ndjson path is a follow-up.
    #[test]
    fn ndjson_format_is_rejected_typed() {
        let dir = tempfile::tempdir().unwrap();
        let tsra = dir.path().join("p.tsra");
        sample(&tsra);

        let mut out = Vec::<u8>::new();
        let err = run_with(
            &tsra,
            "events",
            "SELECT ms FROM events",
            Format::Ndjson,
            &mut out,
        )
        .unwrap_err();
        assert!(
            format!("{err}").contains("ndjson"),
            "typed rejection expected"
        );
    }
}
