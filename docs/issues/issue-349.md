---
type: issue
state: closed
created: 2026-07-30T12:40:49Z
updated: 2026-09-28T17:55:14Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/349
comments: 3
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:46.183Z
---

# [Issue 349]: [ADR-0032: log-spaced axis transform + interp/ICDF-sampler descriptor for physics-sampling calibration tables](https://github.com/vig-os/tessera/issues/349)

## Summary

`fd5` nails the **FAIR source-of-truth** side (immutable, self-describing, provenance-linked HDF5, with human-oriented derived dumps: TOML / YAML / RO-Crate). What it doesn't yet define is the **compute-access side**: how a `calibration`/`sim` product is consumed by a hot loop that needs **O(1) random-access gathers** — a CPU transport kernel, a CUDA kernel, or an FPGA fabric.

HDF5 is the right *canonical* container but it is not what a GPU kernel or FPGA DMA consumes in the inner loop (host-side library, chunked/compressed, no zero-copy device mmap, per-lookup API overhead). So there's a gap between "the FAIR product" and "the array the kernel indexes at 300 M gathers/s."

Proposal: define a **compute-oriented derived representation** — exactly analogous to the existing human-oriented dumps, and just as regenerable from the canonical HDF5 — plus the schema that makes it self-describing.

This comes out of the PPU / strata Monte-Carlo work (measured this week on an RTX 5090: a table-driven MC transport kernel at ~305 M events/s; the physics is entirely `calibration`-archetype lookup tables — σ curves, 2D inverse-CDF samplers, Woodcock majorant grids — and the output is `sim`/`listmode`). Those tables *want* to live in `fd5`; the kernels *need* a flat emission.

## The two derived views (compute-side)

A single generic reader/emitter (one Rust crate → per-OS binaries + C-ABI + PyO3 + wasm) reads an `fd5` product and emits, on demand:

1. **Arrow (zero-copy)** — for query/analysis/scoring consumers (DataFusion coincidence sorting, energy/timing windows, Python inspection). Free from the HDF5 via Arrow.
2. **Flat ranged-access matrices** — for compute kernels: dense, **uncompressed**, contiguous typed arrays + a compact access-law header, uploaded to CPU slice / GPU VRAM / FPGA SRAM. This is the "device ABI."

Key point mirroring fd5's philosophy: **HDF5 stays the single source of truth; both views are derived and regenerable** — the compute view is just the machine-facing counterpart of the TOML/YAML/RO-Crate human-facing views. Nothing new is canonical.

## What makes the access pattern *correct by construction*

The flat emission must carry its **access law** so the consumer's gather is correct from the recipe, not hardcoded. For the `calibration`/lookup-table archetype, extend the embedded schema to declare, per table:

- **axis transform** per dimension: `linear{lo,step}` | `log{ln_lo,ln_step}` | `unit` (for ICDF u-axis)
- **interpolation**: `linear` | `log-log` | `nearest`
- **dtype / quantization**: `f64|f32|f16|int16|int8` — canonical stays f32/f64; the *emission* can be quantized per consumer (e.g. int8 tables, <0.1% error for smooth cross-sections) **without touching the source**
- shape / row-major stride

