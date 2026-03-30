# Schema Migration Guide

## Overview

fd5 files are sealed and immutable -- once created, a file's data, metadata, and content hash are fixed. Schema changes therefore require **migration to a new file**. The migration framework creates a new file with an updated schema, links it back to the original via provenance, and recomputes the content hash.

Migrations are registered as Python callables keyed by `(product, from_version, to_version)`. When multiple single-step migrations are registered, fd5 chains them automatically (e.g., v1 -> v2 -> v3).

## How migration works

The `fd5.migrate()` function performs a copy-on-write upgrade:

1. **Read source metadata.** Open the source file and read its `_schema_version` (int) and `product` attributes.
2. **Resolve the migration chain.** Look up registered migration callables that walk from the current version to the target version, one step at a time.
3. **Apply each step.** For every step in the chain:
   - Open the previous file (or original source) for reading and the next file for writing.
   - Copy all root-level attributes from source to destination.
   - Call the migration callable `fn(src, dst)`, which copies datasets and applies transformations.
   - Set `_schema_version` on the destination to the step's target version.
   - Intermediate files use a temporary directory and are cleaned up automatically.
4. **Write provenance.** Record a `sources/migrated_from` entry on the final file that links back to the original file's `id`, `content_hash`, path, and product.
5. **Recompute content hash.** Call `compute_content_hash()` on the final file and store the result in the `content_hash` attribute.
6. **Return the destination path.**

The source file is never modified. The destination file is a fully sealed fd5 file with a valid content hash.

## Writing a migration callable

A migration callable receives two open `h5py.File` handles -- `src` (read-only) and `dst` (writable) -- and is responsible for copying data and applying transformations. Root-level attributes are already copied before the callable runs, so you only need to handle groups and datasets.

Here is a worked example: a hypothetical `recon` product migration from v1 to v2 that adds an optional `statistics` group with summary statistics per volume.

```python
import h5py
import numpy as np

from fd5.migrate import register_migration


def recon_v1_to_v2(src: h5py.File, dst: h5py.File) -> None:
    """Migrate recon schema from v1 to v2.

    v2 adds a 'statistics' group with min/max/mean/std per volume.
    """
    # Copy existing volume data
    if "volume" in src:
        src.copy("volume", dst)

        # Compute statistics for the new field
        vol = src["volume"][...]
        stats = dst.require_group("statistics")
        stats.attrs["min"] = float(np.min(vol))
        stats.attrs["max"] = float(np.max(vol))
        stats.attrs["mean"] = float(np.mean(vol))
        stats.attrs["std"] = float(np.std(vol))
        stats.attrs["description"] = "Volume statistics computed during migration"

    # Copy all other groups/datasets
    for key in src:
        if key not in dst and key != "volume":
            src.copy(key, dst)


# Register the migration
register_migration("recon", from_version=1, to_version=2, fn=recon_v1_to_v2)
```

**Key points:**

- `src.copy(name, dst)` is the most efficient way to copy HDF5 groups and datasets -- it preserves chunking, compression, and attributes.
- Root attributes are automatically copied from source to destination before your callable runs. You do not need to copy them manually.
- `_schema_version`, `content_hash`, and `id` are managed by the migration framework. Do not set them in your callable.
- The callable is free to add new groups, datasets, and attributes to `dst`.

## Registering migrations

Use `register_migration()` to associate a callable with a specific product and version transition:

```python
from fd5.migrate import register_migration

register_migration(
    product="recon",
    from_version=1,
    to_version=2,
    fn=recon_v1_to_v2,
)
```

Each `(product, from_version, to_version)` key can only be registered once. Attempting to register a duplicate raises `ValueError`.

For testing, `clear_migrations()` removes all registered migrations:

```python
from fd5.migrate import clear_migrations

clear_migrations()
```

## Running migrations

### CLI

```bash
fd5 migrate source.fd5.h5 /output/migrated.fd5.h5 --target 2
```

The `--target` (or `-t`) flag specifies the desired schema version. The command prints the output path on success or an error message on failure.

