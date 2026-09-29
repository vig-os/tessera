//! End-to-end determinism for generic table ingest (#386, ADR-0056 §5).
//!
//! The unit tests in `arrow_table.rs` / `csv_table.rs` pin the type map column by column. These pin the
//! property the *seal* actually claims, which is stronger and different:
//!
//! > For any input `F` and any tessera version `V`, `tessera ingest F` produces the same `content_hash`
//! > under every distributed build of `V`. Feature selection may change which formats are **readable**;
//! > it must never change the **bytes** produced for a readable one.  — ADR-0057 §7
//!
//! ADR-0056 §5 makes the point that has to be internalised to read these tests: the determinism risk of
//! generic ingest is **upstream** of our encoder. Tessera's codecs are already byte-deterministic and
//! gated by the conformance corpus. What is new is that two decoders — or one decoder handed the same
//! logical table encoded two ways — can produce two different `ColumnData`, and *both* then encode
//! deterministically to two different `content_hash`es. So the tests here all have the same shape:
//! produce the same **logical** table by deliberately different physical means, and require one hash.
//!
//! The one ADR-0056 §5 fixture these cannot cover is `ingest_parquet_producers` — the same table written
//! by pyarrow, polars and DuckDB. That needs three foreign writers, so it lives in the hermetic
//! `ingest-producer-equality` nix check instead of here. What *is* covered here is the mechanism that
//! fixture exists to test: dictionary ordering and null padding not reaching the hash.

use std::path::Path;
use std::sync::Arc;

use arrow_array::{ArrayRef, Float64Array, Int32Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use parquet::arrow::ArrowWriter;
use parquet::basic::{Compression, ZstdLevel};
use parquet::file::properties::{EnabledStatistics, WriterProperties};
use tessera_core::collection::{member_filename, MemberKind, Role};
use tessera_core::manifest::Manifest;
use tessera_ingest::column_meta::ColumnMeta;
use tessera_ingest::spec::{CollectionMeta, FormatOptions, IngestSpec, ProductSpec, SpecMeta};
use tessera_ingest::{decoder, engine};

const TS: &str = "2024-03-01T12:00:00Z";

/// The one logical table every physical variant in this file must reduce to.
///
/// Deliberately exercises the three things that most often leak a producer's choices into a hash: a
/// **low-cardinality string** column (the dictionary-ordering hazard), a **nullable** column (the
/// null-padding hazard), and a float column (the NaN/rounding hazard).
fn logical_batch() -> RecordBatch {
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int32, false),
        Field::new("label", DataType::Utf8, false),
        Field::new("energy", DataType::Float64, true),
    ]));
    let id: ArrayRef = Arc::new(Int32Array::from(vec![1, 2, 3, 4, 5, 6]));
    let label: ArrayRef = Arc::new(StringArray::from(vec![
        "red", "green", "red", "blue", "green", "red",
    ]));
    let energy: ArrayRef = Arc::new(Float64Array::from(vec![
        Some(511.0),
        None,
        Some(7.25),
        Some(-0.5),
        None,
        Some(1e-9),
    ]));
    RecordBatch::try_new(schema, vec![id, label, energy]).expect("fixture batch")
}

/// Write `logical_batch()` to a Parquet file under a deliberately-chosen physical encoding.
fn write_parquet(path: &Path, props: WriterProperties) {
    let batch = logical_batch();
    let file = std::fs::File::create(path).expect("create parquet");
    let mut w = ArrowWriter::try_new(file, batch.schema(), Some(props)).expect("writer");
    w.write(&batch).expect("write");
    w.close().expect("close");
}

/// Run a one-product generic-ingest spec through the real engine and return the sealed manifest.
fn ingest(dir: &Path, options: FormatOptions, schema: &str) -> tessera_core::Result<Manifest> {
    ingest_with_config(dir, options, schema, &tessera_io::WriteConfig::default())
}

