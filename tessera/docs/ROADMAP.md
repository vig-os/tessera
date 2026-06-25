# Tessera — end-to-end roadmap (phases · milestones · gates · critical path)

Single source for "where we're going + in what order." Pairs with `FEATURE-MATRIX.md` ("what
passes") and the RFC ("what it is"). Effort = rough dev-weeks (1 focused dev). Status: ✓/◑/○.

## Critical path (the dependency spine — do in this order)
```
P0 ADRs ─▶ P1 core ─▶ P2 read-API ─▶ P3 io(write) ─▶ P4 conformance ═▶ v0.1
  │                                                        │
  └─ canonical-JSON + identity + container-spec            └─▶ P5 ingest ─▶ P6 integrity/dist ═▶ v0.3
     poison every fixture if deferred → FIRST                              └─▶ P7 bindings/ops ═▶ v0.5 ─▶ P8 stabilize ═▶ v1.0
```
**Hard orderings:** canonical-JSON+identity (#20) before *any* hashing/fixtures · read-API (#21b)
before the write-engine (else reader-hostile) · container spec (#22) before layout/read ·
conformance corpus (#21c) gates v1.0.

## Phases
| # | Phase | Scope (tasks) | Done-gate | ~wk |
|---|---|---|---|--:|
| **P0** | De-risk & ADRs | **S13 ✓**, **S15 remainder** (cross-ver/arch + vendored-reader prototype); ADRs: D4 canonical-encoding, D5 identity, #22 versioning-DAG+container, D3 schema-id allocator, D2 sync/async, D1 fd5 repo | ADRs accepted; vendored reader decodes a pinned-version file | 1 |
| **P1** | `tessera-core` finish | #20 manifest+BlockRef schema; #19 restore fd5 conventions+fields (id_inputs · `_type`/`_version` · `_vocabulary`/`_code` · `default` · `extra/` · `sources` roles+resolve · `study` · units · axes · `fill_value` · descriptions); #21a error taxonomy; product schemas (recon/listmode/sinogram/spectrum/roi/transform/calibration/sim/device_data) + required-field tables; D7 encryption non-goal | all §C correctness + spine tests green incl canonical-hash + validation | 2 |
| **P2** | Read path **first** | #21b Reader API (`open`/range-read/block-handle/partial-product) + `object_store` backend | reads a hand-built `.tessera`; range-read a chunk | 1 |
| **P3** | `tessera-io` write engine | S5 zarrs backend; S17 streaming (fragment-append · hash-on-write · incremental Merkle · crash-recovery to watermark); S3 chunk-Merkle integrity tree; container writer; observability (`tracing` on watermarks) | acq→sealed roundtrip; crash-recovery resumes; §D perf-SLA met | 4–6 |
| **P4** | Conformance + CLI → **v0.1** | #21c conformance corpus + `SPEC.md`; `tessera-cli` (pack/unpack/verify/inspect); perf-SLA CI gates; S6 object-store range-read | 4 release gates green (conformance · roundtrip · SLA · determinism) | 2 |
| **P5** | Ingest → **v0.2/v0.3** | S9 DICOM (files/DICOMweb/DIMSE, PS3.15 verify, lossless tags, rescale/units, egress); then GE-HDF5 · Siemens · raw `.dat`/`.BLF` · NIfTI; S14 cross-shape query demo | lossless DICOM roundtrip + egress; golden DICOM corpus | 4 |
| **P6** | Integrity & distribution → **v0.3** | S16 signing (cosign minimal → source-rooted chain-verify); WORM/Object-Lock; OCI artifact mapping; RO-Crate/DataCite/tessera-index exports | chain-of-custody verify; OCI push/pull; WORM enforced on MinIO | 3 |
| **P7** | Bindings & ops → **v0.5** | pyo3 (`tessera-py`); reference podman-compose stack (zot+MinIO+InvenioRDM+cosign); migration tooling (`schema diff/validate`); format-spec semver policy | Python parity passes conformance; ref-stack smoke test | 4 |
| **P8** | Spec stabilization → **v1.0** | 2nd independent reader (C-ABI/WASM/Python) passes corpus; cross-version determinism; freeze `tessera-1.0`; deprecation policy | 2nd impl green; 12 mo zero-breaking since v0.5 | — |

## Milestone gates (definition of shippable)
Per `FEATURE-MATRIX.md §H`: **shippable = ① conformance corpus · ② bit-exact roundtrip · ③ perf-SLA
(§D floors) · ④ writer-determinism** — all green on the supported matrix. v0.1 freezes the wire format.

## Open gating decisions (register — decide by the listed phase)
| id | decision | options | by |
|---|---|---|---|
| D1 | fd5 supersession vs sibling repo | rename `vig-os/fd5`→`tessera` (keep history) **vs** subtree split | P0 |
| D2 | concurrency model | sync `core` / async `io` (tokio + `object_store`) + `rayon` encode pool, `spawn_blocking` boundary | P0 |
| D3 | schema-id allocation | per-schema monotonic + `<plugin>:<id>` namespacing + reserved ranges | P0/P1 |
| D4 | canonical encoding for hashing | RFC 8785 JCS-JSON **vs** deterministic CBOR | P0 |
| D5 | identity definition | `id` = logical (over id_inputs, stable) + `content_hash` = Merkle **vs** id = Merkle root | P0 |
| D6 | language-binding priority | pyo3 first → C-ABI → WASM-reader | P7 |
| D7 | encryption-at-rest | non-goal (storage SSE/dm-crypt) **vs** per-block envelope | P1 |

## Coverage check (every FEATURE-MATRIX area → a phase; no orphans)
A core→P1 · B codec→✓done · C correctness→P0(S13✓/S15) · D perf→P3+P4(SLA gates) · E write→P3 ·
F integrity/FAIR→P1(FAIR fields)+P6(signing/WORM/exports) · G layout→P0/P2/P4, ingest→P5, read→P2 ·
H bindings→P7 · release gates→P4(v0.1)…P8(v1.0). **No feature is unslotted.**

## Effort summary
P0–P4 (→ **v0.1**, format frozen + single-impl): **~10–12 wk.**  v0.1→**v0.3** (+ingest+integrity):
**+7 wk.**  →**v0.5** (+Python+ops): **+4 wk.**  →**v1.0** (2nd impl + freeze): stabilization window.
