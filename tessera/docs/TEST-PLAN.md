# Tessera test plan — the guarding scaffold (dev target)

Synthesised from a survey of the test suites of the formats Tessera composes/competes with:
**arrow-rs / parquet-testing / Lance** (columnar tables), **zarrs / zarr-python / h5py /
OME-NGFF** (N-D arrays), **Iceberg / Delta / Lance** (manifest + transactions), and
**BLAKE3 / git / IPFS / proptest / cargo-fuzz / NeXus / RO-Crate** (integrity + FAIR).

The benchmarks (fd5 #192/#193/#194 + this spike) are the *performance* guard; this plan is the
*correctness* guard. Tests are the dev target: a feature isn't done until its P0 row is green.

## Status legend
- **spine** = testable now on the manifest/identity/hash/provenance/product core (no backend).
- **zarrs** / **arrow** / **lance** = needs that block backend before the test can run.

## P0 — must be green before any reader/writer ships

### Integrity / identity / immutability (spine — implement first)
| Test | Intent | Module |
|---|---|---|
| `digest_kat_vectors` | pin `digest()` to BLAKE3 known-answer vectors; algo/encoding drift fails loud | hash |
| `merkle_root_deterministic_cross_run` | same ordered digests → identical root across runs/restart | hash |
| `merkle_root_order_sensitive` | reordering block digests changes the root | hash |
| `id_stable_and_distinct` | same identity inputs → same id; different → different | identity |
| `id_timestamp_normalised` | `…T00:00:00`, `…+00:00`, `…+01:00`(same instant) → ONE id (RFC3339/UTC) | identity |
| `manifest_json_roundtrip_proptest` | `from_json(to_json(m)) == m` over arbitrary manifests | manifest |
| `seal_idempotent` | two identical builds → byte-equal id + content_hash + JSON | product |
| `seal_immutability_invariants` | post-seal: content_hash set, blocks==refs, no `None` digest | product |
| `seal_rejects_missing_block_digest` | a block with no digest fails `seal()` (no silent drop) | product |
| `tamper_one_byte_changes_content_hash` | mutating any block spec/byte changes the root | hash+product |
| `block_reorder_changes_content_hash` | swapping blocks changes the root (order is semantic) | product |
| `unknown_tessera_version_errors_cleanly` | future major version → typed error, never panic/silent | manifest |
| `provenance_source_roundtrip` | sources DAG edges serialise/deserialise faithfully | provenance |

### Array correctness (zarrs)
| Test | Intent |
|---|---|
| `int16_ct_volume_roundtrip_487x512x512_cubic` | real CT shape, cubic chunks, **edge chunk on Z (487%64)** bit-equal |
| `fill_value_zero_vs_null_vs_air_hu` | `0` ≠ `null` ≠ `-1024` HU; unwritten chunks return the right value |
| `sharded_partial_shard_roi_read` | ROI crossing shard boundaries → minimal range-reads, bit-equal |
| `orthogonal_axial_sagittal_coronal` | extract each plane without whole-volume decode |
| `dimension_order_zyx_no_silent_swap` | ZYX vs XYZ → different content_hash, never aliased |
| `rescale_slope_intercept_preserved` | CT int16 + slope/intercept survive; HU = v·slope+intercept |
| `codec_snapshot_matrix` | golden chunk fixtures per dtype×codec; regen via env flag (zarrs pattern) |

### Table correctness (arrow / lance)
| Test | Intent |
|---|---|
| `table_roundtrip_numeric_limits` | per dtype write `-MIN,-100,-1,0,1,100,MAX`+null, exact back |
| `table_dtype_x_codec_matrix` | Cartesian dtype × {none,zstd,lz4} roundtrip |
| `table_all_nulls_and_empty_column` | all-null + zero-row; valid null mask + offsets; deterministic hash |
| `table_projection_subset` | read only `{ms,crystal_a,crystal_b}`; other columns untouched |
| `table_take_by_rowid` | take `[0,100,9999,last]` + multi-chunk strided take (lance) |
| `table_stats_minmax_no_lie` | recomputed min/max/null == recorded; **NaN excluded, counted** |
| `table_corrupt_block_detected` | truncated/zeroed/bad-rle blocks error cleanly, never panic |

## P1 — strong coverage
- `pruning_no_false_negative` / `pruning_skips_guaranteed_miss` (stats property tests — Iceberg/Delta).
- `field_id_rename_does_not_break_reads`, `field_id_drop_readd_new_id` (id-keyed schema evolution).
- `time_travel_reads_schema_of_that_version`; `version_chain_append_is_cas_optimistic`; conflict-class distinctness.
- `partial_write_no_visible_manifest` (block visible iff a sealed manifest references its digest).
- `golden_manifest_corpus_v0` (frozen id+hash fixtures; doubles as fuzz seed).
- `fuzz_manifest_from_json` (cargo-fuzz; arbitrary bytes never panic; seed = goldens).
- `canonical_json_hash_invariant` (re-pretty-print must NOT change content_hash — RFC 8785 JCS).
- `affine_roundtrip_float64_precision`; `big_endian_int16_read_compat`; `multiscale_pyramid_levels_share_scaled_affine`.
- `dictionary_fallback_on_high_cardinality`; `zstd_empty_page_no_codec_call`; `predicate_window_filter_skips_chunks`.

## P2 — later
- `manifest_of_manifests_scales_linearly`; `cross_version_compatibility_corpus`; `differential_reader_python_rust`.
- `description_completeness_lint` + `embedded_schema_validates_manifest` (AI-readability / FAIR).
- `units_string_well_formed` (UDUNITS-2/QUDT); `provenance_dag_acyclic` (needs resolver/corpus).
- `stateful_dag_op_model` (Hypothesis/proptest-state-machine).

## Gotchas to encode as regression tests (from the surveys)
1. **NaN / signed-zero in stats** — NaN never in min/max; counted separately; `-0.0 < +0.0`. Pin the chosen sort order in the manifest.
2. **Empty / zero-byte compressed pages** — don't call the codec on 0 bytes; a zstd stream → 0 bytes is legal.
3. **Dictionary fallback / malformed dict pages** — must spill to plain + reject negative dict-header counts without panic.
4. **Edge / partial chunks** (487%64) and chunk≥array and 0-length dims are legal and a distinct code path.
5. **Sharded reads have 3 failure modes** (index load / chunk-slice parse / chunk decode) — keep typed + isolated.
6. **dim-order / axes not round-trippable through shape alone** — ZYX→XYZ must change the hash.
7. **id-keyed (not name-keyed) schema evolution** — rename/drop+re-add-same-name must behave; re-add gets a NEW id.
8. **Conservative truncated min/max** — round min↓ / max↑ so range containment never lies.
9. **Conflicts are a taxonomy** — classify (ParentChanged / SchemaChanged / ProtocolChanged …), not one error.
10. **Block visible iff a sealed manifest references its digest** — never "iff the file exists" (orphan recovery).
11. **Canonical JSON before hashing** — map order / number normalization / whitespace must not move the hash.
12. **Timezone-naive timestamps** — normalize to RFC3339 UTC before they feed `id`.
13. **Fill-value polymorphism** — `0` ≠ `null` ≠ dtype-default; for CT, `0` HU ≠ air `-1024`.
14. **Empty-digest collapse in `seal()`** — a `None` digest silently dropped from the Merkle root; require it.
15. **LZ4 has 3 framings** — if ever offered, name the framing explicitly in the codec field.

## Tooling
- `proptest` (roundtrip + stats properties) · `cargo-fuzz` + `arbitrary` (parser) · golden fixtures under
  `tests/golden/vX.Y/` · snapshot fixtures under `tests/data/snapshots/{dtype}/{codec}/` (zarrs pattern) ·
  per-released-version migration corpus (Lance pattern).
