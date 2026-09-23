# Tessera — end-to-end roadmap (phases · milestones · gates · critical path)

Single source for "where we're going + in what order." Pairs with `FEATURE-MATRIX.md` ("what
passes") and the RFC ("what it is"). No time/effort estimates — work is agent-executed, so the
unit that matters is **dependency order** (what unblocks what), not dev-weeks. Status: ✓/◑/○.

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
| # | Phase | Scope (tasks) | Done-gate |
|---|---|---|---|
| **P0** | De-risk & ADRs | **S13 ✓**, **S15 remainder** (cross-ver/arch + vendored-reader prototype); ADRs: D4 canonical-encoding, D5 identity, #22 versioning-DAG+container, D3 schema-id allocator, D2 sync/async, D1 fd5 repo | ADRs accepted; vendored reader decodes a pinned-version file |
| **P1** | `tessera-core` finish | #20 manifest+BlockRef schema; #19 restore fd5 conventions+fields (id_inputs · `_type`/`_version` · `_vocabulary`/`_code` · `default` · `extra/` · `sources` roles+resolve · `study` · units · axes · `fill_value` · descriptions); #21a error taxonomy; product schemas (recon/listmode/sinogram/spectrum/roi/transform/calibration/sim/device_data) + required-field tables; D7 encryption non-goal | all §C correctness + spine tests green incl canonical-hash + validation |
| **P2** | Read path **first** | #21b Reader API (`open`/range-read/block-handle/partial-product) + `object_store` backend | reads a hand-built `.tsra`; range-read a chunk |
| **P3** | `tessera-io` write engine | S5 zarrs backend; S17 streaming (fragment-append · hash-on-write · incremental Merkle · crash-recovery to watermark); S3 chunk-Merkle integrity tree; container writer; observability (`tracing` on watermarks) | acq→sealed roundtrip; crash-recovery resumes; §D perf-SLA met |
| **P4** | Conformance + CLI → **v0.1** | #21c conformance corpus + `SPEC.md`; `tessera-cli` (pack/unpack/verify/inspect); perf-SLA CI gates; S6 object-store range-read | 4 release gates green (conformance · roundtrip · SLA · determinism) |
| **P5** | Ingest → **v0.2/v0.3** | S9 DICOM (files/DICOMweb/DIMSE, PS3.15 verify, lossless tags, rescale/units, egress); then GE-HDF5 · Siemens · raw `.dat`/`.BLF` · NIfTI; S14 cross-shape query demo | lossless DICOM roundtrip + egress; golden DICOM corpus |
| **P6** | Integrity & distribution → **v0.3** | S16 signing (cosign minimal → source-rooted chain-verify); WORM/Object-Lock; OCI artifact mapping; RO-Crate/DataCite/tessera-index exports | chain-of-custody verify; OCI push/pull; WORM enforced on MinIO |
| **P7** | Bindings & ops → **v0.5** | `tessera-py` (**pyo3 wrapping the core** — same engine, not a reimpl); `tessera-wasm` (**Rust→WASM** for TS/browser readers); reference podman-compose stack (zot+MinIO+InvenioRDM+cosign); migration tooling (`schema diff/validate`); format-spec semver policy | Python/TS parity passes conformance; ref-stack smoke test |
| **P8** | Spec stabilization → **v1.0** | **independent reader** — from `SPEC.md` only, not linking the Rust (see note below); cross-version determinism; freeze `tessera-1.0`; deprecation policy | independent reader passes corpus; 12 mo zero-breaking since v0.5 |

## The v1.0 independent-reader gate (why bindings don't count)
The spec is "done" only when a **second, independent codebase reads Tessera files correctly using
`SPEC.md` alone** — no access to the `tessera-core` source. Until then "the spec" is really "whatever
the Rust happens to do," and ambiguities hide. **Bindings are not independent:** `tessera-py` (pyo3
wrap) and `tessera-wasm` (Rust→WASM) *are* the Rust engine reached from another language — they agree
by construction and validate nothing. The genuinely-independent target is a **pure-Python reader**
(stdlib `zipfile` central-dir + JSON manifest + the `pcodec` py lib + `blake3` — none derived from our
Rust) that parses → range-reads a block → decodes → verifies the Merkle root. It only needs to *read*,
so it's small. **Agent-native method:** spawn a fresh-context agent with *only* `SPEC.md` + the
conformance corpus and have it implement that reader; every gap it hits is a spec gap to fix. Cheap and
repeatable, so this gate runs continuously from P4 onward, not once at v1.0.

## Milestone gates (definition of shippable)
Per `FEATURE-MATRIX.md §H`: **shippable = ① conformance corpus · ② bit-exact roundtrip · ③ perf-SLA
(§D floors) · ④ writer-determinism** — all green on the supported matrix. v0.1 freezes the wire format.

## Open gating decisions (register — decide by the listed phase)
| id | decision | options | by |
|---|---|---|---|
| D1 | fd5 supersession | **DONE.** fd5 superseded by Tessera. Repo renamed `vig-os/fd5`→**`vig-os/tessera`** (history kept; GitHub redirects active). fd5 Python CI dropped for the `nix flake check` shim; `main` branch protection requires the `nix flake check` status check. fd5 Python app remains as legacy until removed. | ✓ |
| D2 | concurrency model | **DONE (ADR-0002, as-built):** synchronous public API; async kept dependency-internal (no tokio); object_store deferred | ✓ |
| D3 | schema-id allocation | **DONE (ADR-0003, as-built):** string product names, open-world (no numeric allocator) | ✓ |
| D4 | canonical encoding for hashing | RFC 8785 JCS-JSON **vs** deterministic CBOR | P0 |
| D5 | identity definition | `id` = logical (over id_inputs, stable) + `content_hash` = Merkle **vs** id = Merkle root | P0 |
| D6 | bindings vs validation | reach = `tessera-py` (pyo3-wrap) + `tessera-wasm` (Rust→WASM, TS); spec-validation = separate pure-Python reader (P8 gate, not a binding) | P7 |
| D7 | encryption-at-rest | non-goal (storage SSE/dm-crypt) **vs** per-block envelope | P1 |

## Coverage check (every FEATURE-MATRIX area → a phase; no orphans)
A core→P1 · B codec→✓done · C correctness→P0(S13✓/S15) · D perf→P3+P4(SLA gates) · E write→P3 ·
F integrity/FAIR→P1(FAIR fields)+P6(signing/WORM/exports) · G layout→P0/P2/P4, ingest→P5, read→P2 ·
H bindings→P7 · release gates→P4(v0.1)…P8(v1.0). **No feature is unslotted.**

## Tracking
GitHub Issues + Milestones on `vig-os/tessera` are the durable tracker. Milestones = the release gates
(**v0.1 · v0.2 · v0.3 · v0.5 · v1.0**); the old fd5 "Phase 1–5" milestones are closed as superseded.
Issues carry `priority:` + `area:` labels (no `effort:` — meaningless for agent-executed work). P0 ADRs
and Phase-1 work are filed per-task; later phases (P5–P8) as one epic issue each.
