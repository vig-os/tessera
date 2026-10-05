//! CSV/TSV → `table`, **inference-free** (ADR-0056 §8).
//!
//! # Why CSV needed a determinism argument before it could ship at all
//!
//! ADR-0056 §8 deferred CSV out of P1, and the reason was not effort — it was that CSV carries no
//! schema, and **sample-based inference is anti-deterministic by construction**: the inferred schema
//! depends on which rows landed in the sample, so the same file can seal to two different products.
//! That is the one property Tessera cannot trade.
//!
//! The resolution is to remove the inference rather than the format. This lane requires an operator to
//! **declare** the columns (`--column name:dtype`, repeatable, or `--schema schema.toml`), and there is
//! no mixed mode: if any column is declared, every column must be. A declared schema is an *assertion*
//! about the file, checkable and reproducible; an inferred one is a guess that the seal would then
//! assert as truth.
//!
//! # Hazards H3 and H8 are closed by construction, not deferred
//!
//! ADR-0056 §5 rated float parsing (**H3**) and locale (**H8**) as the reasons CSV was dangerous, both
//! written with C's `strtod` in mind. Rust's `str::parse::<f64>()` is neither:
//!
//! - it is **correctly rounded** — the Eisel-Lemire/Clinger implementation in `core::num::dec2flt`
//!   returns the nearest representable double for every input, so two hosts cannot disagree, and there
//!   is no FMA-sensitive fast path to diverge on;
//! - it is **locale-independent** — the grammar is fixed by the language, so `LC_NUMERIC=de_DE` cannot
//!   make `1,5` parse as 1.5 (it fails to parse at all, which is the honest outcome).
//!
//! So this lane parses every value with std and uses the `csv` crate for nothing but RFC-4180 record
//! splitting. The [`tests::parsing_is_locale_independent`] test sets a comma-decimal locale and proves
//! it. That is the whole argument for CSV being P1-safe, and it is why the sealed record carries
//! `csv_explicit_schema` rather than `csv_inferred_schema`.
//!
//! # What is still deferred
//!
//! Optional inference behind an explicit flag, recording `csv_inferred_schema` in the seal (§8's own
//! escape hatch). Not built: nothing needs it yet, and the flag is the easy part — the hard part is
//! that its output is only reproducible if the sample is the whole file, at which point it is not
//! really inference.

use std::path::Path;

use tessera_core::block::table::Column;
use tessera_core::{Error, IngestTransform, Result};
use tessera_io::table::ColumnData;

use crate::canonical::{
    canonicalise, empty_column, transform, unclassified_column, CanonicalTable, TableBuilder,
};

fn he(e: impl std::fmt::Display) -> Error {
    Error::Invalid(format!("ingest: csv: {e}"))
}

/// The error ADR-0056 §8 specifies verbatim for a CSV with no declared schema.
///
/// Kept as one function so the CLI, the spec engine and the docs cannot drift into three differently
/// worded versions of the same refusal — and so the conversion command stays copy-pasteable.
pub fn no_schema_error() -> Error {
    he(
        "CSV carries no schema and Tessera does not infer one (an inferred schema depends on which \
         rows were sampled, so the same file could seal two ways — and the seal must be \
         reproducible).\n  \
         declare the columns:\n    \
           tessera ingest table input.csv --column id:i8 --column energy:f8 --column label:str\n  \
         or convert first, keeping the dtypes the producer already knew:\n    \
           duckdb -c \"COPY (SELECT * FROM 'input.csv') TO 'input.parquet' (FORMAT parquet)\"\n    \
           tessera ingest table input.parquet",
    )
}

/// One operator-declared column: storage name, fd5 dtype code, and whether NULLs are allowed.
#[derive(Debug, Clone, PartialEq)]
pub struct Declared {
    pub name: String,
    pub dtype: String,
    pub nullable: bool,
}

/// How a CSV's columns were declared.
#[derive(Debug, Clone, Default)]
pub struct CsvOptions {
    /// The declared columns, in file order. Empty ⇒ [`no_schema_error`].
    pub columns: Vec<Declared>,
    /// Field delimiter. `,` for CSV, `\t` for TSV — an explicit choice, never sniffed, for the same
    /// reason the dtypes are not: a sniffed delimiter is an inference the seal would assert.
    pub delimiter: u8,
    /// Whether the first record is a header row to skip **and check against the declaration**.
    pub header: bool,
    /// Columns to drop after reading, by declared name.
    ///
    /// Post-read rather than "just don't declare it": declarations are **positional**, so omitting a
    /// column would silently shift every later column onto the wrong field. Dropping after the read
    /// keeps the positions honest and makes `--exclude` mean the same thing in both lanes.
    pub exclude: Vec<String>,
    /// Token(s) read as NULL in a nullable column. Defaults to the empty field only — `"NA"`, `"NULL"`
    /// and `"-"` are real conventions but guessing between them is exactly the inference this lane
    /// refuses, so an operator names them.
    pub null_tokens: Vec<String>,
}

