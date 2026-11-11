"""Generate the #801 draft consumer schemas.

Two files per product, both in shapes tessera already serialises:
  <product>.schema.json  -- a `tessera_core::schema::ProductSchema` (deserialisable today).
  <product>.tables.json  -- the PROPOSED per-role column contract: for every table role, a list of
                            `tessera_core::block::table::Column` objects (the existing serde shape),
                            plus the proposed `sequence_key` (#802), `keys` / `references` (#780).

Usage (from the repo root): python3 docs/spikes/801-consumer-schemas/generate.py docs/spikes/801-consumer-schemas
"""

import json
import sys
from pathlib import Path

OUT = Path(sys.argv[1])


def field(
    fid, desc, dtype, *, unit=None, required=False, recommended=False, vocabulary=None
):
    f = {"id": fid, "description": desc, "dtype": dtype}
    if unit:
        f["unit"] = unit
    if vocabulary:
        f["vocabulary"] = vocabulary
    f["required"] = required
    f["recommended"] = recommended
    f["sensitivity"] = "public"
    return f


def block(role, kind, min_count, desc):
    b = {"role": role}
    if kind:
        b["kind"] = kind
    b["min_count"] = min_count
    b["description"] = desc
    return b


def col(name, dtype, unit, desc, *, nullable=False):
    c = {"name": name, "dtype": dtype, "description": desc, "unit": unit}
    if nullable:
        c["nullable"] = True
    return c


def vec(
    prefix, dtype, unit, desc, n=3, *, nullable=False, axes="xyz", unit_suffix=True
):
    """A fixed-size vector as N scalar columns: the writer has no FixedSizeList (gap G10)."""
    sfx = f"_{unit}" if unit_suffix and unit not in ("1", "") else ""
    names = [f"{prefix}_{a}{sfx}" for a in axes[:n]]
    return [
        col(nm, dtype, unit, f"{desc} ({a})", nullable=nullable)
        for nm, a in zip(names, axes)
    ]


def idx(prefix, dtype, unit, desc, n, *, nullable=False):
    """A fixed-size per-row tuple as `<prefix>.0 … <prefix>.N-1` (the ADR-0056 fixed_list_expand naming)."""
    return [
        col(f"{prefix}.{i}", dtype, unit, f"{desc} [{i}]", nullable=nullable)
        for i in range(n)
    ]


SIM_FIELDS = [
    field(
        "simulation_mode",
        "g4ring mode: normal | phase_space_writer | phase_space_reader",
        "string",
        required=True,
    ),
    field(
        "number_of_events",
        "Events requested for the run (numberOfEvents; one value per run, never summed)",
        "uint64",
        required=True,
    ),
    field(
        "events_run",
        "Events actually simulated over all workers (the sum of the shards' events_run)",
        "uint64",
        required=True,
    ),
    field(
        "save_all_events",
        "Whether events with no pethits are kept (saveAllEvents)",
        "bool",
        required=True,
    ),
    field(
        "edep_threshold_keV",
        "Per-crystal deposit threshold applied before a pethit is written",
        "float64",
        unit="keV",
        required=True,
    ),
    field(
        "scanner_axial_axis",
        "Scanner axial axis in the world frame: x | y | z",
        "string",
        required=True,
    ),
    field(
        "seed",
        "Master RNG seed; with seedOncePerCommunication = 0 it fixes every event's seed pair",
        "int64",
        required=True,
    ),
    field(
        "run_id",
        "Operator batch index (runId); with event_id it keys rows across batches",
        "int64",
        recommended=True,
    ),
    field(
        "event_id_offset",
        "eventIdOffset added to every Geant4 event id written (resume / batching)",
        "uint64",
        recommended=True,
    ),
]

products = {}

