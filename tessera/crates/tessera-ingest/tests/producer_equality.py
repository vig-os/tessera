#!/usr/bin/env python3
"""ADR-0056 §5's `ingest_parquet_producers` fixture: three foreign writers, one `content_hash`.

The same *logical* table is written to Parquet by **pyarrow**, **polars** and **DuckDB**, and all three
must ingest to a byte-identical payload. This is the test that actually catches the two leaks the
in-Rust corpus can only approximate:

- **dictionary ordering.** pandas' `Categorical` order is insertion-time, polars builds its own, DuckDB
  another. Preserving a writer's integer codes would import that order into `content_hash`, so the same
  logical data would seal three ways — which is why ADR-0056 §2 materialises dictionaries to their
  *values* and drops the codes.
- **nullability declaration.** The three disagree about whether a column with no nulls is *declared*
  nullable, and about how they pad a validity buffer. Tessera's boundary decides nullability from the
  **data**, not from the schema, for exactly this reason.

It lives here rather than in `tests/generic_table.rs` because it needs three third-party writers. The
Rust corpus covers the same mechanisms with writer *configurations* (dictionary on/off, row-group size,
compression) so the fast unit path is not blind to them; this is the independent confirmation that the
mechanisms generalise beyond arrow-rs's own writer.

Run by the `ingest-producer-equality` flake check:

    python3 producer_equality.py <path-to-tessera-binary>
"""

import json
import pathlib
import subprocess
import sys
import tempfile

# One logical table, given once. Deliberately includes a low-cardinality string column (the dictionary
# hazard), a nullable float column (the null hazard) and a column whose values are NOT in sorted order
# (so a first-seen dictionary differs from a sorted one).
ROWS = {
    "id": [1, 2, 3, 4, 5, 6],
    "label": ["zeta", "alpha", "zeta", "mu", "alpha", "zeta"],
    "energy": [511.0, None, 7.25, -0.5, None, 1e-9],
}

# Fixed identity inputs — a corpus-shaped comparison must never see a clock or a temp path.
NAME = "producers"
TIMESTAMP = "2024-01-01T00:00:00Z"
LABEL = "corpus/producers"


def write_pyarrow(path: pathlib.Path) -> None:
    import pyarrow
    import pyarrow.parquet

    table = pyarrow.table(
        {
            "id": pyarrow.array(ROWS["id"], type=pyarrow.int64()),
            "label": pyarrow.array(ROWS["label"], type=pyarrow.string()),
            "energy": pyarrow.array(ROWS["energy"], type=pyarrow.float64()),
        }
    )
    # Dictionary encoding ON and a small row group, i.e. the choices most likely to leak.
    pyarrow.parquet.write_table(
        table, path, use_dictionary=True, compression="snappy", row_group_size=2
    )


def write_polars(path: pathlib.Path) -> None:
    import polars

    frame = polars.DataFrame(
        {
            "id": polars.Series(ROWS["id"], dtype=polars.Int64),
            "label": polars.Series(ROWS["label"], dtype=polars.Utf8),
            "energy": polars.Series(ROWS["energy"], dtype=polars.Float64),
        }
    )
    # A different compression and no explicit row-group hint: a second set of physical choices.
    frame.write_parquet(path, compression="zstd")


def write_duckdb(path: pathlib.Path, workdir: pathlib.Path) -> None:
    import duckdb

    # Go through SQL rather than the Python API, so this really is DuckDB's own writer taking its own
    # decisions — and it is the exact command ADR-0056 §8 tells a CSV user to run, so the path a user is
    # advised onto is the path under test.
    csv = workdir / "duck.csv"
    lines = ["id,label,energy"]
    for i, lab, en in zip(ROWS["id"], ROWS["label"], ROWS["energy"]):
        lines.append(f"{i},{lab},{'' if en is None else repr(en)}")
    csv.write_text("\n".join(lines) + "\n")
    con = duckdb.connect()
    con.execute(
        "COPY (SELECT CAST(id AS BIGINT) AS id, label, CAST(energy AS DOUBLE) AS energy "
        f"FROM read_csv('{csv}', header=true, columns={{'id':'BIGINT','label':'VARCHAR','energy':'DOUBLE'}})) "
        f"TO '{path}' (FORMAT parquet)"
    )
    con.close()