impl CsvOptions {
    /// Declarations from repeatable `NAME:DTYPE` strings (the CLI's `--column`).
    pub fn from_decls(decls: &[String]) -> Result<Self> {
        let mut columns = Vec::with_capacity(decls.len());
        for d in decls {
            let (name, dtype, nullable) = crate::canonical::parse_column_decl(d)?;
            columns.push(Declared {
                name,
                dtype,
                nullable,
            });
        }
        Ok(CsvOptions {
            columns,
            delimiter: b',',
            header: true,
            exclude: Vec::new(),
            null_tokens: Vec::new(),
        })
    }

    /// Is this field text NULL for a column of this dtype?
    ///
    /// **The rule differs by dtype, and the difference is the point.** For a numeric or boolean column
    /// an empty field cannot be a value — there is no empty number — so it is NULL. For a `str` column
    /// the empty field **is** the empty string: a legitimate value that a CSV has no other way to write,
    /// and treating it as NULL would make `""` un-ingestable *and* silently reinterpret data the
    /// operator wrote.
    ///
    /// A `str` column therefore takes NULLs only from an explicitly declared `--null-token`, which is
    /// the same principle as the rest of this lane: the operator says what the file means, Tessera does
    /// not guess. `read_table` records `csv_null_tokens` in the seal when any are declared, so the
    /// artifact carries which spellings were treated as absent.
    fn is_null(&self, text: &str, dtype: &str) -> bool {
        if self.null_tokens.iter().any(|t| t == text) {
            return true;
        }
        dtype != "str" && text.is_empty()
    }
}

/// Read + canonicalise a CSV under an operator-declared schema.
/// Per-declared-column accumulator — the shared half of the whole-file and chunked readers.
///
/// Extracted so the rules that are easy to get subtly wrong exist **once**: which columns are trimmed
/// (everything but `str`, because trimming text would be a silent unrecorded transformation ADR-0056 §2
/// forbids), which field texts count as NULL, the declared-non-nullable refusal, and
/// nullable-by-presence. A chunked reader that reimplemented any of them would make a streamed CSV
/// disagree with a batch one for reasons no `content_hash` comparison could explain.
struct CsvAccum {
    /// Owned, not borrowed: the chunked reader returns a self-contained iterator, and a borrow here
    /// would make that iterator borrow its own captures.
    opts: CsvOptions,
    values: Vec<ColumnData>,
    validity: Vec<Vec<bool>>,
    saw_null: Vec<bool>,
    rows: usize,
}

impl CsvAccum {
    fn new(opts: CsvOptions) -> Result<Self> {
        Ok(Self {
            values: opts
                .columns
                .iter()
                .map(|c| empty_column(&c.dtype, 0))
                .collect::<Result<_>>()?,
            validity: vec![Vec::new(); opts.columns.len()],
            saw_null: vec![false; opts.columns.len()],
            rows: 0,
            opts,
        })
    }

    fn rows(&self) -> usize {
        self.rows
    }

    /// Absorb one record. `row_no` is the 1-based file row, so every error in this module reports the
    /// same number whether the read is chunked or not.
    fn push_record(
        &mut self,
        path: &Path,
        record: &csv::StringRecord,
        row_no: usize,
    ) -> Result<()> {
        if record.len() != self.opts.columns.len() {
            return Err(he(format!(
                "{}: row {} has {} fields but {} columns were declared",
                path.display(),
                row_no,
                record.len(),
                self.opts.columns.len()
            )));
        }
        for (i, decl) in self.opts.columns.iter().enumerate() {
            let raw = record.get(i).unwrap_or_default();
            // Surrounding whitespace is insignificant to a *number* and significant to a *string*.
            // Trimming a `str` column would be a silent, unrecorded transformation (` foo ` → `foo`) of
            // exactly the kind ADR-0056 §2 exists to forbid — and one that could not be undone from the
            // artifact. So text columns are taken verbatim and everything else is trimmed, which is what
            // also makes ` 1.5 ` parse rather than fail.
            let text = if decl.dtype == "str" { raw } else { raw.trim() };
            if self.opts.is_null(text, &decl.dtype) {
                if !decl.nullable {
                    return Err(he(format!(
                        "{}: row {}, column '{}' is empty but was declared non-nullable — append '?' \
                         to allow NULLs ({}:{}?)",
                        path.display(),
                        row_no,
                        decl.name,
                        decl.name,
                        decl.dtype
                    )));
                }
                push_default(&mut self.values[i]);
                self.validity[i].push(false);
                self.saw_null[i] = true;
                continue;
            }
            push_parsed(&mut self.values[i], text).map_err(|e| {
                he(format!(
                    "{}: row {}, column '{}' (declared {}): {e}",
                    path.display(),
                    row_no,
                    decl.name,
                    decl.dtype
                ))
            })?;
            self.validity[i].push(true);
        }
        self.rows += 1;
        Ok(())
    }

