# ADR-0046 — Time model: RFC-3339 wall-clock in the seal, elapsed-quantity + named epoch for axes, integer ticks for events

Status: **Proposed** (2026-07-01) · Tracks `#220` · Ratifies + pins the time-half of the
`(transform, unit, frame)` primitive from ADR-0032 (as-built) · Relates to ADR-0022 (manifest —
`product`/`name`/`timestamp`), ADR-0028 (per-level referencing derivation), ADR-0029 (dynamic PET
frame-timing table), ADR-0035 (declarative ingest), ADR-0042 (aux `ingested_at`), ADR-0045 (units
half of the same descriptor).

## Context

Three kinds of "time" show up in a Tessera product and they answer different questions:

- **The product's acquisition/logical timestamp** — "when was this scan taken?" A single wall-clock
  instant, part of the identity-forming key `(product, name, timestamp)` (ADR-0022). Human-readable
  and audit-relevant.
- **A time axis on an array** — dynamic PET frame mid-times, gated-cardiac phases, a 4-D
  perfusion series. Regular or irregular; measured from an *epoch*.
- **Event timestamps in a table** — listmode PET coincidences, ECG samples, TOF measurements.
  High-rate, must be lossless, must be compact.

There is also a fourth, deliberately kept **outside** the seal: **when the file was ingested**
(the wall-clock at which the writer ran). That lives in `aux/provenance.json` (ADR-0042) so
re-ingest stays byte-identical (writer-determinism, FEATURE-MATRIX §C).

#220 asks: which representation for which kind, and how do they compose?

The concrete gap the review surfaced: the listmode product schema declares an `ms` column but
carries **no described semantics** for it — is it ms-since-unix, ms-since-acquisition-start,
ms-since-injection, or plain elapsed ms since the first event? The read is ambiguous. This ADR
pins the answer.

## Decision

### 1. Three roles, three representations — kept distinct

| Role | Representation | Where it lives | Identity? |
|---|---|---|---|
| Product acquisition time | RFC-3339 UTC string | `Manifest.timestamp` (sealed) | **Yes** — part of `id` |
| Ingest wall-clock | RFC-3339 UTC string | `aux/provenance.json` (ADR-0042) | **No** — outside the seal |
| Time axis on an array | `affine_1d` or `lookup` transform, `unit="s"`, `frame="epoch:<instant>"` | `ArraySpec.axis_referencing` | Yes |
| Event timestamps in a table | integer ticks + `time_ticks(period_s, start_s)` scale, `unit="s"`, `frame="epoch:<instant>"` | Column + `ArraySpec` on the table | Yes |

Three rules make this coherent:

- **Absolute wall-clock instants** (acquisition, ingest, PET decay-correction reference) are
  **RFC-3339 UTC strings**. Subject to leap seconds — fine for stamps, never for intervals. UTC is
  normalised at seal by `identity::normalize_timestamp` so the digest is stable across
  producer-local zones.
- **Elapsed / relative time** (durations, frame offsets, event times) is a **quantity on a
  monotonic scale** — a `Referenced` with `transform ∈ {affine_1d, lookup}`, `unit = "s"`, and
  `frame = "epoch:<instant>"`. This is ADR-0032's `(transform, unit, frame)` primitive with rank
  and regularity chosen by the axis.
- **An absolute instant = elapsed quantity + its epoch.** There is no third representation for
  "when did this coincidence happen in UTC?" — you either want the elapsed offset (compute) or you
  compose it with `epoch:<instant>` (which itself resolves to a UTC RFC-3339 string in metadata).

### 2. High-rate event timestamps = **integer ticks + a scale**, never floats

Listmode PET emits coincidences at ps-to-ms cadence for hours. The seal-native representation is:

- **Storage:** an integer column (`u64` for ps ticks, `u32` for ms) — lossless, dense, cheap.
- **Semantics:** `Referenced::time_ticks(period_s, start_s)`, i.e. an `affine_1d` with
  `scale = period_s`, `offset = start_s`, `unit = "s"`, `frame = "epoch:<instant>"`. `physical_s
  = tick · period_s + start_s`.