fn ingest_with_config(
    dir: &Path,
    options: FormatOptions,
    schema: &str,
    cfg: &tessera_io::WriteConfig,
) -> tessera_core::Result<Manifest> {
    let out = dir.join(format!("out-{}", uniq()));
    let spec = IngestSpec {
        collection: CollectionMeta {
            name: "gen-01".into(),
            description: None,
            timestamp: TS.into(),
            study: None,
        },
        spec: SpecMeta::default(),
        products: vec![ProductSpec {
            name: "gen-01".into(),
            role: Role::Raw,
            schema: schema.into(),
            description: None,
            derived_from: Vec::new(),
            // A stable label rather than the tempdir path: the path differs per run, and it would
            // otherwise land in the sealed `ingested_from` edge and move `manifest_hash`. (ADR-0040
            // wants this anyway for PHI hygiene; here it is also what makes the test meaningful.)
            source_label: Some("fixture/table".into()),
            metadata: Default::default(),
            generation: None,
            producer: None,
            options,
        }],
    };
    let coll = engine::run(
        &spec,
        Path::new("test-inline-spec"),
        &out,
        cfg,
        engine::DEFAULT_STREAM_THRESHOLD_BYTES,
    )?;
    let member = &coll.members[0];
    let path = out.join(member_filename(&member.reference, MemberKind::Product));
    Ok(tessera_io::Reader::open(&path)?.manifest().clone())
}

/// A per-call unique suffix, so several ingests in one test do not collide on an output directory.
fn uniq() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    N.fetch_add(1, Ordering::Relaxed)
}

/// The hash triple a determinism claim is actually about.
fn triple(m: &Manifest) -> (String, String, String) {
    (
        m.id.clone(),
        m.content_hash.clone().expect("sealed"),
        m.manifest_hash.clone().expect("sealed"),
    )
}

// ── the headline property ─────────────────────────────────────────────────────────────────────

/// **The same logical table, written five deliberately different physical ways, seals to one hash.**
///
/// This is the mechanism behind ADR-0056 §5's `ingest_parquet_producers` fixture. Each variant is a
/// choice a real writer makes differently:
///
/// - **dictionary on vs off** — the one that matters most. Preserving a writer's integer codes would
///   import its *dictionary order* into `content_hash`, and pandas' `Categorical` order is
///   insertion-time, so the same data from pandas, polars and Spark would seal three ways. §2 therefore
///   materialises dictionaries to their values.
/// - **row-group size** — changes how many chunks the reader hands us, i.e. exactly the fold this
///   module's canonicaliser performs.
/// - **compression** — snappy vs zstd vs none. Ingest is a logical re-encode, so the source codec is
///   irrelevant by construction; this is what proves "by construction" is not wishful.
/// - **statistics** — page/chunk metadata that must not reach a value.
#[test]
fn the_same_logical_table_seals_identically_however_it_was_physically_written() {
    let dir = tempfile::tempdir().unwrap();
    let variants: Vec<(&str, WriterProperties)> = vec![
        (
            "dictionary-on-snappy",
            WriterProperties::builder()
                .set_dictionary_enabled(true)
                .set_compression(Compression::SNAPPY)
                .build(),
        ),
        (
            "dictionary-off-snappy",
            WriterProperties::builder()
                .set_dictionary_enabled(false)
                .set_compression(Compression::SNAPPY)
                .build(),
        ),
        (
            "uncompressed-no-stats",
            WriterProperties::builder()
                .set_compression(Compression::UNCOMPRESSED)
                .set_statistics_enabled(EnabledStatistics::None)
                .build(),
        ),
        (
            "zstd-page-stats",
            WriterProperties::builder()
                .set_compression(Compression::ZSTD(ZstdLevel::try_new(3).unwrap()))
                .set_statistics_enabled(EnabledStatistics::Page)
                .build(),
        ),
        (
            "tiny-row-groups",
            WriterProperties::builder()
                .set_max_row_group_row_count(Some(2))
                .set_dictionary_enabled(true)
                .build(),
        ),
    ];

    let mut seen: Vec<(&str, (String, String, String), u64)> = Vec::new();
    for (name, props) in variants {
        let p = dir.path().join(format!("{name}.parquet"));
        write_parquet(&p, props);
        let bytes = std::fs::metadata(&p).unwrap().len();
        let m = ingest(
            dir.path(),
            FormatOptions::Parquet {
                input: p,
                exclude: Vec::new(),
                column_meta: ColumnMeta::empty(),
            },
            "table",
        )
        .unwrap_or_else(|e| panic!("{name}: {e}"));
        seen.push((name, triple(&m), bytes));
    }

    // The source files genuinely differ — otherwise this test would be vacuous.
    let sizes: std::collections::BTreeSet<u64> = seen.iter().map(|(_, _, b)| *b).collect();
    assert!(
        sizes.len() > 1,
        "the five writer configurations produced identical files, so this proves nothing: {sizes:?}"
    );

    let (first_name, first, _) = &seen[0];
    for (name, t, _) in &seen[1..] {
        // `id` and `content_hash` — the identity and the payload — must be byte-identical. This is
        // ADR-0056 §6a Finding 1 made executable: `content_hash = f(extracted logical values,
        // Tessera's encoder)`, so nothing about how the source was *encoded* can reach it.
        assert_eq!(
            (&t.0, &t.1),
            (&first.0, &first.1),
            "'{name}' produced a different id/content_hash from '{first_name}' — a physical writer \
             choice reached the payload, which is the exact leak ADR-0056 §2 materialises \
             dictionaries and re-encodes to prevent"
        );
        // `manifest_hash` legitimately DIFFERS, and the reason is a feature rather than a leak: the
        // sealed `ingested_from` edge pins a merkle root over the *source bytes*, and these five
        // files genuinely are different bytes. That edge is what makes decoder drift detectable from
        // sealed data alone — same source digest + changed `content_hash` ⟹ the interpretation
        // changed (ADR-0056 §6a Finding 2) — so it must not be weakened to make this test tidier.
        assert_ne!(
            t.2, first.2,
            "'{name}' and '{first_name}' are different source bytes, so the seal must say so"
        );
    }

    // State the invariant the other way round too, so a future change that made `content_hash`
    // source-byte-dependent could not pass by also making `manifest_hash` collide.
    let payloads: std::collections::BTreeSet<&String> = seen.iter().map(|(_, t, _)| &t.1).collect();
    assert_eq!(payloads.len(), 1, "exactly one payload across every writer");
    let seals: std::collections::BTreeSet<&String> = seen.iter().map(|(_, t, _)| &t.2).collect();
    assert_eq!(seals.len(), seen.len(), "one seal per distinct source");
}