# ---- 1. gPET-sim normal output -------------------------------------------------------------------
products["gpet-sim:g4ring-output"] = (
    {
        "product": "gpet-sim:g4ring-output",
        "version": "3.0",
        "description": "gPET-sim g4ring normal-mode (Stage 2) output: kept events and their per-crystal pethits.",
        "fields": SIM_FIELDS,
        "blocks": [
            block(
                "events",
                "table",
                1,
                "One row per kept event (all events when save_all_events)",
            ),
            block(
                "pethits",
                "table",
                0,
                "One row per (event, crystal, source photon) above edep_threshold_keV",
            ),
        ],
        "requires_generation": True,
    },
    {
        "events": {
            "sequence_key": ["event_id"],
            "keys": [["event_id"]],
            "references": [],
            "columns": [
                col(
                    "event_id",
                    "u8",
                    "1",
                    "Geant4 event id + event_id_offset; unique within the run",
                ),
                col("n_pethits", "u2", "1", "Number of pethits rows of this event"),
                *vec("source", "f4", "mm", "Sampled source position, world frame"),
                *vec(
                    "annih",
                    "f4",
                    "mm",
                    "Positron annihilation vertex, world frame; NULL when the event has none",
                    nullable=True,
                ),
            ],
        },
        "pethits": {
            "sequence_key": ["event_id", "hit_ordinal"],
            "keys": [["event_id", "hit_ordinal"]],
            "references": [
                {
                    "columns": ["event_id"],
                    "to": {
                        "scope": "block",
                        "block": "events",
                        "columns": ["event_id"],
                    },
                },
                {
                    "columns": ["crystal_id"],
                    "to": {
                        "scope": "source(crystal_map)",
                        "block": "crystals",
                        "columns": ["crystal_id"],
                    },
                },
            ],
            "columns": [
                col("event_id", "u8", "1", "Owning event (references events.event_id)"),
                col(
                    "hit_ordinal",
                    "u4",
                    "1",
                    "PROPOSED: position of the hit within its event in g4ring's flush order; not written today",
                ),
                col(
                    "hit_type",
                    "u2",
                    "1",
                    "0 prompt, 1 annihilation photon 1, 2 annihilation photon 2, 4 pre",
                ),
                col(
                    "crystal_id",
                    "u4",
                    "1",
                    "GDML copy number of the crystal (references the crystal map)",
                ),
                col(
                    "edep_keV", "f4", "keV", "Aggregated energy deposit in the crystal"
                ),
                col(
                    "time_ps",
                    "f8",
                    "ps",
                    "Time of the earliest deposit since the event start",
                ),
                col("nHits", "u2", "1", "Geant4 steps aggregated into this hit"),
                col(
                    "n_compton",
                    "u1",
                    "1",
                    "Compton scatters before the hit (saturates at 255); NULL when not recorded",
                    nullable=True,
                ),
                col(
                    "n_rayleigh",
                    "u1",
                    "1",
                    "Rayleigh scatters before the hit (saturates at 255); NULL when not recorded",
                    nullable=True,
                ),
                *vec("pos", "f4", "mm", "Energy-weighted deposit centroid"),
                *vec("first", "f4", "mm", "Position of the earliest deposit"),
            ],
        },
    },
)

