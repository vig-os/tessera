"""fd5 command-line interface."""

from __future__ import annotations

import json
import sys
from pathlib import Path
from typing import Any

import click
import h5py

from fd5.hash import verify
from fd5.ingest._base import discover_loaders
from fd5.manifest import write_manifest
from fd5.quality import check_descriptions
from fd5.rocrate import write as write_rocrate
from fd5.schema import dump_schema, validate


@click.group()
@click.version_option(package_name="fd5")
def cli() -> None:
    """fd5 – FAIR Data Format 5 toolkit."""


@cli.command()
@click.argument("file", type=click.Path(exists=True, dir_okay=False))
def validate_cmd(file: str) -> None:
    """Validate an fd5 file against its embedded schema and content_hash."""
    path = Path(file)
    errors: list[str] = []

    try:
        schema_errors = validate(path)
    except KeyError:
        click.echo("Error: file has no embedded _schema attribute.", err=True)
        sys.exit(1)

    for err in schema_errors:
        errors.append(f"Schema: {err.message}")

    if not verify(path):
        errors.append("Integrity: content_hash mismatch or missing.")

    if errors:
        for msg in errors:
            click.echo(msg, err=True)
        sys.exit(1)

    click.echo("OK – schema valid, content_hash verified.")


# click registers the command name from the function; override with the decorator
validate_cmd.name = "validate"


@cli.command()
@click.argument("file", type=click.Path(exists=True, dir_okay=False))
def info(file: str) -> None:
    """Print file metadata: root attrs and dataset shapes."""
    path = Path(file)

    with h5py.File(path, "r") as f:
        click.echo(f"File: {path.name}")

        for key in sorted(f.attrs.keys()):
            click.echo(f"  {key}: {_format_attr(f.attrs[key])}")

        datasets = _collect_datasets(f)
        if datasets:
            click.echo("Datasets:")
            for ds_path, shape, dtype in datasets:
                click.echo(f"  {ds_path}: shape={shape}, dtype={dtype}")


@cli.command("schema-dump")
@click.argument("file", type=click.Path(exists=True, dir_okay=False))
def schema_dump(file: str) -> None:
    """Extract and pretty-print the embedded JSON Schema."""
    path = Path(file)
    try:
        schema_dict = dump_schema(path)
    except KeyError:
        click.echo("Error: file has no embedded _schema attribute.", err=True)
        sys.exit(1)

    click.echo(json.dumps(schema_dict, indent=2))


@cli.command()
@click.argument("directory", type=click.Path(exists=True, file_okay=False))
@click.option(
    "--output",
    "-o",
    type=click.Path(),
    default=None,
    help="Output path for manifest.toml (default: <directory>/manifest.toml).",
)
def manifest(directory: str, output: str | None) -> None:
    """Generate manifest.toml from fd5 files in a directory."""
    dir_path = Path(directory)
    out_path = Path(output) if output else dir_path / "manifest.toml"
    write_manifest(dir_path, out_path)
    click.echo(f"Wrote {out_path}")


@cli.command()
@click.argument("directory", type=click.Path(exists=True, file_okay=False))
@click.option(
    "--output",
    "-o",
    type=click.Path(),
    default=None,
    help="Output path for datacite.yml (default: <directory>/datacite.yml).",
)
def datacite(directory: str, output: str | None) -> None:
    """Generate datacite.yml from manifest.toml in a directory."""
    from fd5.datacite import write as datacite_write

    dir_path = Path(directory)
    manifest_path = dir_path / "manifest.toml"
    if not manifest_path.is_file():
        click.echo(f"Error: {manifest_path} not found.", err=True)
        sys.exit(1)
    out_path = Path(output) if output else dir_path / "datacite.yml"
    datacite_write(manifest_path, out_path)
    click.echo(f"Wrote {out_path}")


@cli.command()
@click.argument("directory", type=click.Path(exists=True, file_okay=False))
@click.option(
    "--output",
    "-o",
    type=click.Path(),
    default=None,
    help="Output path for ro-crate-metadata.json (default: <directory>/ro-crate-metadata.json).",
)
def rocrate(directory: str, output: str | None) -> None:
    """Generate ro-crate-metadata.json from fd5 files in a directory."""
    dir_path = Path(directory)
    out_path = Path(output) if output else None
    write_rocrate(dir_path, out_path)
    written = out_path or dir_path / "ro-crate-metadata.json"
    click.echo(f"Wrote {written}")