/// The batch size the reader is driven at is an RSS knob, not a determinism input. A reader would be
/// right to suspect it could be one — the canonicaliser folds chunks — so it is pinned.
#[test]
fn the_reader_batch_size_does_not_reach_the_hash() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("b.parquet");
    write_parquet(&p, WriterProperties::builder().build());

    let mut hashes = Vec::new();
    for batch_rows in [1usize, 2, 5, 8192] {
        let batches = tessera_ingest::parquet_table::read_parquet_batched(&p, batch_rows).unwrap();
        assert!(
            batch_rows >= 6 || batches.len() > 1,
            "batch_rows={batch_rows} must actually split the 6-row fixture"
        );
        let t = tessera_ingest::arrow_table::canonicalise_batches(&batches, &[]).unwrap();
        let (block, _payload) = tessera_io::table::table_block(
            tessera_ingest::canonical::GENERIC_BLOCK,
            &t.spec().unwrap(),
            &t.data(),
        )
        .unwrap();
        hashes.push((batch_rows, block.digest.clone()));
    }
    let first = &hashes[0].1;
    for (n, h) in &hashes[1..] {
        assert_eq!(h, first, "batch_rows={n} changed the encoded block digest");
    }
}

/// The brief's determinism requirement, literally: the same input under different **worker counts**.
///
/// The generic table lane encodes single-shot (it does not use the multi-block streaming writer), so
/// `WriteConfig` cannot reach its bytes — but "cannot" is a claim worth a test rather than a comment,
/// because the day generic ingest *does* grow a streaming path this is the test that fails.
#[test]
fn worker_count_and_ram_budget_do_not_reach_the_hash() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("w.parquet");
    write_parquet(&p, WriterProperties::builder().build());

    let opts = || FormatOptions::Parquet {
        input: p.clone(),
        exclude: Vec::new(),
        column_meta: ColumnMeta::empty(),
    };
    let one = tessera_io::WriteConfig::default().workers(1);
    let many = tessera_io::WriteConfig::default().workers(8);
    assert_ne!(
        one.worker_count(),
        many.worker_count(),
        "the two configs must actually differ, or this test is vacuous"
    );
    let a = ingest_with_config(dir.path(), opts(), "table", &one).unwrap();
    let b = ingest_with_config(dir.path(), opts(), "table", &many).unwrap();
    assert_eq!(triple(&a), triple(&b), "worker count moved a hash");
}