# ---- 2. gPET-sim Stage 1 phase space -------------------------------------------------------------
products["gpet-sim:g4ring-phase-space"] = (
    {
        "product": "gpet-sim:g4ring-phase-space",
        "version": "3.0",
        "description": "gPET-sim g4ring Stage 1 phase space: every particle leaving the CT bounding box.",
        "fields": SIM_FIELDS
        + [
            field(
                "phase_space_format_version",
                "Encoding version of source_photon_id (3 since schema 3.0)",
                "int32",
                required=True,
            ),
        ],
        "blocks": [
            block(
                "events",
                "table",
                1,
                "One row per simulated event, including empty ones",
            ),
            block(
                "phase_space_particles",
                "table",
                1,
                "One row per particle crossing the CT bounding box",
            ),
        ],
        "requires_generation": True,
    },
    {
        "events": {
            "sequence_key": ["event_id"],
            "keys": [["event_id"]],
            "references": [],
            "columns": [
                col("event_id", "u8", "1", "Geant4 event id + event_id_offset"),
                col(
                    "n_particles", "u4", "1", "phase_space_particles rows of this event"
                ),
                *vec("source", "f4", "mm", "Sampled source position, world frame"),
                *vec(
                    "annih",
                    "f4",
                    "mm",
                    "Positron annihilation vertex; NULL when none",
                    nullable=True,
                ),
            ],
        },
        "phase_space_particles": {
            "sequence_key": ["event_id", "particle_ordinal"],
            "keys": [["event_id", "particle_ordinal"]],
            "references": [
                {
                    "columns": ["event_id"],
                    "to": {
                        "scope": "block",
                        "block": "events",
                        "columns": ["event_id"],
                    },
                },
            ],
            "columns": [
                col("event_id", "u8", "1", "Owning event (references events.event_id)"),
                col(
                    "particle_ordinal",
                    "u4",
                    "1",
                    "PROPOSED: position of the crossing within its event; not written today",
                ),
                col(
                    "source_photon_id",
                    "i2",
                    "1",
                    "0 prompt, 1 annihilation 1, 2 annihilation 2, -1 other",
                ),
                col("pdg_code", "i4", "1", "PDG particle code"),
                *vec("pos", "f4", "mm", "Exit position on the CT bounding box"),
                *vec("dir", "f4", "1", "Unit direction vector"),
                col("kinetic_energy_keV", "f4", "keV", "Kinetic energy at exit"),
                col("time_ps", "f8", "ps", "Time at exit since the event start"),
                col(
                    "n_compton",
                    "u1",
                    "1",
                    "Compton scatters before exit; NULL when not recorded",
                    nullable=True,
                ),
                col(
                    "n_rayleigh",
                    "u1",
                    "1",
                    "Rayleigh scatters before exit; NULL when not recorded",
                    nullable=True,
                ),
            ],
        },
    },
)

# ---- 3. crystal map (g4ring input; the target of pethits.crystal_id) ------------------------------
products["gpet-sim:crystal-map"] = (
    {
        "product": "gpet-sim:crystal-map",
        "version": "2.0",
        "description": "Scanner crystal map: centre, orientation and GDML volume per crystal_id.",
        "fields": [
            field(
                "crystal_size_axial_mm",
                "Crystal size along the axial axis",
                "float64",
                unit="mm",
                required=True,
            ),
            field(
                "crystal_size_tangential_mm",
                "Crystal size along the tangential axis",
                "float64",
                unit="mm",
                required=True,
            ),
            field(
                "crystal_size_radial_mm",
                "Crystal size along the radial axis",
                "float64",
                unit="mm",
                required=True,
            ),
            field(
                "scanner_axial_axis",
                "Scanner axial axis in the world frame: x | y | z",
                "string",
                required=True,
            ),
            field("n_crystals", "Number of crystals", "int64", required=True),
        ],
        "blocks": [block("crystals", "table", 1, "One row per crystal")],
        "requires_generation": False,
    },
    {
        "crystals": {
            "sequence_key": ["crystal_id"],
            "keys": [["crystal_id"]],
            "references": [],
            "columns": [
                col("crystal_id", "u4", "1", "GDML copy number"),
                *vec(
                    "center",
                    "f8",
                    "mm",
                    "Crystal centre, world frame",
                    unit_suffix=False,
                ),
                *vec("dir", "f8", "1", "Crystal radial direction (unit vector)"),
                col("volume_name", "str", "1", "GDML physical-volume name"),
            ],
        },
    },
)