Given that header + the flat array, the consumer computes index + interpolates exactly as declared. For link-compatible consumers (CPU VM via the crate, Python via PyO3) the reader *is* the gather. For GPU/FPGA (can't link the reader) the gather is a parallel implementation of the same access law, validated by a statistical (Tier-B/TOST) parity gate. For the hot path, the header specializes the gather at **load/build time** (compile-time-known dims/interp → no per-lookup branching).

## Packaging / UX (self-describing, not self-executing)

- The pack is **data**, never executable-by-default (a data file that runs code is a security anti-pattern, and cross-OS self-exec polyglots are fragile). The **reader** is the executable.
- For a drop-anywhere bundle: a directory (or mountable/extractable archive) with `data/` (the fd5 product) + `bin/<os-arch>` readers (Rust cross-compiles trivially; `memmap2` abstracts mmap; format is fixed-LE so no per-OS byte logic) + a `tsra.wasm` universal fallback (any OS w/ a wasm runtime, and the browser) + a launcher + the manifest. Reader footprint ~1-2 MB vs MB-GB of data → bundling all OSes is rounding error.
- UX: `tsra view product.fd5` / `tsra emit-flat product.fd5 --table compton --dtype int16`, or `cd product.tsra && ./view`.
- **Viewer**: `egui`/`eframe` (Rust) compiles to native *and* wasm from one codebase — cross-OS inspector (σ/ICDF heatmaps, geometry slices) with **zero JS toolchain**, reusing the existing `strata-viz` stack. Reserve an Astro/Vite web viewer only if a publicly-embeddable inspector becomes a real requirement.

## Open questions (for discussion)

1. **First-class or ephemeral?** Is the flat compute emission a *named, hashed, provenance-linked* derived representation (regenerable, like the other dumps) or an ephemeral cache? Leaning: regenerable-derived, hash pinned to the source product, so integrity/provenance carry over.
2. **HDF5 vs random-access alternatives for the source.** HDF5 has decisive FAIR/tooling advantages (open, `h5dump`, SWMR, universal support) and should almost certainly stay canonical. Worth *noting* (not adopting) that HDF5's weakness is exactly random-access-in-kernel — which is why the flat emission exists; formats like Vortex/Lance target that natively. Decision: HDF5-canonical + flat-emission (recommended) vs re-basing. I'd keep HDF5 and treat the flat pack as the derived compute view.
3. **Access-law schema location.** Where in the `calibration`/`sim` schema do the axis-transform/interp/dtype attributes live so an `h5dump -A` still reads as a complete self-describing manifest?
4. **dtype/quantization policy.** Canonical exact (f32/f64) + per-consumer quantized emission — how to record which quantization a given emission used (provenance), and the parity tolerance that gates it.
5. **The reader as the one access core.** Confirm the model: *one* Rust reader crate → CLI + C-ABI + PyO3 + wasm + egui viewer, driving all frontends; nothing reimplements access in another language (GPU/FPGA gathers are the exception, parity-gated).

## Why this belongs in tessera

The physics-table packs and MC sim output are `calibration`/`sim`/`listmode` products already. Giving fd5 a compute-access emission makes it the canonical home for *both* the archival FAIR product *and* the thing the kernel eats — with the same self-describing/regenerable discipline. It also generalizes cleanly: any `calibration`/lookup-table product in any domain gets a correct, self-describing, GPU/FPGA-ready gather for free.

---
*Context: filed from the strata/PPU Monte-Carlo hardware track (measured GPU/CPU results this week; the "spk" table-pack was a bespoke stand-in for exactly this compute-access layer — this issue proposes folding it into fd5 as a derived representation rather than a competing format).*

---

# [Comment #1]() by [gerchowl]()

_Posted on July 30, 2026 at 12:45 PM_

## Reconciliation — this was filed against the stale `main`/fd5 white-paper; the `dev` design already settles most of it (better)

Apologies — I wrote the above from `main`'s `white-paper.md` (fd5-on-HDF5). After reading the `dev` RFC (`docs/rfc-tessera.md`) + the ADR set, **most of what I proposed is already decided, with real-data benchmarks I didn't have.** Retracting the redundant parts and narrowing to what's actually additive.

### Already settled on `dev` (I retract these — they're done, and better than my sketch)

| my proposal | already in `dev` |
|---|---|
| "don't assume HDF5" | RFC explicitly **supersedes the HDF5 substrate**; manifest + shape-dispatched blocks |
| "self-contained pack bundle" | **`.tsra`** = sealed STORED zip64, central-dir index → cloud range-reads; ADR-0042 aux members; ADR-0037 embedded ed25519 |
| "Vortex host store → Arrow/flat emission" | RFC §0.3-4: **tables → Vortex** (7-42× random `take`, zero-copy Arrow→DuckDB); **arrays → Zarr v3 + 64³ pcodec** — shape-dispatch, benchmarked on DUPLET |
| "access-law recipe (axis transform, dtype)" | **ADR-0032** `referencing = {transform, unit, frame}` with `identity`/`affine_1d`/`affine_nd`/`lookup` + `apply_scalar`/`invert_scalar` — this *is* the access-law, generalized |
| "wasm reader" | ADR-0034 runtime-parallelism-wasm |
| "quantization / native dtype" | RFC §2.4 store-at-native-precision, all dtypes |
| "sign, not-executable-by-default" | ADR-0037 embedded signature envelope; store-don't-compute throughout |
| "sim/listmode home" | ADR-0051 listmode-hdf5-decomposition |

### What is NOT a tessera concern — I over-reached, and `dev`'s own principle says so

The **flat device-ABI for the GPU/FPGA hot loop**, **per-consumer quantized emission**, and the **parity-gate** are all **consumer** decode/discipline, not format features. Tessera's store-don't-compute stance (ADR-0031/0032: "stores the code and never implements a units engine") is exactly right here: tessera stores native bytes + the `referencing` recipe; the **PPU/strata consumer** decodes Zarr+pcodec / Vortex → a flat dense buffer → VRAM/SRAM, quantizes per its own bandwidth needs, and parity-gates its own kernels. That whole layer lives in the consumer reading tessera — **not in tessera.** So I withdraw it as an ask here; it belongs in the strata/PPU `spk`/reader (which is what `.spk` was — a bespoke stand-in that should just *become* "a consumer of tessera arrays/tables").

### The only two things that might be genuinely additive (both small, both format-level)

Both concern **physics-sampling calibration tables** (cross-sections, 2D inverse-CDF samplers) — the `calibration` archetype used as a *sampling* source, not just stored values:

1. **A log-spaced axis transform is missing from ADR-0032's taxonomy.** The taxonomy is `identity` / `affine_1d` (`scale·stored + offset`, linear) / `affine_nd` / `lookup`. Essentially *every* cross-section / ICDF table is on a **log-spaced energy grid** — physical = `exp(a·index + b)` — which `affine_1d` can't express. `lookup` can represent it, but loses the analytic **O(1)** index (`i = (ln E − ln_lo)/ln_step`) and exact invert that log-spacing exists to provide (you'd binary-search instead). Proposal: a `log_affine` transform (or an `affine_1d { space: log }` flag) so log-spaced calibration axes keep analytic O(1) `apply`/`invert`. Small, concrete, and reuses the existing `apply_scalar`/`invert_scalar` machinery.

2. **Interpolation-kind + "this value-axis is an inverse-CDF" for lookup tables that drive stochastic sampling.** ADR-0032 references axis *coordinates*; a physics ICDF is `F⁻¹(E,u)` that a consumer *samples* (draw `u` → value) and *interpolates* (log-log between grid points). The **interp kind** (linear vs log-log) and the fact that a value-axis is a **sampler** (inverse-CDF) are metadata a `calibration`/LUT product should carry so *any* consumer samples it identically. Question for you: does that fit `value_referencing` (+ an interp attribute), or is it a `sampling`/`lut` descriptor on the `calibration` product schema? This is the one genuinely PPU/MC-specific need.

### Net

The issue collapses from "add a compute-access layer" (already built) to **two small ADR-0032 / `calibration`-schema questions** (log-spaced axis transform; interp/ICDF-sampler descriptor). Everything else is either done on `dev` or correctly consumer-side. If useful, I can turn (1) and (2) into a focused PR against ADR-0032 + the `calibration` schema; otherwise close this and I'll fold the consumer layer into strata/PPU as "a tessera reader."


---

# [Comment #2]() by [gerchowl]()

_Posted on July 30, 2026 at 01:23 PM_

## Addendum — two things my reconciliation dropped too hastily (the "who ships the reader" boundary)

The reconciliation above moved the whole consumer layer out of scope. On reflection, two parts of that belong *back in* — they're about **who owns the reference reader**, which is squarely a format-project concern, and both make a product *more* FAIR, not less. (The runtime GPU/FPGA **domain kernel** stays consumer-side — that part of the retraction stands.)

### 3. A FAIR product MAY carry its reference reader as `aux/` members (accessibility-over-time + interpretation-provenance)

`store-don't-compute` says the format doesn't *run* code — it does **not** say the archive can't *carry* its own decoder. Bundling a pinned, hashed reference reader with the data is a real FAIR win on **A** and **R**:

- **Accessible-over-time:** the product stays openable without depending on external tooling that may bit-rot. This is the RFC's *own* pcodec-maturity hedge (keep zstd as archival fallback) applied one level up — carry the reader so the data outlives the codec/library ecosystem.
- **Reusable / unambiguous:** the canonical code that reads the pack *correctly* is itself provenance — it pins the semantics, so there's no reverse-engineering interpretation in 15 years.

And **ADR-0042 `aux/` is exactly the mechanism** — carry the reader as `aux/reader/` (source + a sandboxed `wasm` build + optionally signed per-OS bins): *present in the one file, seal-ignored*, so it never touches `content_hash`/`manifest_hash`. The immutable identity stays data-only; `store-don't-compute` is intact (still nothing *runs*); the decoder ships with the bytes. Safe form only — source + sandboxed wasm, not auto-executing binaries. Near-free (KB–MB reader vs GB data).

### 4. tessera ships **thin per-architecture reference consumers** — so no downstream project reinvents the reader

Collapse the "N projects × M architectures" reinvention matrix to "tessera ships M thin readers." This is the format owner's natural role and it maximizes **Accessible** (one blessed, correct way to read a product on each target, not a scatter of divergent bespoke parsers).

**The boundary that keeps it store-don't-compute — stop at the flat-dense-buffer line:**

- **tessera ships (thin, reusable):** the arch-*independent* decode reader (Rust core → C-ABI + PyO3 + wasm: Zarr+pcodec / Vortex → flat dense ndarray) + thin **device-materialization** helpers ("flat buffer in VRAM" = a `cudaMemcpy` wrapper; "DMA layout for SRAM" = a descriptor).
- **the consumer provides (domain-specific, NOT tessera):** only the compute kernel — the transport loop / the ICDF *sampling* loop.

So a "GPU consumer" is really *one arch-independent decoder + a ~`cudaMemcpy` shim*, not a fat per-arch stack.

**Guardrail (the honest cost):** shipping per-arch consumers is real maintenance surface (CUDA-upload / FPGA-DMA helpers vs toolchain churn). Bound it by discipline: **thin only — decode + materialize + device-upload, never domain kernels.** If it creeps into shipping physics/domain compute, tessera becomes a domain library and maintenance explodes. Keep it at the flat-buffer line.

### How 3 + 4 compose with the two ADR-0032 asks

- Repo-for-reuse **and** in-product-for-provenance: reference readers live in the tessera repo as libs/artifacts (#4) *and* a product can carry a pinned copy as `aux/` (#3). Both, not either.
- Richer access-law → thinner project: if the **log-spaced axis transform** (#1) + **interp/ICDF-sampler descriptor** (#2) land in ADR-0032, tessera's thin reader can do the generic *interpolated* lookup itself, so a consumer writes only the sampling loop on top of a correct gather.

### The one-line boundary this whole thread was circling

**tessera owns:** the sealed FAIR product + its access-law recipe (ADR-0032) + thin reference consumers that decode → flat buffer → device, optionally carried in-product as `aux/`.
**the domain project owns:** only the compute kernel over that flat buffer.

For our side (strata/PPU) that means the bespoke `.spk` + its reader simply *dissolve* into "a tessera thin consumer + a physics kernel" — no format, no parser to invent.


---

# [Comment #3]() by [gerchowl]()

_Posted on September 28, 2026 at 05:55 PM_

Closing as **done** — verified on `origin/dev` in the 2026-09-28 backlog triage.

Evidence: ADR-0032 Accepted (docs/adr/0032-referenced-coordinates-and-quantities.md status: Accepted, as-built). Log-spaced axis transform: commit 38054c9 (#350); Interp + IcdfSampler descriptors: commit d629c6a (#353).

https://claude.ai/code/session_01XdERKMVDAwfMJSKdTytNnK