/// Re-running the same ingest reproduces the same product exactly — the property `id` depends on.
#[test]
fn re_running_the_same_ingest_is_byte_identical() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("r.parquet");
    write_parquet(&p, WriterProperties::builder().build());
    let opts = || FormatOptions::Parquet {
        input: p.clone(),
        exclude: Vec::new(),
        column_meta: ColumnMeta::empty(),
    };
    let a = ingest(dir.path(), opts(), "table").unwrap();
    let b = ingest(dir.path(), opts(), "table").unwrap();
    assert_eq!(triple(&a), triple(&b));
}

/// **A CSV and the Parquet a converter would make from it seal to the same hash.**
///
/// The strongest statement of "the container is not the product" available: ADR-0056 §8's own advice is
/// `duckdb -c "COPY … TO 'input.parquet'"`, and this proves that following that advice and *not*
/// following it land in the same place. It is also why the CSV lane must apply the same
/// nullable-by-presence rule the Arrow lane does — a declared-nullable CSV column with no nulls has to
/// come out unwrapped, or the two paths would diverge.
#[test]
fn a_csv_and_its_parquet_equivalent_seal_to_the_same_hash() {
    let dir = tempfile::tempdir().unwrap();

    let pq = dir.path().join("x.parquet");
    write_parquet(&pq, WriterProperties::builder().build());

    // The same logical table as text. The empty fields are the two NULLs.
    let csv = dir.path().join("x.csv");
    std::fs::write(
        &csv,
        "id,label,energy\n1,red,511.0\n2,green,\n3,red,7.25\n4,blue,-0.5\n5,green,\n6,red,1e-9\n",
    )
    .unwrap();

    let from_pq = ingest(
        dir.path(),
        FormatOptions::Parquet {
            input: pq,
            exclude: Vec::new(),
            column_meta: ColumnMeta::empty(),
        },
        "table",
    )
    .unwrap();
    let from_csv = ingest(
        dir.path(),
        FormatOptions::Csv {
            input: csv,
            columns: vec!["id:i4".into(), "label:str".into(), "energy:f8?".into()],
            delimiter: None,
            header: true,
            null_tokens: Vec::new(),
            exclude: Vec::new(),
            column_meta: ColumnMeta::empty(),
        },
        "table",
    )
    .unwrap();

    assert_eq!(
        from_pq.content_hash, from_csv.content_hash,
        "the same logical table read from CSV and from Parquet must produce the same payload"
    );
    // `manifest_hash` legitimately differs: the recipe records a different decoder and a different
    // `source_format`, which is exactly what a recipe is for. `id` is unaffected either way, because
    // product metadata is not an identity input.
    assert_eq!(from_pq.id, from_csv.id);
    assert_ne!(
        from_pq.manifest_hash, from_csv.manifest_hash,
        "the two were made differently, so the SEAL should say so"
    );
}

// ── what the seal records ─────────────────────────────────────────────────────────────────────

/// ADR-0056 §6a: every generic ingest seals the decoder triple in the provenance recipe bag, and
/// ADR-0056 §6.2's transform receipt rides in the manifest.
#[test]
fn the_sealed_product_carries_the_decoder_triple_the_receipt_and_the_source_digest() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("d.parquet");
    write_parquet(&p, WriterProperties::builder().build());
    let m = ingest(
        dir.path(),
        FormatOptions::Parquet {
            input: p.clone(),
            exclude: Vec::new(),
            column_meta: ColumnMeta::empty(),
        },
        "table",
    )
    .unwrap();

    // The recipe bag, under the well-known key. Not a format field — no new manifest surface.
    let g = m.generation.as_ref().expect("a recipe is recorded");
    let d = &g.config[decoder::RECIPE_KEY];
    assert_eq!(d["name"], "arrow-rs/parquet");
    assert!(
        d["version"].as_str().unwrap().starts_with('='),
        "the version is recorded as a PIN: {d}"
    );
    assert_eq!(
        d["features"].as_str().map(str::to_owned),
        decoder::feature_digest(),
        "the sealed feature component is the digest this build derives"
    );

    // The transform receipt: this fixture has a nullable column, so H5 fired and says so.
    let names: Vec<&str> = m.ingest_transform.iter().map(|t| t.name.as_str()).collect();
    assert!(
        names.contains(&"null_slot_normalisation"),
        "the receipt must record the H5 normalisation that actually happened: {names:?}"
    );

    // ADR-0056's consequence: the `ingested_from` source digest is what makes decoder drift
    // detectable from sealed data alone (same source digest + changed content_hash ⟹ the
    // interpretation changed), so it is promoted from convention to a requirement here.
    let edge = m
        .sources
        .iter()
        .find(|s| s.role == "ingested_from")
        .expect("an ingested_from edge is mandatory for generic ingest");
    assert_eq!(
        edge.reference, "fixture/table",
        "the label replaced the path"
    );
    assert_eq!(
        edge.content_hash.as_deref(),
        Some(
            tessera_ingest::provenance::source_digest(&[p.as_path()])
                .unwrap()
                .as_str()
        ),
        "the edge pins the SOURCE bytes"
    );

    // The product declares the primitive schema and carries the source format for a reader.
    assert_eq!(m.product, "table");
    assert_eq!(m.metadata["source_format"], "parquet");
    m.verify().unwrap();
}

