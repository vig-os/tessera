# fd5 — FAIR Data on HDF5

`fd5` is a self-describing, FAIR-principled data format for scientific data products
built on HDF5. Each file is sealed with a Merkle-tree content hash at creation time,
making it immutable and verifiable. The format is **domain-agnostic**: core conventions
(schema, provenance, units, hashing) apply to any scientific domain. Domain-specific
**product schemas** are layered on top.

Implementations exist for **Python** (reference), **Rust**, and **C/C++**.

## Quick start

### 1. Install

```bash
# from source (PyPI publishing coming soon)
git clone https://github.com/vig-os/fd5.git && cd fd5
pip install -e "."

# with optional extras
pip install -e ".[science,nifti,dicom,parquet]"
```

### 2. Create a sealed file

```python
import numpy as np
import fd5

with fd5.create(
    "output/",
    product="timeseries",
    name="sensor-log-001",
    description="Temperature and humidity readings from lab sensor",
    timestamp="2026-03-30T10:00:00Z",
) as b:
    b.write_product({
        "signals": {
            "temperature": np.array([22.1, 22.3, 22.5, 22.2]),
            "humidity": np.array([45.0, 44.8, 44.5, 45.2]),
        },
        "time": np.array([0.0, 60.0, 120.0, 180.0]),
        "sampling_rate": 1/60,
        "description": "Lab environment monitoring",
    })
```

The file is now sealed — schema embedded, content hash computed, atomically renamed.

### 3. Verify and inspect

```bash
fd5 validate output/*.h5      # schema + integrity check
fd5 info output/*.h5           # print root attrs and dataset shapes
fd5 schema-dump output/*.h5    # extract embedded JSON Schema
```

### 4. Export back to standard formats

```bash
fd5 export csv output/*.h5 -o data.csv
fd5 export parquet output/*.h5 -o data.parquet
fd5 export nifti output/*.h5 -o volume.nii.gz   # for volume data
```

### 5. Try all 3 languages (inside the devcontainer)

```bash
just try-python    # Python — uses uv
just try-rust      # Rust   — uses cargo
just try-c         # C      — uses cmake
just try-all       # all 3
```

See [`examples/`](examples/) for complete working examples in each language.

## Product schemas

fd5 ships with 12 product schemas. Use the one that fits your data:

| Schema | Domain | Use case |
|--------|--------|----------|
| `timeseries` | Generic | Sensor streams, IoT, physiological monitoring |
| `tabular` | Generic | Lab measurements, parameter tables, clinical records |
| `ndarray` | Generic | Arbitrary N-D arrays, simulation grids, image stacks |
| `recon` | Imaging | Reconstructed 3D/4D/5D volumes (PET, CT, MRI) |
| `listmode` | Imaging | Detector event tables |
| `sinogram` | Imaging | Projection / Radon data |
| `spectrum` | Imaging | Energy/lifetime histograms |
| `transform` | Imaging | Spatial transforms, registrations |
| `calibration` | Imaging | Gain maps, LUTs, normalization |
| `roi` | Imaging | Segmentations, masks, contours |
| `device_data` | Imaging | Embedded sensor streams (ECG, bellows) |
| `sim` | Imaging | Monte Carlo simulation output |

Need a schema for your domain? See the
[custom schema tutorial](docs/tutorials/custom-schema.md).

## Ingest from existing formats

```bash
fd5 ingest csv    data.csv     -o output/ --product tabular --name my-data
fd5 ingest nifti  brain.nii.gz -o output/ --product recon   --name scan-001
fd5 ingest dicom  series_dir/  -o output/ --name patient-001
fd5 ingest parquet events.pq   -o output/ --product tabular --name events
fd5 ingest raw    blob.bin     -o output/ --product ndarray --name raw-001 \
    --shape 128,128,64 --dtype float32
```

## CLI reference

```
fd5 validate           Check schema + content_hash integrity
fd5 info               Print root attrs and dataset shapes
fd5 schema-dump        Extract embedded JSON Schema
fd5 manifest           Generate manifest.toml from a directory of fd5 files
fd5 check-descriptions Validate description quality for AI-readability
fd5 migrate            Migrate file to newer schema version
fd5 datacite           Generate DataCite YAML for DOI registration
fd5 rocrate            Generate RO-Crate JSON-LD
fd5 ingest <format>    Convert external formats to sealed fd5 files
fd5 export <format>    Export fd5 files back to standard formats
```

## Multi-language support

fd5 has implementations in Python (reference), Rust, and C/C++. All produce
interoperable files validated by the cross-language conformance test suite.

| Language | Location | What it provides |
|----------|----------|-----------------|
| Python | `src/fd5/` | Full library: create, verify, validate, ingest, export, CLI |
| Rust | `crates/fd5/` | Builder, hash, verify, edit, product schema trait |
| C/C++ | `lib/c/` | Builder, hash, verify + C++17 RAII wrapper |

## Key design principles

- **HDF5 is the single source of truth** — TOML, YAML, JSON-LD are derived exports
- **One file = one data product** — sealed with `content_hash` at creation
- **Write-once, read-many** — immutable after sealing
- **Merkle-tree integrity** — tamper-evident, verifiable at any time
- **Self-describing** — embedded JSON Schema + description attrs on every group/dataset
- **Extensible** — custom product schemas via Python entry points
- **Pre-seal validation** — schema compliance + description quality checked before sealing

## Documentation

| Doc | Description |
|-----|-------------|
| [White paper](white-paper.md) | Full format specification |
| [Custom schema tutorial](docs/tutorials/custom-schema.md) | How to create your own product schema |
| [Migration guide](docs/tutorials/migration-guide.md) | Schema versioning and migration |
| [Compatibility policy](docs/COMPATIBILITY.md) | Backwards compatibility guarantees |
| [Examples](examples/) | Working examples in Python, Rust, and C |

## Development

### Prerequisites

- Python >= 3.12 + [uv](https://docs.astral.sh/uv/)
- Rust toolchain (for `crates/fd5/`)
- CMake + libhdf5-dev (for `lib/c/`)

Or use the **devcontainer** — all toolchains pre-installed:

```bash
# open in VS Code, it will offer to reopen in container
code .
```

### Running tests

```bash
# Python
uv run pytest tests/ -x

# Rust
cargo test -p fd5

# C
cd lib/c && mkdir -p build && cd build && cmake .. && make && ctest
```

### Project layout

```
src/fd5/              Python library (reference implementation)
├── generic/          Domain-agnostic schemas (timeseries, tabular, ndarray)
├── imaging/          Medical imaging schemas (recon, listmode, spectrum, ...)
├── ingest/           Format converters (DICOM, NIfTI, CSV, Parquet, raw)
├── export/           Export to standard formats (NIfTI, CSV, Parquet)
├── create.py         Fd5Builder context manager
├── hash.py           Merkle-tree hashing and verification
├── cli.py            Click CLI
└── ...

crates/fd5/           Rust implementation (hash, verify, edit, builder)
lib/c/                C/C++ implementation (builder, hash, verify)
examples/             Working examples in Python, Rust, and C
docs/                 Tutorials and specifications
```

## License

Apache-2.0. See [LICENSE](LICENSE).
