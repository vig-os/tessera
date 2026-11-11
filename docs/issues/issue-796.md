---
type: issue
state: open
created: 2026-11-10T21:35:44Z
updated: 2026-11-11T03:36:42Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/796
comments: 4
labels: feature, priority:medium, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-11T08:50:44.653Z
---

# [Issue 796]: [feat(io): offline archival re-encode profile — denser lossless, deterministic re-seal for long-term storage](https://github.com/vig-os/tessera/issues/796)

## Description

An optional, **offline archival re-encode** of a sealed product into a smaller, still lossless and still deterministic product for long-term storage. It is not on the live or streaming ingest path, which keeps its current fast, bounded-memory settings. It is a post-seal step ("write once, re-encode once for cold storage, read many") that produces a new sealed product with provenance pointing at the original.

## Problem Statement

The live path optimises for throughput and bounded RSS, not maximum compression. Long-term archives of raw list-mode (~120 GB per GE DMI DUPLET acquisition, `docs/adr/0051-listmode-hdf5-decomposition.md`) are written once and kept for years, so extra CPU spent once on compression pays off in every later year of storage and backup.

Current evidence of headroom:
- On real DUPLET list-mode, `.tsra` is only 1.03× smaller than HDF5 shuffle+gzip-4 (`docs/book/src/why-tessera.md`).
- On the periodic fixture, HDF5 shuffle+gzip wins by ~6×, because deflate's LZ77 window exploits periodicity that value-distribution codecs (Pco/dictionary/bit-packing) can't (`tessera/bench/ecosystems/common.py`, #493/#497).
- ALP is excluded from the table compressor for cross-build determinism (ADR-0024). That's the right call for content addressing, but it costs size on float columns (#380, #412).
- The vendor comparison running for EMIM 2027 includes GE's closed, geometry-aware list-mode codec (HDF5 filter 32783, "GLEPL_MI"), which needs a binary plugin plus scanner geometry to read. An open archival profile should get as close as possible to it with generic, vendor-agnostic codecs.

## Proposed Solution

A `tessera archive` / `reencode --profile archival` command (name TBD) that reads a sealed product and writes a new sealed product, choosing per column/array the **smallest lossless** encoding from a deterministic candidate set:

1. **Source-precision types:** store integer ADC counts, ticks and crystal ids as integers rather than floats when the producer quantised them, so the float/ALP question disappears. Losslessness is checked against the source.
2. **Sort-aware time encoding:** after time sort, delta / frame-of-reference encode the near-monotonic time column.
3. **Higher Pcodec compression level** for arrays and Pco table columns, plus a zstd level/dictionary where a byte codec is used.
4. **An LZ-style candidate for periodic columns** (the case where shuffle+deflate wins ~6×), so the scheme search can pick it.
5. **ALP only behind a determinism guarantee:** re-enable it in the archival profile only when proven byte-identical across build profiles (ADR-0024 lists this as a future option).
6. **Optional value-range partitioning** (e.g. energy-band leaves per ADR-0064) where it shrinks size or speeds up the common window queries.

Invariants:
- Lossless: bit-exact decode equality to the source product, CI-gated.
- Deterministic: same input + profile + version ⇒ same bytes and content_hash.
- Provenance: the new product records `derived_from = <source content_hash>` and the profile; the source can then be deleted or tiered.

## Alternatives Considered

- Making the live path slower and denser: rejected, because the live writer's bounded RSS (147 MiB on a 294 M-row stream, ADR-0026) and throughput matter for acquisition and live coincidence building.
- External post-compression (zstd/xz over the whole `.tsra`): rejected, because it breaks chunk-level random access, partial Merkle verification and zone-map pruning.

## Additional Context

This came up while preparing EMIM 2027 abstracts (DUPLET list-mode storage) and the vendor-format benchmark on real GE / Siemens canine acquisitions (Nov 2026).

## Impact

Smaller cold-tier archives for raw list-mode retention (storage + backup cost), with the live path unaffected. Measured savings TBD by benchmark. No change to the format contract, other than a new provenance edge and profile id.

## Changelog Category

Added

---

# [Comment #1]() by [gerchowl]()

_Posted on November 10, 2026 at 09:37 PM_

Two additions from discussion (Lars, 10 Nov):

**1. Compression level as a recorded profile, also usable LIVE when the DAQ has spare cores.**
Today the levels are fixed for determinism: `ZSTD_LEVEL: i32 = 3` ("never ... a runtime knob", `tessera-io/src/array.rs`) and `PcodecCodecConfiguration::default()` (`array.rs` `pcodec()`). A higher level is still a pure function of (data, level). So determinism holds if the level is part of a named, manifest-recorded **encoding profile** (e.g. `live-default`, `live-dense`, `archival`) and not a hidden runtime setting. That gives an honest CPU-vs-size knob: a DAQ host with idle cores could write `live-dense` directly, and this issue's offline re-encode stays the maximum-density path.
- Trade-off to decide: the same logical data under two profiles gets two content hashes (dedup across profiles breaks) unless identity is (also) defined over logical values (see 2b).
- Needs a benchmark: throughput, RSS and size vs pcodec level / zstd level at 1..N workers, to see where a live profile still keeps up with realistic singles rates.

**2. ALP vs content addressing (ADR-0024).** Pinning the toolchain would make one build reproducible, but not every conforming writer: x86_64 vs aarch64, wasm/Python bindings, third-party implementations, re-encodes years later. Options:
- (a) make ALP's exponent/parameter search exact (integer or explicitly non-fused arithmetic), so the encoder decision is platform-independent. Upstream change or wrapper.
- (b) record the chosen ALP parameters per chunk and/or define the identity hash over decoded logical values, with the byte-level seal kept for integrity only. Question for the architect: which of `id` / `content_hash` / `manifest_hash` currently covers encoded bytes vs logical values?
- (c) for the archival profile only: a pinned, recorded encoder binary with explicit provenance (`encoder=<name>@<version>+<toolchain>`), accepting that re-encoding is a new provenance step.


---

# [Comment #2]() by [gerchowl]()

_Posted on November 10, 2026 at 09:39 PM_

Follow-up on option 2(a), a portable ALP:
- ALP decode is lossless regardless of the chosen parameters. Only the encoder's parameter choice (exponent/factor search) differs across builds (ADR-0024). So a portable variant only needs a deterministic choice, e.g. scoring candidates with exact integer arithmetic instead of float-rounding tricks. The format and decoder stay unchanged.
- It would live as a patch in the already-pinned Vortex fork (`gerchowl/vortex@608d8a1`, see #635), consistent with "sealed table bytes are Vortex encoder output".
- No existing cross-platform-deterministic ALP variant was found in the literature (scitadel: Afroozeh/Kuffó/Boncz, PACMMOD 2023, doi:10.1145/3626717; SIGMOD'24 reproducibility report doi:10.1145/3687998.3717057; GPU and Elf/Serf/DeXOR-style variants). Not proof of absence; worth asking upstream vortex-data/vortex and CWI.
- Acceptance: CI-gated byte identity across x86_64/aarch64 (+ wasm if supported) and dev/release profiles, encoder recorded in provenance, and a measured size/speed win on real float columns (DUPLET energies) vs the current Pco fallback.


---

# [Comment #3]() by [gerchowl]()

_Posted on November 11, 2026 at 01:35 AM_

Measured input for this profile, from the vendor-format benchmark on real GE / Siemens canine list-mode (tessera `6a3078079`). Details and fixtures are in #822.

**Hot vs cold.** The vendor files are cold-tier products compressed after acquisition (GE's geometry-aware GLEPL; Siemens' packed packets), while `.tsra` was written with live settings. On list-mode:
- GE: `.tsra` 3.90 B/event, against GE as delivered 3.36 and HDF5 shuffle + zstd-3 3.77.
- Siemens: `.tsra` 7.51 B/event, against native 8.00 and HDF5 shuffle + zstd-19 6.90.

That gap is this profile's target.

**ALP potential: 0 %.** Vortex (fork pin `gerchowl/vortex@608d8a11`, #635) ran with ALP / ALPRD allowed versus excluded.
- The data: 5 float columns of simulated PET singles (`edep_keV, time_ps, x/y/z_mm`, 3.5 M rows each, 140 MB raw).
- The BtrBlocks sample selector picks `vortex.pco` on every column in both configurations. The bytes are identical: 66,212,984 in both.
- The payload sha256 is also identical across `dev` / `release` builds and two `release` runs. That is **not** evidence that ALP is deterministic: the ALP path was never taken.
- The GE and Siemens list-mode data are integer-only, so ALP doesn't apply to them.

So item 5 (ALP behind a determinism guarantee) has no measured payoff on PET data so far. Items 1–4 (source-precision ints, sort-aware time encoding, higher Pco / zstd levels, an LZ candidate) are where the list-mode gap would have to close.


---

# [Comment #4]() by [gerchowl]()

_Posted on November 11, 2026 at 03:36 AM_

Measured follow-up in #828: a per-column codec sweep on the same fixtures, with every round trip verified. Plain Pco per column gives 3.18 B/ev on GE (`.tsra` 3.91) and 6.84 B/ev on Siemens (`.tsra` 7.51). The best codec per column gives 3.09 and 6.83; Siemens is then at its order-0 entropy. A fast fixed profile gives 3.15 / 6.89 B/ev at ≈25 / 49 M ev/s single-core codec time (indicative). Row-group size is not the cause (+0.2–0.3 %), so the gap points at scheme selection.