/// Every column is stamped `Unknown` — and that has to survive the seal, because the whole point is
/// that a downstream consumer reads it off the manifest.
#[test]
fn sealed_columns_are_stamped_unclassified_unless_the_operator_says_otherwise() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("s.parquet");
    write_parquet(&p, WriterProperties::builder().build());

    let bare = ingest(
        dir.path(),
        FormatOptions::Parquet {
            input: p.clone(),
            exclude: Vec::new(),
            column_meta: ColumnMeta::empty(),
        },
        "table",
    )
    .unwrap();
    let spec = block_spec(&bare);
    for c in &spec.columns {
        assert_eq!(
            c.sensitivity,
            tessera_core::schema::Sensitivity::Unknown,
            "column '{}' must not claim to be Public — nobody classified it",
            c.name
        );
    }

    // …and `--column-meta` lands INSIDE the seal (ADR-0056 §7's "it ships with the door").
    let annotated = ingest(
        dir.path(),
        FormatOptions::Parquet {
            input: p,
            exclude: Vec::new(),
            column_meta: ColumnMeta::parse(
                r#"
                [energy]
                short_name = "Photon energy"
                description = "Calibrated per-photon energy"
                unit = "keV"

                [label]
                sensitivity = "coded"
                "#,
            )
            .unwrap(),
        },
        "table",
    )
    .unwrap();
    let spec = block_spec(&annotated);
    let energy = spec.columns.iter().find(|c| c.name == "energy").unwrap();
    assert_eq!(energy.unit.as_deref(), Some("keV"));
    assert_eq!(energy.short_name.as_deref(), Some("Photon energy"));
    let label = spec.columns.iter().find(|c| c.name == "label").unwrap();
    assert_eq!(label.sensitivity, tessera_core::schema::Sensitivity::Coded);

    // Annotating changes the manifest, so the seal moves — but not the payload, so `content_hash`
    // does not. That separation is what makes "label it later" a real workflow.
    assert_eq!(bare.content_hash, annotated.content_hash);
    assert_ne!(bare.manifest_hash, annotated.manifest_hash);
}

/// The sealed `TableSpec` of the single `data` block.
fn block_spec(m: &Manifest) -> tessera_core::block::table::TableSpec {
    let b = m
        .blocks
        .iter()
        .find(|b| b.name == tessera_ingest::canonical::GENERIC_BLOCK)
        .expect("the data block");
    serde_json::from_value(b.spec.clone()).expect("a table spec")
}

// ── values actually survive ───────────────────────────────────────────────────────────────────