# ---- 4. gPET-sim2listmode singles ----------------------------------------------------------------
products["gpet-sim2listmode:listmode-singles"] = (
    {
        "product": "gpet-sim2listmode:listmode-singles",
        "version": "3.0",
        "description": "Time-ordered list-mode singles built from g4ring output (intrinsic + tracer sources).",
        "fields": [
            field(
                "min_edep_keV",
                "Minimum single energy kept",
                "float64",
                unit="keV",
                required=True,
            ),
            field(
                "edep_threshold_keV",
                "g4ring threshold carried forward from the inputs",
                "float64",
                unit="keV",
                recommended=True,
            ),
            field(
                "scanner_axial_axis",
                "Scanner axial axis in the world frame: x | y | z",
                "string",
                recommended=True,
            ),
            field(
                "acquisition_complete",
                "Whether the acquisition reached its configured end",
                "bool",
                required=True,
            ),
            field(
                "sources",
                "Per-source facts (intrinsic, tracers/<name>): event_count, dropped_* counts, "
                "has_* flags, detection_acceptance, simulated_events",
                "json",
                required=True,
            ),
            field(
                "statistics",
                "min/max/mean energy, min/max time (the /statistics group)",
                "json",
                recommended=True,
            ),
        ],
        "blocks": [
            block(
                "listmode_singles",
                "table",
                1,
                "One row per single, non-decreasing absolute_time_ps",
            ),
            block(
                "source_files",
                "table",
                1,
                "Input shard per source with its event_id offset and sha256",
            ),
            block(
                "decay_channels",
                "table",
                0,
                "Per tracer: decay-channel intensities and lifetimes",
            ),
            block(
                "hit_type_counts",
                "table",
                0,
                "Singles per hit_type (the /statistics hit_types/hit_counts)",
            ),
        ],
        "requires_generation": True,
    },
    {
        "listmode_singles": {
            "sequence_key": [],
            "order": "producer",
            "keys": [],
            "references": [
                {
                    "columns": ["source_type", "source_event_id"],
                    "to": {
                        "scope": "source(g4ring_output)",
                        "block": "events",
                        "columns": ["event_id"],
                    },
                    "note": "source_event_id is remapped by the source_files offsets; resolving it needs them",
                },
            ],
            "columns": [
                col("edep_keV", "f4", "keV", "Single energy"),
                col("crystal_id", "u4", "1", "GDML copy number"),
                col(
                    "hit_type",
                    "u2",
                    "1",
                    "0 prompt, 1 annihilation 1, 2 annihilation 2",
                ),
                col(
                    "nHits",
                    "u2",
                    "1",
                    "Aggregated Geant4 steps; NULL when not recorded",
                    nullable=True,
                ),
                col(
                    "absolute_time_ps",
                    "f8",
                    "ps",
                    "Acquisition time of the single (Poisson clock)",
                ),
                col(
                    "relative_time_ps", "f8", "ps", "Time since the source event start"
                ),
                col(
                    "source_event_id",
                    "u8",
                    "1",
                    "Event id remapped to be unique within its source",
                ),
                col("source_type", "u2", "1", "0 intrinsic, 1..N tracer index"),
                *vec("source", "f4", "mm", "Source position of the event"),
                col(
                    "n_compton",
                    "u1",
                    "1",
                    "Compton scatters; NULL when not recorded",
                    nullable=True,
                ),
                col(
                    "n_rayleigh",
                    "u1",
                    "1",
                    "Rayleigh scatters; NULL when not recorded",
                    nullable=True,
                ),
                *vec(
                    "pos",
                    "f4",
                    "mm",
                    "Energy-weighted position; NULL when the input has none",
                    nullable=True,
                ),
                *vec(
                    "annih",
                    "f4",
                    "mm",
                    "Annihilation vertex (opt-in); NULL when not recorded",
                    nullable=True,
                ),
                *vec(
                    "first",
                    "f4",
                    "mm",
                    "Earliest-deposit position (opt-in); NULL when not recorded",
                    nullable=True,
                ),
            ],
        },
        "source_files": {
            "sequence_key": ["source", "file_index"],
            "keys": [["source", "file_index"]],
            "references": [],
            "columns": [
                col("source", "str", "1", "intrinsic | tracers/<name>"),
                col(
                    "file_index",
                    "u4",
                    "1",
                    "Position of the shard in the source's input list",
                ),
                col("path", "str", "1", "Shard path as given to the run"),
                col(
                    "event_id_offset",
                    "u8",
                    "1",
                    "Offset added to the shard's event ids",
                ),
                col("sha256", "str", "1", "SHA-256 of the shard"),
            ],
        },
        "decay_channels": {
            "sequence_key": ["tracer", "channel"],
            "keys": [["tracer", "channel"]],
            "references": [],
            "columns": [
                col("tracer", "str", "1", "Tracer name"),
                col("channel", "u2", "1", "Channel index"),
                col("intensity", "f8", "1", "Branching intensity"),
                col("tau_ps", "f8", "ps", "Lifetime"),
            ],
        },
        "hit_type_counts": {
            "sequence_key": ["hit_type"],
            "keys": [["hit_type"]],
            "references": [],
            "columns": [
                col("hit_type", "u2", "1", "hit_type value"),
                col("count", "u8", "1", "Singles of that hit_type"),
            ],
        },
    },
)