WRITERS = {
    "pyarrow": write_pyarrow,
    "polars": write_polars,
    "duckdb": write_duckdb,
}


def ingest(tessera: str, source: pathlib.Path, out: pathlib.Path) -> dict:
    """Ingest one Parquet file and return the sealed hashes."""
    subprocess.run(
        [
            tessera,
            "ingest",
            "table",
            str(source),
            str(out),
            "--name",
            NAME,
            "--timestamp",
            TIMESTAMP,
            # A fixed label, so the sealed `ingested_from` reference does not carry the temp path. The
            # edge still pins a digest over the SOURCE BYTES, which genuinely differ between writers —
            # that is why `manifest_hash` is expected to differ and `content_hash` is not.
            "--source-label",
            LABEL,
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    proc = subprocess.run(
        [tessera, "inspect", str(out)], check=True, capture_output=True, text=True
    )
    hashes = {}
    for line in proc.stdout.splitlines():
        parts = line.split()
        if len(parts) == 2 and parts[0] in {"id", "content_hash", "manifest_hash"}:
            hashes[parts[0]] = parts[1]
    missing = {"id", "content_hash", "manifest_hash"} - hashes.keys()
    if missing:
        raise SystemExit(f"could not read {sorted(missing)} from:\n{proc.stdout}")
    return hashes


def main() -> int:
    if len(sys.argv) != 2:
        raise SystemExit(f"usage: {sys.argv[0]} <path-to-tessera>")
    tessera = sys.argv[1]

    with tempfile.TemporaryDirectory() as tmp:
        work = pathlib.Path(tmp)
        results = {}
        sizes = {}
        for producer, write in WRITERS.items():
            source = work / f"{producer}.parquet"
            if producer == "duckdb":
                write(source, work)
            else:
                write(source)
            sizes[producer] = source.stat().st_size
            results[producer] = ingest(tessera, source, work / f"{producer}.tsra")
            print(
                f"{producer:8s} {sizes[producer]:7d} B  {results[producer]['content_hash']}"
            )

        # The three source files must genuinely differ, or the whole comparison is vacuous — the same
        # trap ADR-0057 §5 names for the corpus itself.
        if len(set(sizes.values())) == 1:
            raise SystemExit(
                f"all three writers produced the same file size {sizes}; they are probably not "
                "really making different physical choices, so this check proves nothing"
            )

        # THE assertion: one payload, three writers.
        payloads = {p: r["content_hash"] for p, r in results.items()}
        if len(set(payloads.values())) != 1:
            raise SystemExit(
                "content_hash DIFFERS between Parquet writers, so a physical writer choice reached "
                "the payload — dictionary ordering or null padding is leaking into the seal "
                f"(ADR-0056 §2/§5):\n{json.dumps(payloads, indent=2)}"
            )
        # `id` too: identity is over the declared inputs, which are the same for all three.
        ids = {p: r["id"] for p, r in results.items()}
        if len(set(ids.values())) != 1:
            raise SystemExit(
                f"id differs between writers:\n{json.dumps(ids, indent=2)}"
            )

        # …and `manifest_hash` must DIFFER, because the `ingested_from` edge pins a digest over the
        # source bytes and the three sources are different bytes. Asserting this too is what stops the
        # check from being satisfied by a bug that made every hash constant.
        seals = {p: r["manifest_hash"] for p, r in results.items()}
        if len(set(seals.values())) != len(seals):
            raise SystemExit(
                "manifest_hash is the SAME across writers with different source bytes — the "
                f"ingested_from source digest is not reaching the seal:\n{json.dumps(seals, indent=2)}"
            )

    print(f"OK  one content_hash across {len(WRITERS)} independent Parquet writers")
    return 0


if __name__ == "__main__":
    sys.exit(main())