/// The round-trip the whole feature is for: `parquet → tsra → read == source`.
#[test]
fn values_round_trip_through_the_seal() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("rt.parquet");
    write_parquet(&p, WriterProperties::builder().build());
    let out = dir.path().join("rt-out");
    let spec = one_product_spec(
        FormatOptions::Parquet {
            input: p,
            exclude: Vec::new(),
            column_meta: ColumnMeta::empty(),
        },
        "table",
    );
    let coll = engine::run(
        &spec,
        Path::new("test-inline-spec"),
        &out,
        &tessera_io::WriteConfig::default(),
        engine::DEFAULT_STREAM_THRESHOLD_BYTES,
    )
    .unwrap();
    let path = out.join(member_filename(
        &coll.members[0].reference,
        MemberKind::Product,
    ));
    let mut r = tessera_io::Reader::open(&path).unwrap();
    // Decode through the SEALED spec, not a spec the test rebuilds: that is what makes this a
    // round-trip of the artifact rather than of our in-memory objects.
    let spec = block_spec(r.manifest());
    let blob = r
        .read_block(tessera_ingest::canonical::GENERIC_BLOCK)
        .unwrap();
    let table = tessera_io::table::decode(&spec, &blob).expect("the data block decodes");
    let by_name = |n: &str| {
        table
            .iter()
            .find(|(name, _)| name == n)
            .map(|(_, d)| d.clone())
            .unwrap_or_else(|| panic!("column '{n}' missing"))
    };
    use tessera_io::table::ColumnData;
    assert_eq!(by_name("id"), ColumnData::I32(vec![1, 2, 3, 4, 5, 6]));
    assert_eq!(
        by_name("label"),
        ColumnData::Utf8(
            ["red", "green", "red", "blue", "green", "red"]
                .iter()
                .map(|s| s.to_string())
                .collect()
        )
    );
    let ColumnData::Nullable { values, validity } = by_name("energy") else {
        panic!("energy must decode as nullable")
    };
    assert_eq!(validity, vec![true, false, true, true, false, true]);
    assert_eq!(
        *values,
        ColumnData::F64(vec![511.0, 0.0, 7.25, -0.5, 0.0, 1e-9]),
        "present values survive exactly; masked slots read back as the dtype default (H5)"
    );
}

fn one_product_spec(options: FormatOptions, schema: &str) -> IngestSpec {
    IngestSpec {
        collection: CollectionMeta {
            name: "gen-01".into(),
            description: None,
            timestamp: TS.into(),
            study: None,
        },
        spec: SpecMeta::default(),
        products: vec![ProductSpec {
            name: "gen-01".into(),
            role: Role::Raw,
            schema: schema.into(),
            description: None,
            derived_from: Vec::new(),
            source_label: Some("fixture/table".into()),
            metadata: Default::default(),
            generation: None,
            producer: None,
            options,
        }],
    }
}

// ── the laundering rule, end to end ───────────────────────────────────────────────────────────

/// ADR-0056 §7: a generic backend may not mint a product carrying a vendor schema's promises.
///
/// The artifact this prevents would be **byte-indistinguishable** from a real vendor listmode ingest
/// while carrying PS3.15 `Identifying` tiers no classification pass ever validated.
#[test]
fn a_generic_backend_cannot_launder_a_vendor_schema() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("l.parquet");
    write_parquet(&p, WriterProperties::builder().build());
    let err = ingest(
        dir.path(),
        FormatOptions::Parquet {
            input: p,
            exclude: Vec::new(),
            column_meta: ColumnMeta::empty(),
        },
        "listmode",
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("generic 'parquet' backend"), "got {err}");
    assert!(err.contains("claims schema 'listmode'"), "got {err}");
    assert!(err.contains(r#"schema = "table""#), "offers the fix: {err}");
}

/// The rule must reject **before** any bytes are read, so a bad spec has no side effects. Pointing it
/// at a file that does not exist is the cheapest way to prove the ordering.
#[test]
fn the_laundering_rule_fires_before_the_file_is_opened() {
    let dir = tempfile::tempdir().unwrap();
    let err = ingest(
        dir.path(),
        FormatOptions::Parquet {
            input: dir.path().join("does-not-exist.parquet"),
            exclude: Vec::new(),
            column_meta: ColumnMeta::empty(),
        },
        "recon",
    )
    .unwrap_err()
    .to_string();
    assert!(
        err.contains("claims schema 'recon'"),
        "expected the laundering rejection, not an I/O error: {err}"
    );
}

/// A rejected spec must leave **nothing** behind — not even an empty output directory. An operator
/// told "this spec cannot run" should not then find a directory suggesting it partly did.
#[test]
fn a_rejected_spec_creates_no_output_directory() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("should-not-exist");
    let spec = one_product_spec(
        FormatOptions::Parquet {
            input: dir.path().join("whatever.parquet"),
            exclude: Vec::new(),
            column_meta: ColumnMeta::empty(),
        },
        "listmode",
    );
    let err = engine::run(
        &spec,
        Path::new("test-inline-spec"),
        &out,
        &tessera_io::WriteConfig::default(),
        engine::DEFAULT_STREAM_THRESHOLD_BYTES,
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("claims schema 'listmode'"), "got {err}");
    assert!(
        !out.exists(),
        "a spec rejected at the door must not have created {}",
        out.display()
    );
}

