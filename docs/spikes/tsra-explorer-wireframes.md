# Wireframes — `tessera-tui` read-only shell (Phase 1b), for sign-off before code

Status: **Wireframe spec** (2026-07-03, #286). The "feature-complete layout before the first line of TUI
code" gate. Synthesises the four independent persona mockups from the UX spike
(`tsra-explorer-ux.md`) into **one shell** — a shared chrome + a union of modes + per-profile defaults —
and traces every pane to a `tessera-explore` view-model function that already exists (Phase 1a), so the
TUI is a *thin renderer*. **No code until these are signed off.**

## The shared shell (chrome — identical in every mode/profile)

```text
┌ tessera-tui · <product>/<name> ───────────────  seal ✓ · sig ✓ alice · schema ✓ · phi ⚠ · <profile> ┐
│ NAVIGATOR  (NodeTree)          │ <MODE> › <tab/subview>                                   [context]  │
│ ▾ study.tsra                   │ ┌────────────────────────────────────────────────────────────────┐ │
│   ├─ meta            12         │ │                                                                │ │
│   ├─ schema          ✓ pet-ct  │ │                                                                │ │
│   ├─ referencing     LPS mm    │ │        (content pane — mode- and tab-specific; below)          │ │
│   ▾ blocks           4         │ │                                                                │ │
│   │  ├─ ct           512³ i16  │ │                                                                │ │
│   │  ├─ pet_suv      200³ f32  │ │                                                                │ │
│   │  └─ events       table×32  │ │                                                                │ │
│   ├─ sources         3         │ │                                                                │ │
│   └─ extra           1         │ └────────────────────────────────────────────────────────────────┘ │
├────────────────────────────────┴─────────────────────────────────────────────────────────────────────┤
│ 1 Navigate · 2 Inspect · 3 Data · 4 Verify · 5 Compare    / cmd · v verify · g hand-off · ? · q       │
└────────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

- **Top bar** = identity + the persistent **verdict strip** (seal/sig/schema/phi glyphs) + active profile.
  The auditor's "never scrolls off" requirement is met by making this chrome, not a pane.
- **Left = NavigatorNode tree** (ADR-0043 hierarchy). Selecting a node drives the content pane.
- **Right = content pane**, one active **mode** (number keys) × its **tab/subview**.
- **Footer** = mode switcher + global verbs (`/` palette, `v` verify overlay, `g` GUI hand-off, `?`, `q`).

## Modes (the union; a profile picks the default + which are pinned)

### 2 · Inspect — metadata tabs (co-equal with data; the FAIR/steward/auditor core)

```text
 INSPECT › [Integrity] Provenance  Trust  Schema  Referencing  Governance  FAIR
 ┌──────────────────────────────────────────────────────────────────────────────┐
 │ id            duplet:pet-ct:014                                                │
 │ manifest_hash blake3:7c9f…6e57   (version)                                     │
 │ sealed        yes                                                             │
 │ blocks        4   ct·pet_suv·events·raw.l64      digests ✓ 4/4 (on Verify)     │
 │ product       pet-ct            schema  ✓ conformant                           │
 └──────────────────────────────────────────────────────────────────────────────┘
```

Tabs (Tab / ←→): **Integrity · Provenance & lineage (log/diff) · Trust (signature + trust-store verdict)
· Schema conformance · Referencing (affine/units) · Governance (PHI/WORM) · FAIR record**.

### 3 · Data — the pane adapts to the selected block's kind

**Table block →** paged viewer over the cross-block logical view + a query line:

```text
 DATA › events  (logical, 1.28e9 rows)                    rows 0–1000/1.28e9 · x export
 ┌ SQL ───────────────────────────────────────────────────── F5 run · 342ms · pruned 31/32 ┐
 │ SELECT energy_kev, count(*) FROM events WHERE prompt GROUP BY 1                          │
 ├ Result ─────────────────────────────────────────────────────────────────────────────────┤
 │  energy_kev │        n                                                                    │
 │        500  │  17_223_884   ← 511 photopeak                                               │
 │        520  │  12_884_017                                                                 │
 └───────────────────────────────────────────────────────────────────────────────────────────┘
```

**Array block →** stats header + histogram + an interactive slice / MIP image:

```text
 DATA › pet_suv  200×200×402 f32   axial z=187   W/L 6.0/3.0 SUV   [physical]
 ┌ stats ──────────────────────────────┬ histogram (SUV) ──────────────────────────────────┐
 │ min 0.0  max 42.1  mean 1.8 std 3.1  │  ▁▂▄▇█▇▅▃▂▁                                        │
 │ voxels 16_080_000                    │  0    5    10   15  SUV                            │
 ├──────────────────────────────────────┴────────────────────────────────────────────────────┤
 │            ░░▒▒▓▓█ (slice / MIP via ratatui-image: sixel/kitty; ASCII/braille fallback) █▓▓▒│
 │   j/k slice · h/l window · m MIP · p project-axis · r ROI                                   │
 └──────────────────────────────────────────────────────────────────────────────────────────────┘
```

### 4 · Verify — the auditor's verdict view (read-only; report export)

```text
 VERIFY                                          offline · trust: acme-qms (fp c4a1…9e2b)
 seal ✓ · signature ✓ (ed25519-env-v1, key_id 9e2b…, signer 0000-0002-… ATTRIBUTION) ·
 trust ✓ matched acme-qms · schema ✓ · phi ⚠ 2 fields · lineage ✓ · tamper-flags: none
 [ E export signed report ]   [ deep-verify: stream all 4 blocks ]
```

### 1 · Navigate (tree focus) · 5 · Compare (A/B or prior — deferred detail to a later pass)

## Per-profile defaults (one binary, `--profile`; every mode still reachable via number keys)

| Profile | Opens in | Pinned tabs | Policy defaults |
|---|---|---|---|
| **clinician** | Data · Array (image) | Referencing, Governance | PHI render **on**; verify = glyph |
| **scientist** | Data · Table (SQL) | Referencing, Schema | PHI render aware; export-with-provenance |
| **steward** | (Queue → ) Inspect | Schema, Provenance, Governance, Trust | edit capability **on** (Phase 2 gate) |
| **auditor** | Verify | Integrity, Trust, Provenance | PHI render **off**; report `E` primary; read-only |

## Traceability — every pane renders an existing view-model function (thin renderer)

| Pane / subview | `tessera-explore` fn (Phase 1a) |
|---|---|
| Navigator tree · Inspect structural | `NodeTree` — **the one Phase-1b addition** (built here against these wireframes) |
| Array stats header | `array::array_stats` |
| Array histogram | `array::histogram` → `RecordBatch` |
| Array slice / MIP image | `array::slice_region` · `array::project_axis` (+ `ratatui-image`) |
| World/mm addressing | `referencing::world_to_index` |
| Table viewer + SQL result | `table::read_page` → `RecordBatch` (+ the `sql` feature) |
| Verify / trust / lineage verdict | **Phase-1b `ArtifactVerdict`** — the second addition (defined against the Verify wireframe) |

So Phase 1b writes **exactly two** new view-model shapes — `NodeTree` and `ArtifactVerdict` — both now
concretely specified by the wireframes above (no longer speculative), plus the ratatui rendering. All
other panes bind to functions that already exist and are tested.

## Open questions for sign-off

1. **Mode set** — is `1 Navigate · 2 Inspect · 3 Data · 4 Verify · 5 Compare` the right top-level set,
   or split Data into `3 Table · 4 Array`?
2. **Default profile** when `--profile` is omitted — auto-detect (image-capable terminal → clinician;
   else scientist), or a fixed default?
3. **Compare (mode 5)** in the v0 read-only TUI, or defer to after the four core modes land?
4. **Queue** (steward/auditor batch) — in the first TUI cut, or a follow-up once single-product is solid?