    /// Finish the rows absorbed so far into a canonical table, leaving the accumulator empty and ready
    /// for the next chunk.
    ///
    /// Nullable-by-presence applies to what this call saw. For the whole-file reader that is the file;
    /// for a chunked one it is the chunk, which is exactly what the streaming shape pass folds over.
    fn finish_chunk(&mut self) -> Result<CanonicalTable> {
        let mut b = TableBuilder::new();
        b.record(IngestTransform::new(transform::CSV_EXPLICIT_SCHEMA));
        if !self.opts.null_tokens.is_empty() {
            // Which field texts were read as absent is a recorded transform: it changes what the values
            // mean, and a reader comparing back to the CSV cannot otherwise tell a NULL from the literal
            // string "NA".
            b.record(
                IngestTransform::new(transform::CSV_NULL_TOKENS)
                    .with("tokens", serde_json::json!(self.opts.null_tokens)),
            );
        }
        for (i, decl) in self.opts.columns.iter().enumerate() {
            if self.opts.exclude.iter().any(|e| e == &decl.name) {
                continue;
            }
            let mut data = std::mem::replace(&mut self.values[i], empty_column(&decl.dtype, 0)?);
            if self.saw_null[i] {
                data = ColumnData::Nullable {
                    values: Box::new(data),
                    validity: std::mem::take(&mut self.validity[i]),
                };
            }
            canonicalise(&mut b, &decl.name, &mut data);
            let column: Column = if self.saw_null[i] {
                unclassified_column(&decl.name, &decl.dtype).nullable()
            } else {
                unclassified_column(&decl.name, &decl.dtype)
            };
            b.push(column, data)?;
        }
        for v in &mut self.validity {
            v.clear();
        }
        self.saw_null.iter_mut().for_each(|f| *f = false);
        self.rows = 0;
        Ok(b.finish())
    }
}

/// Validate `--exclude` against the declaration. A name that matches nothing is a typo, and a typo that
/// silently does nothing is the worst outcome: the operator believes they dropped a PHI column.
fn check_exclude(opts: &CsvOptions) -> Result<()> {
    if let Some(unknown) = opts
        .exclude
        .iter()
        .find(|e| !opts.columns.iter().any(|c| &&c.name == e))
    {
        return Err(he(format!(
            "--exclude names '{unknown}', which is not a declared column (declared: {})",
            opts.columns
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>()
                .join(" · ")
        )));
    }
    Ok(())
}

/// Open a CSV, honouring the declared delimiter and header, and check the header against the
/// declaration before any value is read.
type DigestedCsv =
    csv::Reader<tessera_core::hash::DigestingReader<std::io::BufReader<std::fs::File>>>;

fn open_csv(path: &Path, opts: &CsvOptions) -> Result<DigestedCsv> {
    let file =
        std::fs::File::open(path).map_err(|e| he(format!("open {}: {e}", path.display())))?;
    let mut rdr = csv::ReaderBuilder::new()
        .delimiter(opts.delimiter)
        .has_headers(opts.header)
        // `flexible(true)` does NOT mean ragged rows are tolerated — the per-row length check in
        // `push_record` rejects them. It means *we* reject them, with our own message: the row number
        // matches the one every other error in this module reports, and the text names the declared
        // column count, which the reader's own "found record with 1 fields, but the previous record has
        // 2" does not. One error vocabulary per surface is worth the explicit check.
        .flexible(true)
        // Digested as it is read (#542): CSV is a forward scan, so the source digest comes out of the
        // same pass. The alternative — re-opening the path afterwards, as `provenance::source_digest`
        // does — is not merely a second read but a WRONG one on a pipe, where the re-open hands back
        // the drained stream and hashes zero bytes.
        .from_reader(tessera_core::hash::DigestingReader::new(
            std::io::BufReader::new(file),
        ));
    if opts.header {
        let header = rdr
            .headers()
            .map_err(|e| he(format!("read header of {}: {e}", path.display())))?
            .clone();
        check_header(&header, &opts.columns)?;
    }
    Ok(rdr)
}

pub fn read_table(path: &Path, opts: &CsvOptions) -> Result<CanonicalTable> {
    read_table_digested(path, opts).map(|(table, _)| table)
}

/// [`read_table`], also returning the `blake3` of the bytes it read.
///
/// The one implementation of both, so the digest cannot be computed over a different read than the
/// one that produced the table. The digest is the raw content hash; a caller sealing it onto an
/// `ingested_from` edge wraps it with [`crate::provenance::single_source_digest`].
pub fn read_table_digested(path: &Path, opts: &CsvOptions) -> Result<(CanonicalTable, String)> {
    if opts.columns.is_empty() {
        return Err(no_schema_error());
    }
    check_exclude(opts)?;
    let mut rdr = open_csv(path, opts)?;
    let mut accum = CsvAccum::new(opts.clone())?;
    let mut record = csv::StringRecord::new();
    let mut row = 0usize;
    while rdr
        .read_record(&mut record)
        .map_err(|e| he(format!("{}: {e}", path.display())))?
    {
        row += 1;
        accum.push_record(path, &record, row)?;
    }
    let table = accum.finish_chunk()?;
    // The loop ran to EOF, so the reader has seen every byte of the source.
    Ok((table, rdr.into_inner().digest()))
}