@cli.command()
@click.argument("source", type=click.Path(exists=True, dir_okay=False))
@click.argument("output", type=click.Path())
@click.option(
    "--target",
    "-t",
    type=int,
    required=True,
    help="Target schema version.",
)
def migrate(source: str, output: str, target: int) -> None:
    """Migrate an fd5 file to a newer schema version (copy-on-write)."""
    from fd5.migrate import MigrationError
    from fd5.migrate import migrate as do_migrate

    try:
        dest = do_migrate(Path(source), Path(output), target_version=target)
        click.echo(f"Migrated to version {target}: {dest}")
    except (MigrationError, FileNotFoundError) as exc:
        click.echo(f"Error: {exc}", err=True)
        sys.exit(1)


@cli.command("datalad-register")
@click.argument("file", type=click.Path(exists=True, dir_okay=False))
@click.option(
    "--dataset",
    "-d",
    type=click.Path(exists=True, file_okay=False),
    default=None,
    help="Path to the DataLad dataset (default: parent directory of FILE).",
)
def datalad_register(file: str, dataset: str | None) -> None:
    """Register an fd5 file with a DataLad dataset."""
    from fd5.datalad import extract_metadata, register_with_datalad

    path = Path(file)

    try:
        result = register_with_datalad(path, dataset)
    except ImportError:
        click.echo(
            "Error: datalad is not installed. Install it with: pip install datalad",
            err=True,
        )
        sys.exit(1)
    except Exception as exc:
        click.echo(f"Error: {exc}", err=True)
        sys.exit(1)

    click.echo(f"Registered {path.name} with dataset {result['dataset']}")
    metadata = extract_metadata(path)
    click.echo(f"  title: {metadata.get('title', 'N/A')}")
    click.echo(f"  product: {metadata.get('product', 'N/A')}")
    click.echo(f"  id: {metadata.get('id', 'N/A')}")


@cli.command("check-descriptions")
@click.argument("file", type=click.Path(exists=True, dir_okay=False))
def check_descriptions_cmd(file: str) -> None:
    """Check description attribute quality for AI-readability."""
    path = Path(file)
    warnings = check_descriptions(path)

    if not warnings:
        click.echo("OK \u2013 all descriptions meet quality standards.")
        return

    for w in warnings:
        click.echo(f"  {w.path}: {w.message}", err=True)

    click.echo(f"\n{len(warnings)} warning(s) found.", err=True)
    sys.exit(1)


# ---------------------------------------------------------------------------
# fd5 ingest — subcommand group
# ---------------------------------------------------------------------------

_ALL_LOADER_NAMES = ("raw", "csv", "nifti", "dicom", "parquet")


def _ingest_binary(
    binary_path: Path,
    output_dir: Path,
    **kwargs: Any,
) -> Path:
    """Thin wrapper for lazy import — patchable in tests."""
    from fd5.ingest.raw import ingest_binary

    return ingest_binary(binary_path, output_dir, **kwargs)


def _get_nifti_loader():  # type: ignore[no-untyped-def]
    """Lazy-import the NiftiLoader so missing nibabel is caught at call time."""
    from fd5.ingest.nifti import NiftiLoader

    return NiftiLoader()


def _get_dicom_loader():  # type: ignore[no-untyped-def]
    """Lazy-import a DICOM loader (not yet implemented)."""
    from fd5.ingest.dicom import DicomLoader  # type: ignore[import-not-found]

    return DicomLoader()


def _get_parquet_loader():  # type: ignore[no-untyped-def]
    """Lazy-import the ParquetLoader so missing pyarrow is caught at call time."""
    from fd5.ingest.parquet import ParquetLoader

    return ParquetLoader()


@cli.group()
def ingest() -> None:
    """Ingest external data formats into sealed fd5 files."""


@ingest.command("list")
def ingest_list() -> None:
    """List available ingest loaders and their dependency status."""
    available = discover_loaders()
    click.echo("Available loaders:")
    for name in _ALL_LOADER_NAMES:
        if name in available:
            click.echo(f"  {name:<10} \u2713")
        else:
            click.echo(f"  {name:<10} \u2717 (dependency not installed)")


