# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0-alpha.1](https://github.com/vig-os/tessera/releases/tag/tessera-core-v0.1.0-alpha.1) - 2026-09-23

### Added

- *(io)* close the array dtype envelope — i1/u1/b1/f2 via checked transparent widening ([#418](https://github.com/vig-os/tessera/pull/418)) ([#420](https://github.com/vig-os/tessera/pull/420))
- *(core+ingest)* land ADR-0058 generation-provenance on dev (supersedes #346) ([#415](https://github.com/vig-os/tessera/pull/415))
- *(io)* nullable table columns — a validity mask, not a sentinel ([#360](https://github.com/vig-os/tessera/pull/360))
- *(referencing)* first-class Interp + IcdfSampler evaluator descriptors ([#353](https://github.com/vig-os/tessera/pull/353))
- *(referencing)* add log-spaced axis transform for calibration LUTs ([#350](https://github.com/vig-os/tessera/pull/350))
- *(core)* table Column carries unit/description/short_name/scale ([#307](https://github.com/vig-os/tessera/pull/307)) ([#312](https://github.com/vig-os/tessera/pull/312))
- *(core)* collection level schemas — terminology as registered/versioned data (ADR-0050, #294) ([#315](https://github.com/vig-os/tessera/pull/315))
- *(core)* recursive-collection mechanism — MemberKind + domain-separated leaf + level tag (ADR-0049 Part 1) ([#306](https://github.com/vig-os/tessera/pull/306))
- *(ingest)* port curated DICOM tags to recon schema fields + full header to extra/ (PHI-scrubbed on de-id)
- *(provenance)* source-file integrity hashes + sealed producer stamp
- *(core)* embed the product schema in every sealed manifest (obligatory, self-describing)
- PHI hygiene (--source-label + dicom-series --deidentify) + sensitivity tier (ADR-0040 §1) ([#242](https://github.com/vig-os/tessera/pull/242))
- *(schema)* recommended-field severity (warn tier) + generic ingest --meta ([#233](https://github.com/vig-os/tessera/pull/233))
- *(blob)* bounded-memory streaming ingest + extract (closes #231) ([#232](https://github.com/vig-os/tessera/pull/232))
- opaque blob/"junk" block — bit-faithful preservation of un-parsed vendor raw ([#229](https://github.com/vig-os/tessera/pull/229)) ([#230](https://github.com/vig-os/tessera/pull/230))
- *(signing)* bind signed_at + key_format; ssh-ed25519 loader; usage tests-as-docs
- *(cli)* keygen + trust store + verify-sig defaults to it (ADR-0037 alpha tier)
- *(io,cli)* commit --add-block / --remove-block — compose encoded blocks (ADR-0036)
- *(io,cli)* evolve + tessera init/import/commit/log — CoW versioning verbs (ADR-0036)
- *(core,io)* #223 / ADR-0033 — content-addressed collection model + 3 projections
- *(core)* schema-complete DataCite (mandatory creators) + flip RO-Crate/DataCite row ✓
- *(cli)* tessera sign / verify-sig verbs (S16 end-to-end CLI)
- *(io)* sign/verify a .tsra container via a .sig.json sidecar (S16 end-to-end)
- *(core)* product-level sign/verify (sign_manifest/verify_manifest)
- *(core)* detached signatures over manifest_hash (S16) — ed25519 backend + envelope
- *(core)* ADR-0032 §gate — lookup-descriptor length must match axis extent
- *(core)* post-Accept validation gates (ADR-0030 non-degenerate, ADR-0032 axis-referencing)
- ADR-0030 §5 — deformable-warp apply (deformation-field point resolve)
- *(core)* ADR-0030 §3 — OME-Zarr multiscales export
- *(core)* ADR-0028 §2 — append-only consistency proofs
- *(core)* ADR-0028 §5 — the fused {hash,stats} streaming fold
- *(core)* ADR-0032 — close the 3 doc-normative items (frame §4 · SPEC · time split)
- *(core)* ADR-0032 — pin unit vocabulary + exercise decay-correction (3rd-audit)
- *(core)* ADR-0032 — close the 3 §Status-note Accepted preconditions
- *(core)* ADR-0032 — close re-audit findings (tick-time, presence, §7 e2e)
- *(core)* ADR-0032 — wire Referenced into ArraySpec + manifest (last gap)
- *(core)* ADR-0032 — close 2 audit gaps (time-axis instances + vocab escape)
- *(adr)* flip ADR-0029 Proposed→Accepted (as-built, fresh-audit-verified)
- *(core)* ADR-0032 unified referenced-coordinate descriptor
- *(core)* WorldFrame::at_offset — crop/ROI geometry (ADR-0030)
- *(core)* ArraySpec::from_physical — inverse affine_1d rescale for ingest (ADR-0032)
- *(core)* ArraySpec::to_physical — affine_1d intensity rescale (ADR-0032 §2/§5)
- *(core)* variance/std_dev via sum_sq monoid — proves §3 extensible-stats factory
- *(core)* ChunkStats::mean — derived stat over sum/count (ADR-0028 §3 factory)
- *(core)* deformation_field schema (ADR-0030 §5 — audit gap)
- *(core)* roi schema accepts representation-by-nature (ADR-0029 §4)
- *(core)* WorldFrame::at_level — derived per-level transforms (ADR-0030 §3)
- *(core)* ChunkIndex::chunk_proof — per-chunk confirmation (ADR-0028 §3+§6)
- *(core)* chunk-index aggregate stat-pyramid (ADR-0028 §3/§7 multiscale overview)
- *(core)* MMR inclusion proofs (ADR-0028 §6)
- *(core)* ADR-0030 §1/§2/§6 — world_frame spatial referencing on ArraySpec
- *(core)* ADR-0029 §5 schemas — dynamic_pet, diffusion_mri, multicontrast_mri
- emit chunk-index as an additive companion manifest block (ADR-0028 §3)
- *(core)* ChunkIndex::to_bytes/from_bytes — deterministic block-payload serialization
- *(core)* chunk-index core — {hash, stats} monoids + sub-block MMR + pruning (ADR-0028 §3)
- *(core)* [**breaking**] MMR content_hash — recursive Merkle Mountain Range root (ADR-0028 §1-2)
- *(core)* FAIR discovery exports — RO-Crate + DataCite from the manifest (P6, #209)
- *(core)* provenance chain-verify — walk the sources DAG, verify edge seals (P6, #209)
- *(core)* incremental hash-on-write Merkle accumulator (P3/S17, #203)
- *(io)* real array payloads in conformance corpus; pcodec default (P3, #203)
- *(io)* real array backend — zarrs+pcodec encode/decode (P3, #203)
- *(io)* P2 — .tsra container reader/writer, read-path-first ([#202](https://github.com/vig-os/tessera/pull/202))
- *(core)* P1 — fd5 field conventions + product-schema registry (#199, #200)
- *(core)* P0 ADR-020 — canonical JCS encoding, identity model, manifest_hash seal

### Fixed

- *(verify)* stream block payloads + typed, located integrity errors ([#268](https://github.com/vig-os/tessera/pull/268)) ([#340](https://github.com/vig-os/tessera/pull/340))
- *(cli)* collection verify/ls resolve members by the sanitized name ([#323](https://github.com/vig-os/tessera/pull/323)) ([#339](https://github.com/vig-os/tessera/pull/339))
- *(core)* decouple sealed producer from the crate version (ADR-0052 C→A) ([#336](https://github.com/vig-os/tessera/pull/336))
- *(export)* PHI-safe + valid JSON-LD FAIR records — opaque source URNs, not raw paths ([#267](https://github.com/vig-os/tessera/pull/267))
- *(guardrails)* reword recon curated-tag comment that tripped no-commented-code (#255 follow-up)
- *(signing)* sign the envelope + carry the sidecar through push/pull; ADR-0037

### Other

- ADR-0056 §6a: the decoder identity is a recipe fact — sealed provenance bag + build-honest triple ([#403](https://github.com/vig-os/tessera/pull/403)) ([#405](https://github.com/vig-os/tessera/pull/405))
- *(integrity)* pin everyday tamper/trust cases + the L1/L2 verify ladder ([#369](https://github.com/vig-os/tessera/pull/369))
- *(core)* runnable doctest for ome_zarr_multiscales (ADR-0030 §3 export)
- *(core)* runnable doctest for consistency_proof (§2 append-only proof)
- *(core)* ADR-0028 §2 — manifest-level consistency-proof fixture
- *(core)* runnable doctest for MerkleStatsAccumulator (§5 fold)
- *(schema)* ADR-0029 §5 trait-set — define imaging modality field once
- *(core)* incremental chunk-index == batch (ADR-0028 §5 streaming property)
- *(adr)* flip ADR-0030 (spatial referencing) → Accepted (as-built) — 2nd flip
- *(ci)* gate doctests (docs-as-tests layer) + add core API doctests
- adopt full nix-flake-check mode; rename file ext to .tsra; supersede fd5
- *(tessera)* dtype allowlist (int16 recommended, not required) + table-flavor design
- *(tessera)* P0 integrity guard tests + fix seal/timestamp bugs
- *(tessera)* scaffold core + RFC + engine selection + test plan
