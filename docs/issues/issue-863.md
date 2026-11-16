---
type: issue
state: open
created: 2026-11-15T09:06:00Z
updated: 2026-11-15T09:06:15Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/863
comments: 0
labels: feature
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-16T08:59:41.950Z
---

# [Issue 863]: [feat(format): schema-declared typed quality annotations (source quirks as closed kinds + counts, carried through views)](https://github.com/vig-os/tessera/issues/863)

## Need

A producer that converts vendor data into a product often meets **source quirks**: the input is valid, but it has a property a consumer should know about. Today each producer invents its own place to record these, or hides them. We'd like **one general, schema-declared facility** for typed quality annotations, so tessera stays system-agnostic while every producer reports quirks the same way.

## The real example (downstream: MorePET/visionx-rs)

visionx-rs converts Siemens Biograph Vision list-mode (`.ptd` / `.l64`) into `.tsra`. The list-mode carries time markers whose counter normally ticks steadily. Sometimes it **jumps**, for example at a clock reset or a gap. visionx handles this at its decoder boundary: it splits the stream into **segments** at each jump, and every time-keyed QC row is keyed `(segment, second)`.

- Pinned vendor fixtures: 0 jumps in 757 markers, and 1 jump in 10 markers.
- A synthetic 16 KiB input whose counter jumps at every marker is still a decodable file, but it produces one segment per marker.
- The owner's ruling (visionx-rs#257): this is **not damage, so it must not be rejected**. It is a property of the source that must be **visible** to every reader, the QC report, and any later aggregation. Only input the decoder cannot trust is refused, and that goes to quarantine.
- visionx will therefore record a typed annotation, for example:

  ```text
  clock_discontinuities { count: u64, max_gap_markers: u64, segments: u64 }
  ```

  It lives next to its existing typed provenance flags (`ModeFlag`, `carrier_flags { reason: unverified_private_creator }`).

## Requirements (what would make this general)

1. **Schema-declared and closed.** The producer declares its annotation kinds in the product schema as a closed enum of kinds, each with integer fields. No free text, and no plain `broken: bool`: a boolean can't say *how bad* or *why*. The privacy-by-schema rule applies: kinds and counts only, never values copied from the source (names, paths, header strings).
2. **Scope.** Annotations attach at product, block or table, and optionally a key range (e.g. segments 3–5, a time range). That lets a consumer mask or flag part of a product without re-reading it.
3. **Integrity.** Annotations are part of the sealed, hashed metadata (ADR-0037 signature scope; compare #407), so they cannot be stripped silently. They are deterministic, with no wall-clock values (compare #824).
4. **Carried through derivations.** Write-time views (#719), pyramids and Collections roll annotations up with a declared rule per kind (sum counts, max of maxima, union of ranges). A time-bucket partial covering an annotated range keeps the annotation, so QC drill-down shows it at any granularity.
5. **Readable without decode.** The CLI, the Python reader and the explorer list annotations (`tsra info` and friends) without decoding data blocks.
6. **Separate from provenance steps, but linked.** #782 (typed pipeline records) says *what was done*; annotations say *what the data is like*. An annotation may cite the pipeline step that detected it.
7. **Producer-owned kinds, tessera-owned mechanism.** tessera ships no PET- or vendor-specific kinds. visionx declares `clock_discontinuities`, and another producer declares its own.

## Related

#782 (typed provenance records), #568 (producer-owned aux run facts), #719 (write-time views, which must carry annotations), #407 (signature scope), #824 (determinism).

Downstream tracking: MorePET/visionx-rs#257 and #222.