/// The allowlisted primitive schema is accepted, so the rule is a filter and not a blanket refusal.
#[test]
fn the_primitive_schema_is_accepted() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("ok.parquet");
    write_parquet(&p, WriterProperties::builder().build());
    ingest(
        dir.path(),
        FormatOptions::Parquet {
            input: p,
            exclude: Vec::new(),
            column_meta: ColumnMeta::empty(),
        },
        "table",
    )
    .expect("schema = \"table\" is the allowed claim");
}

// ── Arrow IPC parity ──────────────────────────────────────────────────────────────────────────

/// Arrow IPC and Parquet differ only in the container, so the same logical table through both must
/// produce the same payload. If they ever diverge, the type map has grown a format-specific branch —
/// which is exactly the thing ADR-0056 §2 factors out.
#[test]
fn arrow_ipc_and_parquet_produce_the_same_payload() {
    let dir = tempfile::tempdir().unwrap();

    let pq = dir.path().join("p.parquet");
    write_parquet(&pq, WriterProperties::builder().build());

    let ipc = dir.path().join("p.arrow");
    {
        let batch = logical_batch();
        let file = std::fs::File::create(&ipc).unwrap();
        let mut w = arrow_ipc::writer::FileWriter::try_new(file, &batch.schema()).unwrap();
        w.write(&batch).unwrap();
        w.finish().unwrap();
    }

    let a = ingest(
        dir.path(),
        FormatOptions::Parquet {
            input: pq,
            exclude: Vec::new(),
            column_meta: ColumnMeta::empty(),
        },
        "table",
    )
    .unwrap();
    let b = ingest(
        dir.path(),
        FormatOptions::Arrow {
            input: ipc,
            exclude: Vec::new(),
            column_meta: ColumnMeta::empty(),
        },
        "table",
    )
    .unwrap();
    assert_eq!(a.content_hash, b.content_hash);
    assert_eq!(a.metadata["source_format"], "parquet");
    assert_eq!(b.metadata["source_format"], "arrow");
}

/// **ADR-0056 §6a: nobody types the decoder record.**
///
/// The key's contract is "a mechanically derived build-honest triple… it is derived, or it is not
/// written" — so a spec-supplied `ingest_decoder` is a hard error for **every** backend, not just the
/// generic ones. The vendor case is the one that matters: a `dicom` or `hdf-compound` product writes no
/// decoder record at all, so a hand-written one would seal *unchanged* and be indistinguishable from a
/// derived triple to every later reader — a sealed, signed, unfalsifiable claim about how the file was
/// interpreted, which is exactly what §6a rejected a profile id for.
#[test]
fn a_hand_written_ingest_decoder_is_refused_for_every_backend() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("h.parquet");
    write_parquet(&p, WriterProperties::builder().build());

    let typed = tessera_core::Generation::default().with(
        tessera_ingest::decoder::RECIPE_KEY,
        serde_json::json!({"name": "totally-legit", "version": "=99.0.0"}),
    );

    // The generic lane, where it would merely be overwritten…
    let mut spec = one_product_spec(
        FormatOptions::Parquet {
            input: p.clone(),
            exclude: Vec::new(),
            column_meta: ColumnMeta::empty(),
        },
        "table",
    );
    spec.products[0].generation = Some(typed.clone());
    let err = tessera_ingest::spec::validate(&spec)
        .unwrap_err()
        .to_string();
    assert!(err.contains("by hand"), "got {err}");
    assert!(err.contains("unfalsifiable"), "says why: {err}");

    // …and a VENDOR backend, where it would seal unchanged. This is the case with teeth.
    let mut vendor = one_product_spec(
        FormatOptions::Nifti {
            input: dir.path().join("scan.nii"),
        },
        "recon",
    );
    vendor.products[0].generation = Some(typed);
    let err = tessera_ingest::spec::validate(&vendor)
        .unwrap_err()
        .to_string();
    assert!(err.contains("by hand"), "got {err}");

    // An operator's OWN recipe keys are untouched — the bag is for exactly that.
    let mut ok = one_product_spec(
        FormatOptions::Parquet {
            input: p,
            exclude: Vec::new(),
            column_meta: ColumnMeta::empty(),
        },
        "table",
    );
    ok.products[0].generation = Some(
        tessera_core::Generation::default().with("energy_window", serde_json::json!([425, 650])),
    );
    tessera_ingest::spec::validate(&ok).expect("an operator's own recipe keys are fine");
}