@ingest.command("raw")
@click.argument("source", type=click.Path(exists=True, dir_okay=False))
@click.option(
    "--output", "-o", type=click.Path(), required=True, help="Output directory."
)
@click.option("--name", required=True, help="Human-readable name.")
@click.option("--description", required=True, help="Description for AI-readability.")
@click.option("--product", required=True, help="Product type (e.g. recon).")
@click.option("--dtype", required=True, help="NumPy dtype (e.g. float32).")
@click.option("--shape", required=True, help="Comma-separated shape (e.g. 128,128,64).")
@click.option("--timestamp", default=None, help="Override ISO-8601 timestamp.")
def ingest_raw(
    source: str,
    output: str,
    name: str,
    description: str,
    product: str,
    dtype: str,
    shape: str,
    timestamp: str | None,
) -> None:
    """Ingest a raw binary file into a sealed fd5 file."""
    shape_tuple = tuple(int(s.strip()) for s in shape.split(","))
    try:
        result = _ingest_binary(
            Path(source),
            Path(output),
            dtype=dtype,
            shape=shape_tuple,
            product=product,
            name=name,
            description=description,
            timestamp=timestamp,
        )
        click.echo(f"Ingested {Path(source).name} \u2192 {result}")
    except (ValueError, FileNotFoundError) as exc:
        click.echo(f"Error: {exc}", err=True)
        sys.exit(1)


@ingest.command("csv")
@click.argument("source", type=click.Path(exists=True, dir_okay=False))
@click.option(
    "--output", "-o", type=click.Path(), required=True, help="Output directory."
)
@click.option("--name", required=True, help="Human-readable name.")
@click.option("--description", required=True, help="Description for AI-readability.")
@click.option("--product", required=True, help="Product type (e.g. spectrum).")
@click.option("--delimiter", default=",", help="Column delimiter (default: comma).")
@click.option("--timestamp", default=None, help="Override ISO-8601 timestamp.")
def ingest_csv(
    source: str,
    output: str,
    name: str,
    description: str,
    product: str,
    delimiter: str,
    timestamp: str | None,
) -> None:
    """Ingest a CSV/TSV file into a sealed fd5 file."""
    from fd5.ingest.csv import CsvLoader

    loader = CsvLoader()
    try:
        result = loader.ingest(
            Path(source),
            Path(output),
            product=product,
            name=name,
            description=description,
            timestamp=timestamp,
            delimiter=delimiter,
        )
        click.echo(f"Ingested {Path(source).name} \u2192 {result}")
    except (ValueError, FileNotFoundError) as exc:
        click.echo(f"Error: {exc}", err=True)
        sys.exit(1)


@ingest.command("nifti")
@click.argument("source", type=click.Path(exists=True))
@click.option(
    "--output", "-o", type=click.Path(), required=True, help="Output directory."
)
@click.option("--name", required=True, help="Human-readable name.")
@click.option("--description", required=True, help="Description for AI-readability.")
@click.option("--product", default="recon", help="Product type (default: recon).")
@click.option("--timestamp", default=None, help="Override ISO-8601 timestamp.")
def ingest_nifti(
    source: str,
    output: str,
    name: str,
    description: str,
    product: str,
    timestamp: str | None,
) -> None:
    """Ingest a NIfTI file (.nii / .nii.gz) into a sealed fd5 file."""
    try:
        loader = _get_nifti_loader()
    except ImportError:
        click.echo(
            "Error: nibabel is not installed. Install with: pip install 'fd5[nifti]'",
            err=True,
        )
        sys.exit(1)

    try:
        result = loader.ingest(
            Path(source),
            Path(output),
            product=product,
            name=name,
            description=description,
            timestamp=timestamp,
        )
        click.echo(f"Ingested {Path(source).name} \u2192 {result}")
    except (ValueError, FileNotFoundError) as exc:
        click.echo(f"Error: {exc}", err=True)
        sys.exit(1)


@ingest.command("dicom")
@click.argument("source", type=click.Path(exists=True))
@click.option(
    "--output", "-o", type=click.Path(), required=True, help="Output directory."
)
@click.option("--name", required=True, help="Human-readable name.")
@click.option("--description", required=True, help="Description for AI-readability.")
@click.option("--product", default="recon", help="Product type (default: recon).")
@click.option("--timestamp", default=None, help="Override ISO-8601 timestamp.")
def ingest_dicom(
    source: str,
    output: str,
    name: str,
    description: str,
    product: str,
    timestamp: str | None,
) -> None:
    """Ingest a DICOM series directory into a sealed fd5 file."""
    try:
        loader = _get_dicom_loader()
    except ImportError:
        click.echo(
            "Error: pydicom is not installed. Install with: pip install 'fd5[dicom]'",
            err=True,
        )
        sys.exit(1)

    try:
        result = loader.ingest(
            Path(source),
            Path(output),
            product=product,
            name=name,
            description=description,
            timestamp=timestamp,
        )
        click.echo(f"Ingested {Path(source).name} \u2192 {result}")
    except (ValueError, FileNotFoundError) as exc:
        click.echo(f"Error: {exc}", err=True)
        sys.exit(1)