# ---- 5. geddf-analysis coincidences --------------------------------------------------------------
def tm():
    return col("tm_ms", "u4", "ms", "Millisecond time marker of the row")


products["geddf:coincidences"] = (
    {
        "product": "geddf:coincidences",
        "version": "3.0",
        "description": "geddf-analysis coincidences: 2- and 3-photon coincidences and events from simulated singles.",
        "fields": [
            field(
                "scanner_axial_axis",
                "Scanner axial axis in the world frame: x | y | z",
                "string",
                required=True,
            ),
            field(
                "has_hit_pos",
                "Whether events_2p carries the hit_pos columns",
                "bool",
                required=True,
            ),
            field(
                "has_singles",
                "Whether the singles block is present",
                "bool",
                required=True,
            ),
            field(
                "clock_unit_ps",
                "Duration of one dt_clk / td_clk tick",
                "float64",
                unit="ps",
                required=True,
            ),
        ],
        "blocks": [
            block("events_2p", "table", 1, "2-photon events"),
            block("events_3p", "table", 1, "3-photon events"),
            block("coin_2p", "table", 1, "2-photon coincidences"),
            block("coin_3p", "table", 1, "3-photon coincidences"),
            block("time_markers", "table", 1, "One row per populated millisecond"),
            block(
                "coin_size_dist", "table", 1, "Coincidence-size histogram (size, count)"
            ),
            block(
                "singles",
                "table",
                0,
                "The singles the coincidences were built from (opt-in)",
            ),
            block(
                "lyso_map",
                "array",
                1,
                "(n_tx, n_ax, 4) float64: calibration, x, z, y per channel",
            ),
        ],
        "requires_generation": True,
    },
    {
        "events_2p": {
            "sequence_key": [],
            "order": "producer",
            "keys": [],
            "references": [],
            "columns": [
                tm(),
                col(
                    "dt_clk",
                    "i8",
                    "1",
                    "Time difference in clock ticks (clock_unit_ps)",
                ),
                *idx("en_keV", "f4", "keV", "Photon energy", 2),
                *idx("ax", "u2", "1", "Axial channel", 2),
                *idx("tx", "u2", "1", "Transaxial channel", 2),
                *vec("vtx", "f4", "mm", "Reconstructed vertex"),
                col(
                    "event_label",
                    "u1",
                    "1",
                    "Truth label (was proc_data/event_labels_2p)",
                ),
                *vec(
                    "true_vtx",
                    "f4",
                    "mm",
                    "True vertex; NULL for non-TRUE events",
                    nullable=True,
                ),
                *[
                    col(
                        f"hit_pos.{i}.{a}_mm",
                        "f4",
                        "mm",
                        f"Hit {i} position ({a}); NULL when missing",
                        nullable=True,
                    )
                    for i in range(2)
                    for a in "xyz"
                ],
            ],
        },
        "events_3p": {
            "sequence_key": [],
            "order": "producer",
            "keys": [],
            "references": [],
            "columns": [
                tm(),
                col("dt_clk", "i8", "1", "Time difference in clock ticks"),
                *idx("en_keV", "f4", "keV", "Photon energy", 3),
                *idx("ax", "u2", "1", "Axial channel", 3),
                *idx("tx", "u2", "1", "Transaxial channel", 3),
                *vec("vtx", "f4", "mm", "Reconstructed vertex"),
                col("lt_ns", "f4", "ns", "Positronium lifetime estimate"),
                col("lt_corr_ns", "f4", "ns", "Corrected lifetime estimate"),
                col(
                    "event_label",
                    "u1",
                    "1",
                    "Truth label (was proc_data/event_labels_3p)",
                ),
                *vec(
                    "true_vtx",
                    "f4",
                    "mm",
                    "True vertex; NULL for non-TRUE events",
                    nullable=True,
                ),
            ],
        },
        "coin_2p": {
            "sequence_key": [],
            "order": "producer",
            "keys": [],
            "references": [],
            "columns": [
                tm(),
                *idx("en_keV", "u2", "keV", "Quantised photon energy", 2),
                *idx("ax", "u2", "1", "Axial channel", 2),
                *idx("tx", "u2", "1", "Transaxial channel", 2),
                col("dt_clk", "i8", "1", "Time difference in clock ticks"),
                col(
                    "coin_label",
                    "u1",
                    "1",
                    "Truth label (was proc_data/coin_labels_2p)",
                ),
            ],
        },
        "coin_3p": {
            "sequence_key": [],
            "order": "producer",
            "keys": [],
            "references": [],
            "columns": [
                tm(),
                *idx("en_keV", "u2", "keV", "Quantised photon energy", 3),
                *idx("ax", "u2", "1", "Axial channel", 3),
                *idx("tx", "u2", "1", "Transaxial channel", 3),
                *idx("dt_clk", "i8", "1", "Time difference in clock ticks", 2),
                col(
                    "coin_label",
                    "u1",
                    "1",
                    "Truth label (was proc_data/coin_labels_3p)",
                ),
            ],
        },
        "time_markers": {
            "sequence_key": ["tm_ms"],
            "keys": [["tm_ms"]],
            "references": [],
            "columns": [
                tm(),
                col("idx", "u8", "1", "Index of the millisecond's first single"),
                col("length_ms", "u1", "ms", "Marker length"),
                col("n_singles", "u4", "1", "Singles in the millisecond"),
                col("n_coins2p", "u4", "1", "2-photon coincidences in the millisecond"),
                col("n_coins3p", "u4", "1", "3-photon coincidences in the millisecond"),
                col("n_event2p", "u4", "1", "2-photon events in the millisecond"),
                col("n_event3p", "u4", "1", "3-photon events in the millisecond"),
            ],
        },
        "coin_size_dist": {
            "sequence_key": ["size"],
            "keys": [["size"]],
            "references": [],
            "columns": [
                col("size", "i8", "1", "Coincidence size"),
                col("count", "i8", "1", "Coincidences of that size"),
            ],
        },
        "singles": {
            "sequence_key": [],
            "order": "producer",
            "keys": [],
            "references": [],
            "columns": [
                tm(),
                col("en_keV", "u2", "keV", "Quantised energy"),
                col("ax", "u2", "1", "Axial channel"),
                col("tx", "u2", "1", "Transaxial channel"),
                col("td_clk", "i8", "1", "Time to the next single in clock ticks"),
            ],
        },
    },
)