- **Epoch:** made explicit via `Referenced::with_epoch(instant)` → `"epoch:acquisition_start"`,
  `"epoch:injection"`, `"epoch:unix"`.

**This closes the review's "listmode `ms` column has no described semantics" gap.** The listmode
product schema's `t_ms` column is declared with an accompanying `Referenced::time_ticks(1e-3, 0.0)
.with_epoch("acquisition_start")` — meaning "integer ms since `acquisition_start`, canonical
epoch, `unit = s`". No ambiguity; parseable by any consumer that speaks ADR-0032.

Floats are rejected for event times: they are lossy at ps cadence (a `f64` at t=1 hour has
resolution ~200 ps, worse than the scanner), non-deterministic across accumulators, and defeat
Vortex integer compression. Integer-ticks-plus-scale is both compact and exact.

### 3. Time axes on arrays

- **Regular** (uniform frames) → `Referenced::time_regular(start_s, step_s)`. E.g. a dynamic PET
  volume `[t, z, y, x]` with `axis_referencing[0] = time_regular(0.0, 30.0).with_epoch("acquisition_start")`.
- **Irregular** (non-uniform frame mid-times — the classic clinical PET protocol
  `[15, 15, 15, 30, 30, 60, 60, 180, 180, 300, 300]`s) → `Referenced::time_irregular(mid_times_s)`
  with `frame = "epoch:acquisition_start"`. Store the per-frame times as data
  (store-don't-compute); do not re-derive from a schedule at read time.
- **Dynamic PET frame-timing table** (ADR-0029) records `(mid, duration)` per frame, with the same
  epoch semantics on both columns. This is a *table alongside* the array (composition, ADR-0029),
  not baked into the array itself.

### 4. PET decay-correction reference = a named instant in metadata

The PET `decay_correction_reference` is a **named instant** (a `frame` suffix on the `epoch`
bucket, e.g. `epoch:injection`), *and* is also recorded as a top-level metadata field on the
`dynamic_pet` schema so a reader has it without parsing frame strings. The redundancy is
deliberate: the descriptor stays composable and the schema stays skimmable.

### 5. Monotonic for intervals; UTC only for stamps

**Never subtract two UTC RFC-3339 timestamps to get a duration** — leap seconds and timezone
transitions make this wrong. Durations, offsets, and event times are the elapsed-quantity form
(§1) precisely so intervals are always over a monotonic scale. UTC wall-clock exists for humans
(audit, provenance) and for the `(product, name, timestamp)` identity key; anything computed rides
the quantity.

### 6. Wall-clock lives outside the seal by construction (ADR-0042)

- **`Manifest.timestamp`** is the **acquisition / logical** time of what the product represents —
  deterministic per source, hashed, in the seal. Same input → same manifest.
- **`aux/provenance.json:ingested_at`** is the **ingest** wall-clock — fresh every time the writer
  runs, in the `aux/` sidecar (`.gitignore` for the seal, ADR-0042). Same input → same seal
  regardless of when re-ingest happens.

The two never confuse each other because they live in structurally different places.

## Consequences

- **One conceptual split — wall-clock instants vs elapsed quantities** — implemented consistently
  from listmode ticks up to product identity.
- **No new time primitive.** Time is the temporal instance of ADR-0032's `(transform, unit, frame)`;
  `time_regular` / `time_irregular` / `time_ticks` are pre-built specialisations.
- **Deterministic** — RFC-3339 canonicalised to UTC at seal (`identity::normalize_timestamp`);
  ticks stored as integers with an `f64` scale; quantities under `manifest_hash`.
- **Interoperable** — ISO 8601 / RFC-3339 for stamps (universal), UCUM `s` for quantities
  (medical-standard), `epoch:<instant>` string for the frame (round-trippable to a UTC instant
  when needed).
- **No leap-second bug in intervals** — durations are always on a monotonic elapsed scale.
- **Readers of listmode never have to guess** — the `t_ms` column's semantics are declared on the
  block via `time_ticks(1e-3, start_s).with_epoch("acquisition_start")`.

### What's built vs still a gap

- **Built:**
  - `Manifest.timestamp` (RFC-3339, normalised UTC at seal, ADR-0022).
  - `Referenced::time_regular(start_s, step_s)`, `Referenced::time_irregular(mid_times_s)`,
    `Referenced::time_ticks(period_s, start_s)` (all `unit="s"`, `frame="epoch"`).
  - `Referenced::with_epoch(instant)` refining `epoch` → `epoch:<instant>`.
  - `CANONICAL_FRAMES` includes `"epoch"` + suffix rule (`epoch:`, `baseline:`, `atlas:`).
  - `aux/provenance.json:ingested_at` (ADR-0042) carries ingest wall-clock outside the seal.
  - Dynamic PET frame-timing table (ADR-0029 §5) declared.
- **Gap: listmode `t_ms` semantics not yet declared on the product schema.** The column exists but
  no `Referenced::time_ticks(...)` is attached in the schema registry. Add the descriptor on the
  listmode table's ArraySpec so `--meta coded`-style ingest lints (ADR-0045 gap) can flag missing
  epoch declarations. This is the concrete #220 fix.
- **Gap: no cross-block epoch coherence check.** A study collection (ADR-0033) may mix arrays with
  `epoch:acquisition_start` and tables with `epoch:injection`; nothing verifies these resolve
  against a shared UTC anchor. Land as a `verify --strict-epoch` lint when the trust-store finding
  from #263 also demands wider verify surfaces.
- **Gap: no UTC anchor on the epoch suffix.** `epoch:acquisition_start` names *which* instant; the
  UTC value of that instant lives in `Manifest.timestamp` by convention, but this convention is not
  yet asserted by the ADR-0032 vocabulary. Pin it: `epoch:acquisition_start` resolves to
  `Manifest.timestamp`; `epoch:injection` requires a schema field (`radiopharmaceutical.injection_time`
  on PET); `epoch:unix` is the fixed instant `1970-01-01T00:00:00Z`.

## Non-goals

- **Not** a general date/time library — no timezone database, no calendar arithmetic, no ISO 8601
  duration parser. Downstream consumers use their language's stdlib.
- **Not** a leap-second-aware interval type. Elapsed time is a monotonic real; if the underlying
  physics needed leap-second correction, that would be baked into the input and stored as data.
- **Not** floats for event times, ever. Integer ticks + scale.
- **Not** a re-encoding of `Manifest.timestamp` — RFC-3339 UTC stays as the identity-key
  representation; ADR-0032 gives an *additional* elapsed view via `epoch:<instant>`.
- **Not** a decision about the *value* of every epoch — canonical epochs (`unix`,
  `acquisition_start`, `injection`) are the pinned set; the schema names any product-specific ones.

## References

- Tracks #220; ratifies the time half of ADR-0032.
- ADR-0022 (`Manifest.timestamp`), ADR-0025 (ingest — RFC-3339 normalisation),
  ADR-0028 (per-level referencing derivation, epoch preservation), ADR-0029 (dynamic PET frame
  timing), ADR-0032 (`(transform, unit, frame)` primitive + `time_regular`/`time_irregular`/
  `time_ticks`), ADR-0035 (declarative ingest), ADR-0042 (`aux/provenance.json:ingested_at`),
  ADR-0045 (units half of the same descriptor).
- Code: `tessera_core::manifest::Manifest.timestamp`, `tessera_core::identity::normalize_timestamp`,
  `tessera_core::referencing::{Referenced::time_regular, time_irregular, time_ticks, with_epoch,
  CANONICAL_FRAMES, frame_is_canonical}`, `tessera_io::provenance::stamp_ingest_provenance`.