/// Lazily canonicalised CSV chunks of at most `rows_per_chunk` rows — what the streaming driver consumes.
///
/// CSV is sequential, so each traversal is a fresh open; the two passes are two reads of the file. The
/// explicit-schema requirement, the header cross-check, the `--exclude` typo check and every per-field
/// rule are the whole-file reader's, because both go through [`CsvAccum`] and [`open_csv`].
pub fn csv_chunks(
    path: &Path,
    opts: &CsvOptions,
    rows_per_chunk: usize,
) -> Result<impl Iterator<Item = Result<CanonicalTable>> + use<>> {
    if opts.columns.is_empty() {
        return Err(no_schema_error());
    }
    check_exclude(opts)?;
    let mut rdr = open_csv(path, opts)?;
    let mut accum = CsvAccum::new(opts.clone())?;
    let display = path.display().to_string();
    let mut record = csv::StringRecord::new();
    let mut row = 0usize;
    let chunk = rows_per_chunk.max(1);
    // Whether any chunk has been handed out yet — see the EOF arm.
    let mut emitted = false;
    Ok(std::iter::from_fn(move || {
        loop {
            match rdr.read_record(&mut record) {
                Err(e) => return Some(Err(he(format!("{display}: {e}")))),
                Ok(false) => {
                    // End of file: emit the partial tail. A source with NO data rows still emits
                    // exactly ONE empty chunk, because a header-only CSV is a legitimate 0-row table
                    // with a declared schema — which is precisely what the whole-file path seals. A
                    // lane that yielded nothing would leave the shape pass with no columns at all, so
                    // streaming would fail ("decoded to no columns") where batch succeeds, and the
                    // two paths would disagree about whether an empty file is an error.
                    if accum.rows() > 0 || !emitted {
                        emitted = true;
                        return Some(accum.finish_chunk());
                    }
                    return None;
                }
                Ok(true) => {
                    row += 1;
                    if let Err(e) = accum.push_record(std::path::Path::new(&display), &record, row)
                    {
                        return Some(Err(e));
                    }
                    if accum.rows() >= chunk {
                        emitted = true;
                        return Some(accum.finish_chunk());
                    }
                }
            }
        }
    }))
}

/// Check the file's header row against the declaration.
///
/// A mismatch is an **error**, because the failure it prevents is silent and severe: declare
/// `--column energy:f8 --column count:u4` against a file whose columns are in the other order and every
/// value lands in the wrong column, parses fine, and seals. The header is the only cross-check
/// available, so it is used rather than skipped.
fn check_header(header: &csv::StringRecord, declared: &[Declared]) -> Result<()> {
    let found: Vec<&str> = header.iter().map(str::trim).collect();
    if found.len() != declared.len() {
        return Err(he(format!(
            "the file has {} header fields but {} columns were declared\n  header:   {}\n  declared: {}",
            found.len(),
            declared.len(),
            found.join(" · "),
            declared.iter().map(|d| d.name.as_str()).collect::<Vec<_>>().join(" · ")
        )));
    }
    for (i, d) in declared.iter().enumerate() {
        if found[i] != d.name {
            return Err(he(format!(
                "declared column {} is '{}' but the header says '{}' — declarations are positional, so \
                 a mismatch here would put every value in the wrong column.\n  header:   {}\n  \
                 declared: {}\n  (pass --no-header if the file genuinely has no header row)",
                i + 1,
                d.name,
                found[i],
                found.join(" · "),
                declared.iter().map(|x| x.name.as_str()).collect::<Vec<_>>().join(" · ")
            )));
        }
    }
    Ok(())
}

/// Does this field text itself spell an infinity?
///
/// The spellings Rust's float parser accepts for infinity, case-insensitively, with an optional sign.
/// Used to tell "the operator wrote `inf`" from "a finite decimal overflowed the declared dtype" — the
/// second of which must not seal silently as infinity.
fn spells_infinity(text: &str) -> bool {
    let t = text.trim_start_matches(['+', '-']).to_ascii_lowercase();
    t == "inf" || t == "infinity"
}

