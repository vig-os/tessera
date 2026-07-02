# Spike — `.tsra` explorer UX: four personas, one substrate (Phase 0.5)

Status: **UX design spike** (2026-07-02, #286). Four personas — **clinician · scientist · data-steward ·
auditor** — were spiked **independently and in parallel** (fresh-context agents, each championing one
persona) precisely so their specs would *not* be artificially harmonised. This doc tests the hypothesis
*"they're similar enough to ship as one"* against those independent results, and derives the
**presentation-agnostic view-model contract** that Phase 1a must satisfy (so the substrate is designed
from requirements, not guessed — the anti-thrash discipline from the explorer spike).

**Verdict up front:** **Ship as one — with two capability seams, not four apps.** The *substrate* (view-model)
is ~fully shared; the *shell* is one binary with **persona profiles**; the two things you must gate are
**the editor (write/CoW)** and **the batch/queue** — plus the clinician's rich 3-D is a **hand-off to the
GUI/wgpu tier**, not a TUI split.

## The four personas (independently spiked)

| Persona | Primary lens | Home screen | Reads/writes | Uniqueness (what no one else forces) |
|---|---|---|---|---|
| **Clinician** (nuc-med/radiologist) | **pixels** (metadata = badges) | image canvas | read | world-yoked crosshair · MPR · PET/CT fusion · W/L presets · jump-to-SUVmax · ROI→SUV · **GUI hand-off** |
| **Scientist** (physicist/analyst) | **data payload** | table/query + plots | read (+notes) | **SQL over listmode** · cross-block joins · histograms/spectra/count-rate · A/B compare + diff/Bland-Altman · **export-with-provenance → notebook** · cohort |
| **Data steward** (curator) | **the edit** | queue/collection | **write** (CoW) | **typed-form editor + live validation + staged delta + diff-preview + commit+sign + publish-gate** · PHI editor · governance consistency · provenance-edge picker · batch · trust-store admin |
| **Auditor** (compliance) | **the verdict** | verdict banner | read-only *by policy* | **persistent per-axis verdict** · **offline-forever** · trust-store as first-class object · **adversarial tamper-pattern detection** · **signed report = the deliverable** · refuse-PHI-render by default |

Each independently produced 5–8 user stories, ASCII layouts, a viewing-properties inventory, and — the key
part — a "view-model needs" section. Those four view-model sections are where the hypothesis is decided.

## The evidence: what's shared vs. divergent

**Capability × persona** (● primary · ◑ secondary · ○ rare/none):

| Capability | Clinician | Scientist | Steward | Auditor |
|---|---|---|---|---|
| Tree navigator (ADR-0043) | ◑ | ● | ● | ● |
| Metadata inspectors (integrity/meta/schema/provenance/trust/gov/referencing) | ◑ (as badges) | ◑ (correctness) | ● | ● |
| Verify pipeline (`ArtifactVerdict`) | ◑ (badge) | ◑ (glance) | ◑ (admin) | ● (verdict-strip + report) |
| Lineage `log`/`diff` graph | ◑ (prior compare) | ◑ (which recon) | ● | ● (per-hop sig) |
| Array slice / MIP / MPR / ROI-stats | ● | ● | ○ (spot-check) | ○ (gated) |
| Fusion · W/L presets · jump-to-max | ● | ○ | ○ | ○ |
| Table pager · **SQL** · cross-block join | ○ | ● | ○ | ○ |
| Histograms / plots (plotters) | ○ (ROI hist) | ● | ○ | ○ |
| A/B compare (diff volume, Bland-Altman) | ◑ (prior) | ● | ○ | ○ (dup-fraud) |
| **Editor** (typed forms → CoW commit + sign) | ○ | ◑ (notes) | ● | ✗ (forbidden) |
| **Queue / batch** over N products | ○ | ◑ (cohort query) | ● (curation) | ● (sampling) |
| Trust-store view/admin | ○ | ○ | ● (admin) | ● (view/verify) |
| Report export | ○ | ◑ (audit dump) | ○ | ● (required) |
| Export-with-provenance → Arrow/py | ○ | ● | ○ | ○ |

Two readings of this table matter:

1. **Down the "view-model needs" axis, the four specs converge.** All four independently derived compatible
   structures — `ArtifactVerdict`/`IntegrityVerdict`/`SignatureVerdict`/`TrustVerdict`/`LineageGraph`/
   `SchemaConformance`/`GovernanceView` (auditor + steward), `slice`/`project`/`roi_stats`/`histogram`/`sql`
   (clinician + scientist), `EditSession`/`MetaOp` (steward). Nobody invented a structure that contradicts
   another. **One `tessera-explore` view-model = the union of these, and it serves all four.** This is the
   load-bearing finding: the *substrate* is genuinely one thing.
2. **Across the "presentation" axis, they diverge in emphasis, not in kind.** Same widgets, different
   *default screen*, *pinned panels*, *keymap emphasis*, and *policy defaults*. That is **profile config over
   one shell**, not four products.

## Verdict: one binary, persona profiles, two gated capabilities

**Ship as one `tessera-tui`** with:

- **A shared chrome** — tree (left) · tabbed inspectors · a status/verdict line · a command palette (`:`).
- **Persona profiles** (`--profile clinician|scientist|steward|auditor`, remembered per user) that set the
  **default mode, pinned panels, keymap emphasis, and policy defaults** — *not* separate codebases. Every
  mode stays reachable from every profile via the palette; the profile only picks sensible defaults.
- **A union of modes**, each a renderer over the shared view-model:

```text
  Navigate (tree)         — all profiles
  Inspect  (metadata)     — all; pin defaults differ per profile
  Image    (slice/fusion/MIP/MPR/ROI)  — clinician ●, scientist ◑
  Data     (table/SQL/histogram/plots) — scientist ●
  Verify   (verdict/trust/lineage)     — auditor ●, all-shared
  Compare  (A/B or prior)              — scientist ●, clinician ◑
  Edit     (forms→diff→commit+sign)    — steward ● · GATED (capability)
  Queue    (batch over N products)     — steward + auditor
```

### Seam 1 — the editor is a feature-gated *capability*, off by default

The write/CoW path (steward) is heavy and distinct, and the **auditor requires provable non-mutation**
(open `O_RDONLY`, no writable session). Resolve both with one lever: **the editor is an additive capability,
compiled/enabled off by default** → a **read-only build** serves clinician/scientist/auditor; **stewards get
the write build** (or a runtime capability tied to a loaded signing key). One codebase, gated capability —
*not* an app split. This directly satisfies the auditor's "must not be able to mutate" and keeps the
read-only surface small and auditable.

### Seam 2 — batch/queue is a shared mode (steward + auditor), not clinician/scientist

Steward (curation queue) and auditor (audit sampling) both work a **list of products**; clinician and
scientist are single-product (the scientist's "cohort" is a *query*, not a curation queue). So **Queue is one
shared mode** surfaced by the steward/auditor profiles — reusing the same header-only scan (manifest +
`verify()`, no block decode) with different columns (conformance/PHI/sign for steward; per-axis verdict for
auditor).

### Not a seam — the clinician's rich 3-D is a hand-off, not a split

The clinician's own spec says the TUI is **triage + verification + quick reads**, and true 3-D / cinematic /
fluid-mouse reading belongs in a **GUI/`wgpu` desktop viewer** (Phase 3b). So the clinician's *full*
experience isn't TUI-complete by design — the TUI clinician mode is deliberately the lighter subset with a
first-class `g` hand-off. That's a tier boundary we already have, not a reason to fork the TUI.

## The derived view-model contract (the anti-thrash artifact for Phase 1a)

The **union** of the four personas' "view-model needs" — this is what `tessera-explore` must expose,
**presentation-agnostic** (consumed by TUI profiles now; serve/web later). Grouped:

**Navigation & inspection (all personas)**
- `tree(root) -> NodeTree` (ADR-0043 hierarchy; nodes carry badges: dirty/error/staged/verdict).
- Typed manifest accessors: `Meta`, `Schema`, `Referencing`, `Provenance`, `Signature`, `Governance`.

**Verify / trust / lineage (auditor ●, all-shared)** — return **structured verdicts, never strings**
- `verify_offline(reader, trust_store) -> ArtifactVerdict` (composite: integrity · signature · trust ·
  schema · governance · lineage · tamper_flags · overall; **no implicit network**).
- `deep_verify_stream(...) -> impl Iterator<BlockDigestVerdict>` (progress + bounded-mem proof).
- `walk_lineage(reader, repo_hint) -> LineageGraph` (per-hop signature, unsigned-gap detection).
- `list_trust(...) -> Vec<TrustEntry>` · `match_signature_to_trust(...)` (pure, unit-testable).

**Data — arrays (clinician + scientist)** — physical-unit/rescale-aware, world-coordinate ROIs
- `slice(block, axis, index_world|voxel, window, preset?, colormap?, pyramid_hint) -> ImageBuffer`
- `mpr(...)` · `fuse(base, over, ...) -> ImageBuffer` (view-model resamples via affine; UI never touches geometry)
- `project(block, axis, MIP|Mean|Sum|MinIP, rotate?, pyramid_hint) -> ImageBuffer`
- `roi_stats(block, RoiSpec{Box|Sphere|Mask|Threshold} in world-mm, co_registered?) -> StatsRow` (SUV/HU)
- `find_maxima(block, threshold?, top_n)` (jump-to-SUVmax) · `pyramid_level(block, budget) -> LevelId`

**Data — tables & aggregation (scientist ●)** — **one Arrow result contract**
- `logical_table(prefix) -> LogicalTable` · `table_page(...)` · `sql(query, params) -> Stream<RecordBatch>`
  (cancellable; exposes bytes-scanned/rows-pruned; prune-before-fetch on cloud).
- `histogram(block|selection, bins, physical, weights?) -> RecordBatch` (dense ndarray kernel **or**
  DataFusion by selection kind — caller sees one shape) · `density2d(...)` · `stats(...)` · `downsample(...)`.
- Compare: `paired_roi_stats([a,b], RoiSpec)` · `array_diff(a, b, resample)` · `bland_altman(a,b)`.
- `export_selection(sink: Arrow|CSV|Parquet|NPY, dst) -> {path, provenance_stub{product_id, manifest_hash,
  query|selection, tool_version, exported_at}}` (the reproducibility guarantee).

**Edit / write (steward ● — GATED)** — staged delta, never live mutation
- `EditSession{ base, delta: Vec<MetaOp>, validation: ValidationSnapshot }` with `apply/undo/redo`,
  `validate() -> ValidationSnapshot` (pure, positional findings), `preview_diff() -> DiffView` (field ops +
  block-reuse + **new manifest_hash that must equal commit's**), `commit(repo, msg, SignOpts) -> CommitOutcome`.
- `batch_commit(products, BatchDelta, repo, SignOpts) -> Stream<CommitOutcome>` · `publish(product, gates)`.
- Invariant: **never mutate sealed bytes**; commit writes a new content-addressed object + moves the ref.

**Batch / cohort (steward + auditor + scientist-cohort)**
- `scan_queue(root, filter, sort) -> Stream<QueueRow>` (header-only, streamed) · `open_cohort(paths|prefix)`
  (fans slice/roi/sql across N, prune-before-fetch, prepends `product_id`/`manifest_hash`).

**Cross-cutting update patterns**
- Slice/W-L/SQL results **stream and are cancellable** (interrupt on next keystroke); first paint on the
  coarsest pyramid level within ~200 ms even for GB volumes; `verify_offline` completes offline.
- **Per-persona policy** is config *over* the view-model, not new code: PHI-render default (auditor off /
  clinician on / scientist aware / steward spot-check), read-only vs write capability, pinned-panel set.

## Implications for the phasing

- **Phase 1a builds this union contract once** — designed from the four specs, so the TUI never asks the
  substrate for something it wasn't shaped to give (the anti-thrash guarantee). Keep it presentation-agnostic
  (serves serve/web too).
- **Phase 1b ships the shared shell + read modes + profiles** (Navigate/Inspect/Image/Data/Verify/Compare),
  read-only build. Covers clinician(triage)/scientist/auditor at v0.
- **Editor (Seam 1)** is its own gated increment (aligns with the existing Phase 2 = CoW editor) — the steward
  profile turns it on; auditor/clinician builds never link it.
- **Queue (Seam 2)** is a small shared mode addable with 1b or 2.
- **Clinician rich-3-D & the auditor's shareable hosted verifier** are the **web/`wgpu` tier** (Phase 3), not
  TUI scope.

**Bottom line for the hypothesis:** your instinct holds at the layer that matters — **one substrate, one
binary**. The personas are one product wearing four *profiles*, with the editor and batch as gated
capabilities and rich-3-D as a hand-off. Nothing here forces a fork; everything is config/capability over a
single `tessera-explore` + `tessera-tui`.