### Python API

```python
from fd5 import migrate

new_path = migrate(
    source_path="source.fd5.h5",
    dest_path="/output/migrated.fd5.h5",
    target_version=2,
)
```

`migrate()` returns a `pathlib.Path` pointing to the newly created file.

**Error handling:**

- `FileNotFoundError` -- source file does not exist.
- `fd5.migrate.MigrationError` -- file is already at or above the target version, or no migration is registered for one of the required steps.

## Chaining migrations

When multiple single-step migrations are registered for a product, fd5 automatically chains them. Each step must increment the version by exactly one:

```python
from fd5.migrate import register_migration

register_migration("recon", from_version=1, to_version=2, fn=v1_to_v2)
register_migration("recon", from_version=2, to_version=3, fn=v2_to_v3)

# Migrating a v1 file to v3 automatically runs: v1 -> v2 -> v3
```

Intermediate files are created in a temporary location and deleted after the final step completes. Only the final output file persists.

If any step in the chain is missing (e.g., no migration registered for v2 -> v3), `MigrationError` is raised before any files are written.

## Testing migrations

Test each migration step independently. Verify that the output file has the expected structure, a valid content hash, and correct provenance:

```python
import h5py
import numpy as np
import pytest

from fd5.hash import compute_content_hash
from fd5.migrate import clear_migrations, migrate, register_migration


@pytest.fixture(autouse=True)
def _clean_registry():
    """Ensure a clean migration registry for each test."""
    yield
    clear_migrations()


def test_recon_v1_to_v2(tmp_path):
    # 1. Create a v1 recon file
    src = tmp_path / "source_v1.h5"
    with h5py.File(src, "w") as f:
        f.attrs["product"] = "recon"
        f.attrs["_schema_version"] = np.int64(1)
        f.attrs["id"] = "sha256:abc123"
        f.attrs["id_inputs"] = "product + name + timestamp"
        f.attrs["name"] = "test"
        f.attrs["description"] = "v1 test file"
        f.attrs["timestamp"] = "2026-01-01T00:00:00Z"
        f.create_dataset("volume", data=np.ones((4, 4), dtype=np.float32))
        f.attrs["content_hash"] = compute_content_hash(f)

    # 2. Register migration and run
    register_migration("recon", 1, 2, recon_v1_to_v2)
    dst = tmp_path / "migrated_v2.h5"
    migrate(src, dst, target_version=2)

    # 3. Verify output
    with h5py.File(dst, "r") as f:
        # Schema version updated
        assert int(f.attrs["_schema_version"]) == 2

        # Statistics group added
        assert "statistics" in f
        assert f["statistics"].attrs["mean"] == pytest.approx(1.0)

        # Content hash is valid
        stored_hash = f.attrs["content_hash"]
        if isinstance(stored_hash, bytes):
            stored_hash = stored_hash.decode()
        assert stored_hash == compute_content_hash(f)

        # Provenance links back to original
        assert "sources" in f
        src_grp = f["sources/migrated_from"]
        role = src_grp.attrs["role"]
        if isinstance(role, bytes):
            role = role.decode()
        assert role == "migration_source"
```

## Best practices

- **Always copy data you do not modify.** Use `src.copy(name, dst)` to preserve chunking, compression, and attributes.
- **Never modify the source file.** The source handle is opened read-only; any attempt to write to it will raise an error.
- **Add `description` attributes to new groups and datasets.** This keeps fd5 files self-documenting.
- **Test round-trip integrity.** Create a file at version N, migrate to N+1, and verify the content hash passes `fd5.verify()`.
- **Register migrations at module import time.** Place `register_migration()` calls in `__init__.py` or a dedicated `migrations.py` module so they are available when the CLI or API runs.
- **Keep migration callables focused.** Each callable should handle exactly one version step. Complex migrations are easier to debug when broken into single-step functions.
- **Do not copy root attributes manually.** The framework copies all root attributes before your callable runs. If you need to remove or rename an attribute, modify `dst.attrs` inside your callable.