@ingest.command("parquet")
@click.argument("source", type=click.Path(exists=True, dir_okay=False))
@click.option(
    "--output", "-o", type=click.Path(), required=True, help="Output directory."
)
@click.option("--name", required=True, help="Human-readable name.")
@click.option("--description", required=True, help="Description for AI-readability.")
@click.option("--product", required=True, help="Product type (e.g. spectrum).")
@click.option("--timestamp", default=None, help="Override ISO-8601 timestamp.")
@click.option(
    "--column-map",
    default=None,
    help="JSON string mapping source columns to fd5 columns.",
)
def ingest_parquet(
    source: str,
    output: str,
    name: str,
    description: str,
    product: str,
    timestamp: str | None,
    column_map: str | None,
) -> None:
    """Ingest a Parquet file into a sealed fd5 file."""
    try:
        loader = _get_parquet_loader()
    except ImportError:
        click.echo(
            "Error: pyarrow is not installed. Install with: pip install 'fd5[parquet]'",
            err=True,
        )
        sys.exit(1)

    parsed_map: dict[str, str] | None = None
    if column_map is not None:
        parsed_map = json.loads(column_map)

    try:
        result = loader.ingest(
            Path(source),
            Path(output),
            product=product,
            name=name,
            description=description,
            timestamp=timestamp,
            column_map=parsed_map,
        )
        click.echo(f"Ingested {Path(source).name} \u2192 {result}")
    except (ValueError, FileNotFoundError) as exc:
        click.echo(f"Error: {exc}", err=True)
        sys.exit(1)


# ---------------------------------------------------------------------------
# fd5 export — subcommand group
# ---------------------------------------------------------------------------


@cli.group()
def export() -> None:
    """Export fd5 files to standard formats."""


@export.command("nifti")
@click.argument("fd5_file", type=click.Path(exists=True))
@click.option("-o", "--output", required=True, type=click.Path())
@click.option("--dataset", default="volume", help="Dataset path to export.")
def export_nifti_cmd(fd5_file: str, output: str, dataset: str) -> None:
    """Export volume data to NIfTI (.nii.gz)."""
    from fd5.export.nifti import export_nifti

    try:
        path = export_nifti(fd5_file, output, dataset=dataset)
        click.echo(f"Exported: {path}")
    except (ImportError, KeyError, ValueError) as exc:
        click.echo(f"Error: {exc}", err=True)
        sys.exit(1)


@export.command("csv")
@click.argument("fd5_file", type=click.Path(exists=True))
@click.option("-o", "--output", required=True, type=click.Path())
@click.option("--group", default=None, help="HDF5 group path to export from.")
def export_csv_cmd(fd5_file: str, output: str, group: str | None) -> None:
    """Export tabular/timeseries/spectrum data to CSV."""
    from fd5.export.csv import export_csv

    try:
        path = export_csv(fd5_file, output, group=group)
        click.echo(f"Exported: {path}")
    except (KeyError, ValueError) as exc:
        click.echo(f"Error: {exc}", err=True)
        sys.exit(1)


@export.command("parquet")
@click.argument("fd5_file", type=click.Path(exists=True))
@click.option("-o", "--output", required=True, type=click.Path())
@click.option("--group", default=None, help="HDF5 group path to export from.")
def export_parquet_cmd(fd5_file: str, output: str, group: str | None) -> None:
    """Export tabular/timeseries data to Parquet."""
    from fd5.export.parquet import export_parquet

    try:
        path = export_parquet(fd5_file, output, group=group)
        click.echo(f"Exported: {path}")
    except (ImportError, KeyError, ValueError) as exc:
        click.echo(f"Error: {exc}", err=True)
        sys.exit(1)


# ---------------------------------------------------------------------------
# Internal helpers
# ---------------------------------------------------------------------------


def _format_attr(value: object) -> str:
    import numpy as np

    if isinstance(value, bytes):
        return value.decode("utf-8")
    if isinstance(value, np.generic):
        return str(value.item())
    return str(value)


def _collect_datasets(
    group: h5py.Group, prefix: str = ""
) -> list[tuple[str, tuple[int, ...], str]]:
    results: list[tuple[str, tuple[int, ...], str]] = []
    for key in sorted(group.keys()):
        item = group[key]
        full_path = f"{prefix}/{key}" if prefix else key
        if isinstance(item, h5py.Dataset):
            results.append((full_path, item.shape, str(item.dtype)))
        elif isinstance(item, h5py.Group):
            results.extend(_collect_datasets(item, full_path))
    return results