# ---- 6. geddf-analysis pairwise record (metadata-only product) ------------------------------------
products["geddf:pairwise-record"] = (
    {
        "product": "geddf:pairwise-record",
        "version": "3.0",
        "description": "geddf-analysis pairwise summary: double/triple counts by class, windows and cuts.",
        "fields": [
            field(
                "doubles",
                "pairs, true, scatter, random, prompt, detscat counts",
                "json",
                required=True,
            ),
            field(
                "triples",
                "triples, true, scatter, same_decay_bad_types, random counts",
                "json",
                required=True,
            ),
            field(
                "windows",
                "ng_window_keV, lt_window_ns, max_dt_clk, clock_unit_ps",
                "json",
                required=True,
            ),
            field("cuts", "Energy and time cuts applied", "json", required=True),
            field(
                "singles_span_s",
                "Time span of the singles",
                "float64",
                unit="s",
                required=True,
            ),
            field("n_singles", "Singles read", "uint64", required=True),
            field("n_511", "Singles in the 511 keV window", "uint64", required=True),
            field("n_ng", "Singles in the non-511 window", "uint64", required=True),
            field("window_diag", "Window diagnostics", "json", recommended=True),
        ],
        "blocks": [],
        "requires_generation": True,
    },
    {},
)

OUT.mkdir(parents=True, exist_ok=True)
for name, (schema, tables) in products.items():
    stem = name.replace(":", "__")
    (OUT / f"{stem}.schema.json").write_text(json.dumps(schema, indent=2) + "\n")
    if tables:
        doc = {"product": name, "status": "proposed (#801)", "tables": tables}
        (OUT / f"{stem}.tables.json").write_text(json.dumps(doc, indent=2) + "\n")
sys.stdout.write("\n".join(sorted(p.name for p in OUT.iterdir())) + "\n")