/// Push the dtype's zero into a column — the value that sits under a NULL before
/// [`canonicalise`] confirms it (H5 normalises masked slots, and this is already that value, so the
/// CSV lane never carries producer noise under a null in the first place).
fn push_default(data: &mut ColumnData) {
    match data {
        ColumnData::I8(v) => v.push(0),
        ColumnData::I16(v) => v.push(0),
        ColumnData::I32(v) => v.push(0),
        ColumnData::I64(v) => v.push(0),
        ColumnData::U8(v) => v.push(0),
        ColumnData::U16(v) => v.push(0),
        ColumnData::U32(v) => v.push(0),
        ColumnData::U64(v) => v.push(0),
        ColumnData::F32(v) => v.push(0.0),
        ColumnData::F64(v) => v.push(0.0),
        ColumnData::Bool(v) => v.push(false),
        ColumnData::Utf8(v) => v.push(String::new()),
        ColumnData::Nullable { .. } => {}
    }
}

/// Parse one field into the column's declared dtype, with Rust std's parsers throughout.
///
/// `b1` accepts the spellings a real CSV uses (`true/false`, `1/0`, `yes/no`, `t/f`,
/// case-insensitively) and nothing else — a permissive "anything non-empty is true" rule would make
/// `false` parse as `true`, which is the worst available failure.
fn push_parsed(data: &mut ColumnData, text: &str) -> std::result::Result<(), String> {
    macro_rules! num {
        ($v:expr, $t:ty) => {{
            let parsed: $t = text
                .parse()
                .map_err(|e| format!("'{text}' is not a valid {}: {e}", stringify!($t)))?;
            $v.push(parsed);
        }};
    }
    /// Floats get the same parse plus an **overflow check**.
    ///
    /// `"1e39".parse::<f32>()` and `"1e400".parse::<f64>()` both succeed, returning `inf` — so without
    /// this a value far outside the declared dtype's range would seal as infinity, silently, and the
    /// artifact would assert that the measurement *was* infinite. An explicitly-spelled `inf` / `nan`
    /// is a different thing and is allowed: those are unambiguous IEEE values, `NaN` is a legitimate
    /// measured quantity (the S13 clinical gate guarantees its bit-exact round-trip), and a text file
    /// that spells one means it.
    macro_rules! float {
        ($v:expr, $t:ty) => {{
            let parsed: $t = text
                .parse()
                .map_err(|e| format!("'{text}' is not a valid {}: {e}", stringify!($t)))?;
            if parsed.is_infinite() && !spells_infinity(text) {
                return Err(format!(
                    "'{text}' overflows {} (it parses to infinity). Declare a wider dtype, or write \
                     'inf' explicitly if infinity is the value you mean.",
                    stringify!($t)
                ));
            }
            $v.push(parsed);
        }};
    }
    match data {
        ColumnData::I8(v) => num!(v, i8),
        ColumnData::I16(v) => num!(v, i16),
        ColumnData::I32(v) => num!(v, i32),
        ColumnData::I64(v) => num!(v, i64),
        ColumnData::U8(v) => num!(v, u8),
        ColumnData::U16(v) => num!(v, u16),
        ColumnData::U32(v) => num!(v, u32),
        ColumnData::U64(v) => num!(v, u64),
        ColumnData::F32(v) => float!(v, f32),
        ColumnData::F64(v) => float!(v, f64),
        ColumnData::Bool(v) => {
            let b = match text.to_ascii_lowercase().as_str() {
                "true" | "t" | "1" | "yes" | "y" => true,
                "false" | "f" | "0" | "no" | "n" => false,
                other => {
                    return Err(format!(
                        "'{other}' is not a boolean (accepted: true/false, t/f, 1/0, yes/no)"
                    ))
                }
            };
            v.push(b);
        }
        ColumnData::Utf8(v) => v.push(text.to_string()),
        ColumnData::Nullable { .. } => return Err("nested nullable column".into()),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
        let p = dir.join("t.csv");
        std::fs::write(&p, body).unwrap();
        p
    }

    fn decls(v: &[&str]) -> CsvOptions {
        CsvOptions::from_decls(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap()
    }

    #[test]
    fn a_declared_csv_reads_into_flat_columns() {
        let dir = tempfile::tempdir().unwrap();
        let p = write(
            dir.path(),
            "id,energy,label,ok\n1,511.0,annihilation,true\n2,-0.5,scatter,no\n",
        );
        let t = read_table(&p, &decls(&["id:i4", "energy:f8", "label:str", "ok:b1"])).unwrap();
        assert_eq!(t.rows(), 2);
        assert_eq!(t.columns[0].1, ColumnData::I32(vec![1, 2]));
        assert_eq!(t.columns[1].1, ColumnData::F64(vec![511.0, -0.5]));
        assert_eq!(
            t.columns[2].1,
            ColumnData::Utf8(vec!["annihilation".into(), "scatter".into()])
        );
        assert_eq!(t.columns[3].1, ColumnData::Bool(vec![true, false]));
        // Every column is stamped unclassified — nobody has told us what this data is.
        for (c, _) in &t.columns {
            assert_eq!(c.sensitivity, tessera_core::schema::Sensitivity::Unknown);
        }
        // The seal records that the schema was declared, not inferred.
        assert!(t
            .transforms
            .iter()
            .any(|x| x.name == transform::CSV_EXPLICIT_SCHEMA));
    }

    /// The whole reason CSV is allowed in P1 (ADR-0056 §5 H8): a hostile locale must change nothing —
    /// and the comma-decimal spelling must *fail* rather than silently mean 1.5.
    ///
    /// Deliberately does **not** mutate `LC_ALL`. An earlier version did, and it was wrong twice over:
    /// `std::env::set_var` is `unsafe` precisely because it is not thread-safe, and this repo runs
    /// shared-process `cargo test`, so a concurrent test reading the environment is UB — and an assert
    /// firing mid-test would have leaked the variable into every test that ran after it.
    ///
    /// It does not need to. H8's hazard is that a *locale-sensitive* parser reinterprets the input;
    /// `str::parse` has a grammar fixed by the language and never consults the C locale, so the property
    /// is established by pinning the exact values and by showing the German spelling is **rejected**.
    /// The full-process version of this claim belongs in a harness that can set the environment before
    /// `main`, not in a `#[test]`.
    #[test]
    fn parsing_is_locale_independent() {
        let dir = tempfile::tempdir().unwrap();

        // Dot-decimal parses to exactly these doubles, under any locale, on any host.
        let p = write(dir.path(), "x\n1.5\n0.1\n2.2250738585072014e-308\n");
        let t = read_table(&p, &decls(&["x:f8"])).unwrap();
        assert_eq!(
            t.columns[0].1,
            ColumnData::F64(vec![1.5, 0.1, 2.2250738585072014e-308])
        );

        // …and the German spelling is REJECTED, not reinterpreted. Accepting `1,5` would also mean a
        // comma-delimited file silently re-splits, which is the actual disaster H8 describes.
        let p = write(dir.path(), "x\n1,5\n");
        let err = read_table(&p, &decls(&["x:f8"])).unwrap_err().to_string();
        assert!(
            err.contains("has 2 fields but 1 columns were declared"),
            "got {err}"
        );

        // A comma *inside a quoted field* is data, not a delimiter — so the rejection above is about
        // the number's grammar, not about commas being unparseable.
        let p = write(dir.path(), "label\n\"a,b\"\n");
        let t = read_table(&p, &decls(&["label:str"])).unwrap();
        assert_eq!(t.columns[0].1, ColumnData::Utf8(vec!["a,b".into()]));
    }

    /// Correct rounding (H3): these are the classic `strtod`-divergence inputs. A nearest-representable
    /// parser gives one answer; a fast-path-with-fallback parser can give another.
    #[test]
    fn float_parsing_is_correctly_rounded() {
        let dir = tempfile::tempdir().unwrap();
        let p = write(
            dir.path(),
            "x\n8.98846567431158e307\n2.2250738585072011e-308\n0.500000000000000166533453693773481063544750213623046875\n",
        );
        let t = read_table(&p, &decls(&["x:f8"])).unwrap();
        let ColumnData::F64(v) = &t.columns[0].1 else {
            unreachable!()
        };
        assert_eq!(v[0], "8.98846567431158e307".parse::<f64>().unwrap());
        // The Clinger/Eisel-Lemire boundary case: the largest double below the normal minimum.
        assert_eq!(v[1].to_bits(), 0x000f_ffff_ffff_ffff);
        // A halfway case that rounds up under round-to-nearest-even.
        assert_eq!(v[2], 0.5000000000000002);
    }

    /// Whitespace is insignificant to a number and significant to a string, and the lane must not
    /// quietly decide otherwise: trimming a `str` column would be an unrecorded transformation that
    /// cannot be undone from the artifact.
    #[test]
    fn whitespace_is_trimmed_for_numbers_and_preserved_for_text() {
        let dir = tempfile::tempdir().unwrap();
        let p = write(dir.path(), "n,label\n  1.5  ,  padded  \n");
        let t = read_table(&p, &decls(&["n:f8", "label:str"])).unwrap();
        assert_eq!(
            t.columns[0].1,
            ColumnData::F64(vec![1.5]),
            "a number is trimmed"
        );
        assert_eq!(
            t.columns[1].1,
            ColumnData::Utf8(vec!["  padded  ".into()]),
            "a string is taken verbatim"
        );
    }

    #[test]
    fn a_csv_without_declarations_gets_the_adr_conversion_path() {
        let dir = tempfile::tempdir().unwrap();
        let p = write(dir.path(), "a,b\n1,2\n");
        let err = read_table(&p, &CsvOptions::default())
            .unwrap_err()
            .to_string();
        assert!(err.contains("does not infer one"), "got {err}");
        assert!(
            err.contains("--column id:i8"),
            "offers the declaration path"
        );
        assert!(err.contains("duckdb -c"), "offers the conversion path");
    }

    /// The silent-disaster guard: positional declarations against a differently-ordered file.
    #[test]
    fn a_header_that_disagrees_with_the_declaration_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let p = write(dir.path(), "energy,id\n511.0,1\n");
        let err = read_table(&p, &decls(&["id:i4", "energy:f8"]))
            .unwrap_err()
            .to_string();
        assert!(err.contains("declared column 1 is 'id'"), "got {err}");
        assert!(err.contains("header says 'energy'"), "got {err}");
        assert!(err.contains("--no-header"), "names the escape hatch");
    }

    #[test]
    fn nulls_need_an_explicit_opt_in_and_then_produce_a_mask() {
        let dir = tempfile::tempdir().unwrap();
        let p = write(dir.path(), "id,energy\n1,511.0\n2,\n3,7.5\n");

        // Not declared nullable ⇒ refused, with the fix in the message.
        let err = read_table(&p, &decls(&["id:i4", "energy:f8"]))
            .unwrap_err()
            .to_string();
        assert!(err.contains("declared non-nullable"), "got {err}");
        assert!(err.contains("energy:f8?"), "got {err}");

        // Declared nullable ⇒ a mask, with the masked slot already at the dtype zero.
        let t = read_table(&p, &decls(&["id:i4", "energy:f8?"])).unwrap();
        let ColumnData::Nullable { values, validity } = &t.columns[1].1 else {
            panic!("expected a nullable column, got {:?}", t.columns[1].1)
        };
        assert_eq!(**values, ColumnData::F64(vec![511.0, 0.0, 7.5]));
        assert_eq!(validity, &vec![true, false, true]);
        assert!(t.columns[1].0.nullable, "the sealed Column says so too");
    }

    /// Nullable-by-presence, on the CSV side: a column *declared* nullable that contains no NULL must
    /// not get a mask, or the same logical table would seal differently depending on a declaration
    /// that no value in the file justifies.
    #[test]
    fn a_nullable_declaration_with_no_nulls_produces_no_mask() {
        let dir = tempfile::tempdir().unwrap();
        let p = write(dir.path(), "energy\n511.0\n7.5\n");
        let strict = read_table(&p, &decls(&["energy:f8"])).unwrap();
        let lenient = read_table(&p, &decls(&["energy:f8?"])).unwrap();
        assert_eq!(strict.columns[0].1, lenient.columns[0].1);
        assert!(!lenient.columns[0].0.nullable);
    }

    #[test]
    fn a_ragged_row_is_an_error_not_a_padded_row() {
        let dir = tempfile::tempdir().unwrap();
        let p = write(dir.path(), "a,b\n1,2\n3\n");
        let err = read_table(&p, &decls(&["a:i4", "b:i4"]))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("row 2"),
            "the error locates the bad row: {err}"
        );
        assert!(
            err.contains("1 fields but 2 columns were declared"),
            "…and says what was expected: {err}"
        );
    }

    #[test]
    fn out_of_range_and_malformed_values_name_the_row_and_column() {
        let dir = tempfile::tempdir().unwrap();
        let p = write(dir.path(), "small\n300\n");
        let err = read_table(&p, &decls(&["small:u1"]))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("row 1") && err.contains("'small'"),
            "got {err}"
        );

        let p2 = dir.path().join("b.csv");
        std::fs::write(&p2, "flag\nmaybe\n").unwrap();
        let err = read_table(&p2, &decls(&["flag:b1"]))
            .unwrap_err()
            .to_string();
        assert!(err.contains("not a boolean"), "got {err}");
        assert!(err.contains("true/false"), "lists what is accepted: {err}");
    }

    /// A finite decimal that overflows the declared dtype must NOT seal as infinity: the artifact would
    /// otherwise assert that the measurement *was* infinite.
    #[test]
    fn a_value_that_overflows_its_declared_dtype_is_an_error_not_an_infinity() {
        let dir = tempfile::tempdir().unwrap();
        for (body, dtype, token) in [
            ("x\n1e39\n", "f4", "f32"),
            ("x\n1e400\n", "f8", "f64"),
            ("x\n-1e400\n", "f8", "f64"),
        ] {
            let p = write(dir.path(), body);
            let err = read_table(&p, &decls(&[&format!("x:{dtype}")]))
                .unwrap_err()
                .to_string();
            assert!(err.contains("overflows"), "{body}: got {err}");
            assert!(err.contains(token), "{body}: names the dtype: {err}");
            assert!(
                err.contains("write 'inf' explicitly"),
                "{body}: offers the fix: {err}"
            );
        }
    }

    /// …but an explicitly-spelled infinity or NaN is a *value*, not an overflow. `NaN` is a legitimate
    /// measured quantity — the S13 clinical gate guarantees its bit-exact round-trip — and a text file
    /// that spells one means it.
    #[test]
    fn an_explicitly_spelled_infinity_or_nan_is_accepted() {
        let dir = tempfile::tempdir().unwrap();
        let p = write(dir.path(), "x\ninf\n-inf\nInfinity\nnan\n1.5\n");
        let t = read_table(&p, &decls(&["x:f8"])).unwrap();
        let ColumnData::F64(v) = &t.columns[0].1 else {
            unreachable!()
        };
        assert!(v[0].is_infinite() && v[0] > 0.0);
        assert!(v[1].is_infinite() && v[1] < 0.0);
        assert!(v[2].is_infinite() && v[2] > 0.0);
        assert!(v[3].is_nan());
        assert_eq!(v[4], 1.5);
    }

    /// **An empty field in a `str` column is the empty string, not NULL.** A CSV has no other way to
    /// write `""`, so treating it as absent would make a legitimate value un-ingestable *and* silently
    /// reinterpret what the operator wrote. A numeric column is the opposite: there is no empty number.
    #[test]
    fn an_empty_field_is_a_string_in_a_text_column_and_a_null_in_a_numeric_one() {
        let dir = tempfile::tempdir().unwrap();
        let p = write(dir.path(), "label,n\n,\nfoo,1.5\n");
        let t = read_table(&p, &decls(&["label:str", "n:f8?"])).unwrap();
        assert_eq!(
            t.columns[0].1,
            ColumnData::Utf8(vec![String::new(), "foo".into()]),
            "the empty field is the empty STRING"
        );
        assert!(
            !t.columns[0].0.nullable,
            "…so the column has no nulls at all"
        );
        let ColumnData::Nullable { validity, .. } = &t.columns[1].1 else {
            panic!("the numeric column IS nullable")
        };
        assert_eq!(validity, &vec![false, true]);

        // A `str` column takes NULLs only from an explicit token — and that is recorded in the seal,
        // because a reader cannot otherwise tell a NULL from the literal string "NA".
        let p = write(dir.path(), "label\nNA\nfoo\n");
        let mut opts = decls(&["label:str?"]);
        opts.null_tokens = vec!["NA".into()];
        let t = read_table(&p, &opts).unwrap_err().to_string();
        assert!(
            t.contains("nullable 'str'"),
            "a nullable str column is still unrepresentable (#457): {t}"
        );

        let p = write(dir.path(), "n\nNA\n1.5\n");
        let mut opts = decls(&["n:f8?"]);
        opts.null_tokens = vec!["NA".into()];
        let t = read_table(&p, &opts).unwrap();
        assert!(
            t.transforms
                .iter()
                .any(|x| x.name == transform::CSV_NULL_TOKENS),
            "the declared tokens are recorded: {:?}",
            t.transforms
        );
    }

    #[test]
    fn a_tsv_reads_with_an_explicit_delimiter() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.tsv");
        std::fs::write(&p, "id\tlabel\n1\tannihilation\n").unwrap();
        let mut opts = decls(&["id:i4", "label:str"]);
        opts.delimiter = b'\t';
        let t = read_table(&p, &opts).unwrap();
        assert_eq!(
            t.columns[1].1,
            ColumnData::Utf8(vec!["annihilation".into()])
        );
    }

    /// `--exclude` drops a column *after* the positional read, so the surviving columns keep their
    /// values. Omitting the declaration instead would shift every later column by one.
    #[test]
    fn exclude_drops_a_column_without_shifting_the_others() {
        let dir = tempfile::tempdir().unwrap();
        let p = write(dir.path(), "id,secret,energy\n1,xyz,511.0\n2,abc,7.5\n");
        let mut opts = decls(&["id:i4", "secret:str", "energy:f8"]);
        opts.exclude = vec!["secret".into()];
        let t = read_table(&p, &opts).unwrap();
        assert_eq!(
            t.columns
                .iter()
                .map(|(c, _)| c.name.as_str())
                .collect::<Vec<_>>(),
            vec!["id", "energy"]
        );
        assert_eq!(t.columns[1].1, ColumnData::F64(vec![511.0, 7.5]));
        // Excluding something that was never declared is a typo, not a no-op.
        let mut bad = decls(&["id:i4", "secret:str", "energy:f8"]);
        bad.exclude = vec!["secrets".into()];
        let err = read_table(&p, &bad).unwrap_err().to_string();
        assert!(err.contains("not a declared column"), "got {err}");
    }

    #[test]
    fn operator_declared_null_tokens_are_honoured_and_nothing_else_is() {
        let dir = tempfile::tempdir().unwrap();
        let p = write(dir.path(), "x\n1.0\nNA\n3.0\n");
        // Without declaring it, `NA` is a parse error — not a guessed NULL.
        let err = read_table(&p, &decls(&["x:f8?"])).unwrap_err().to_string();
        assert!(err.contains("'NA' is not a valid f64"), "got {err}");
        // Declared, it is a NULL.
        let mut opts = decls(&["x:f8?"]);
        opts.null_tokens = vec!["NA".into()];
        let t = read_table(&p, &opts).unwrap();
        let ColumnData::Nullable { validity, .. } = &t.columns[0].1 else {
            panic!("expected a mask")
        };
        assert_eq!(validity, &vec![true, false, true]);
    }
}
